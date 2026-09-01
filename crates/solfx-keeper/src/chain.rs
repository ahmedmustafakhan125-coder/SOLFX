//! RPC access and transaction submission.

use std::sync::Arc;

use anchor_lang::{AccountDeserialize, Discriminator};
use anyhow::{anyhow, Context as _, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use solana_account_decoder_client_types::{UiAccountData, UiAccountEncoding};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_rpc_client_api::filter::{Memcmp, RpcFilterType};
use solana_signer::Signer;
use solana_transaction::Transaction;

use crate::config::Config;
use crate::throttle::Throttle;

pub struct Chain {
    pub rpc: Arc<RpcClient>,
    pub payer: Arc<Keypair>,
    pub commitment: CommitmentConfig,
    /// Paces every RPC call this process makes. See [`crate::throttle`].
    ///
    /// The keeper is a busier RPC client than it looks: the scan loop runs every 400 ms and a
    /// book refresh reads four account sets. On its own that is fine; beside the price poster
    /// on one free-tier key it is not, and the poster is the one that suffers because its
    /// calls are the ones with a 60-second deadline. Measured: the poster alone ran 11
    /// consecutive clean passes, and lost 491 feeds across 371 passes while a keeper ran.
    throttle: Arc<Throttle>,
    priority_fee: u64,
    cu_limit: u32,
    dry_run: bool,
}

impl Chain {
    pub fn new(cfg: &Config, payer: Keypair) -> Self {
        Self {
            rpc: Arc::new(RpcClient::new_with_commitment(
                cfg.rpc_url.clone(),
                cfg.commitment(),
            )),
            payer: Arc::new(payer),
            commitment: cfg.commitment(),
            throttle: Arc::new(Throttle::new(cfg.max_rps)),
            priority_fee: cfg.priority_fee_micro_lamports,
            cu_limit: cfg.compute_unit_limit,
            dry_run: cfg.dry_run,
        }
    }

    pub fn pubkey(&self) -> Pubkey {
        self.payer.pubkey()
    }

    /// Fetch one Anchor account and deserialize it.
    pub async fn account<T: AccountDeserialize>(&self, key: &Pubkey) -> Result<T> {
        self.throttle.acquire().await;
        let data = self
            .rpc
            .get_account_data(key)
            .await
            .with_context(|| format!("fetching {key}"))?;
        T::try_deserialize(&mut data.as_slice()).map_err(|e| anyhow!("deserializing {key}: {e}"))
    }

    /// Fetch every account of one Anchor type owned by `program`.
    ///
    /// Filtered on the 8-byte discriminator server-side. Without that filter this returns
    /// every account the program owns — positions, users, markets and orders together — and
    /// the response grows with total protocol usage rather than with what we asked for.
    pub async fn all<T: AccountDeserialize + Discriminator>(
        &self,
        program: &Pubkey,
    ) -> Result<Vec<(Pubkey, T)>> {
        let cfg = RpcProgramAccountsConfig {
            filters: Some(vec![RpcFilterType::Memcmp(Memcmp::new_base58_encoded(
                0,
                T::DISCRIMINATOR,
            ))]),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                commitment: Some(self.commitment),
                ..Default::default()
            },
            ..Default::default()
        };
        self.throttle.acquire().await;
        let raw = self
            .rpc
            .get_program_ui_accounts_with_config(program, cfg)
            .await
            .context("get_program_accounts")?;

        // A single undeserializable account must not blind the keeper to the rest of the
        // book: after a program upgrade adds a field, old accounts can trail the new layout,
        // and dropping the whole scan would stop liquidations protocol-wide.
        let mut out = Vec::with_capacity(raw.len());
        for (key, account) in raw {
            let UiAccountData::Binary(encoded, UiAccountEncoding::Base64) = &account.data else {
                tracing::warn!(%key, "unexpected account encoding");
                continue;
            };
            let Ok(bytes) = BASE64.decode(encoded) else {
                tracing::warn!(%key, "undecodable account payload");
                continue;
            };
            match T::try_deserialize(&mut bytes.as_slice()) {
                Ok(parsed) => out.push((key, parsed)),
                Err(e) => tracing::warn!(%key, error = %e, "skipping undeserializable account"),
            }
        }
        Ok(out)
    }

    /// Fetch many accounts at once, preserving order and reporting misses as `None`.
    pub async fn multiple(&self, keys: &[Pubkey]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut out = Vec::with_capacity(keys.len());
        // `getMultipleAccounts` caps at 100 keys per call.
        for chunk in keys.chunks(100) {
            self.throttle.acquire().await;
            let accounts = self
                .rpc
                .get_multiple_accounts(chunk)
                .await
                .context("get_multiple_accounts")?;
            out.extend(accounts.into_iter().map(|a| a.map(|a| a.data)));
        }
        Ok(out)
    }

    /// Send one instruction, prefixed with a compute-unit limit and a priority-fee bid.
    ///
    /// Returns `Ok(None)` in dry-run mode. Errors are returned rather than logged, because
    /// which failures are expected depends on the caller: a liquidator racing another keeper
    /// *expects* to lose sometimes, and treating that as an incident trains operators to
    /// ignore the alerts that matter.
    pub async fn send(&self, ix: Instruction, label: &str) -> Result<Option<String>> {
        if self.dry_run {
            tracing::info!(action = label, "dry run: not sending");
            return Ok(None);
        }
        let ixs = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(self.cu_limit),
            ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee),
            ix,
        ];
        self.throttle.acquire().await;
        let blockhash = self
            .rpc
            .get_latest_blockhash()
            .await
            .context("get_latest_blockhash")?;
        let message = Message::new(&ixs, Some(&self.pubkey()));
        let mut tx = Transaction::new_unsigned(message);
        tx.try_sign(&[self.payer.as_ref()], blockhash)
            .context("signing")?;

        self.throttle.acquire().await;
        let sig = self
            .rpc
            .send_and_confirm_transaction(&tx)
            .await
            .with_context(|| format!("sending {label}"))?;
        Ok(Some(sig.to_string()))
    }
}

/// Whether a failure is one a keeper should shrug at.
///
/// Losing a race is the *normal* outcome of a permissionless design working as intended —
/// several keepers see the same liquidatable position and only one lands. Logging that at
/// error level teaches operators to ignore the log, which is how the one real failure gets
/// missed.
pub fn is_benign_race(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}");
    [
        "AccountNotInitialized",
        "NotLiquidatable",
        "TriggerNotMet",
        "AccountOwnedByWrongProgram",
        "already been processed",
        "ConstraintSeeds",
        "FundingNotDue",
        "SessionUnchanged",
        "PriceUnchanged",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}
