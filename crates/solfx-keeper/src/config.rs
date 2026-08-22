//! Keeper configuration.
//!
//! Everything here has a default that works against a local validator, because a keeper you
//! cannot start without a config file is a keeper nobody runs during development — and the
//! whole argument for permissionless liquidation is that *other people* run one.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use clap::Parser;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;

/// Pyth's public Hermes endpoint. Used only to *check* whether an on-chain price account has
/// fallen behind the real market; see [`crate::pyth`].
pub const DEFAULT_HERMES: &str = "https://hermes.pyth.network";

#[derive(Parser, Debug, Clone)]
#[command(name = "solfx-keeper", about = "SolFX off-chain keepers")]
pub struct Config {
    /// Which keepers to run. Several may be given; they share one RPC client and one book.
    #[arg(long, value_enum, default_values_t = [Service::Liquidator, Service::Triggers, Service::Cranks, Service::Watchdog], env = "SOLFX_SERVICES")]
    pub service: Vec<Service>,

    #[arg(long, default_value = "http://127.0.0.1:8899", env = "SOLFX_RPC_URL")]
    pub rpc_url: String,

    /// Signer. Needs SOL for fees and, for the liquidator, a USDC token account.
    #[arg(long, default_value = "keeper.json", env = "SOLFX_KEYPAIR")]
    pub keypair: PathBuf,

    /// The liquidator's USDC account, where penalties are paid. Trigger tips arrive as
    /// lamports on the signer itself and need no token account.
    #[arg(long, env = "SOLFX_REWARD_TOKEN_ACCOUNT")]
    pub reward_token_account: Option<Pubkey>,

    #[arg(long, default_value = DEFAULT_HERMES, env = "SOLFX_HERMES_URL")]
    pub hermes_url: String,

    /// How often to re-read the whole book from the chain.
    ///
    /// This is the **slow** loop. It exists so a keeper that has been running for a week and
    /// has drifted, or one that just started, converges on the truth; it is not how
    /// liquidations are found in time. See `scan_interval`.
    #[arg(long, default_value_t = 30, env = "SOLFX_REFRESH_SECS")]
    pub refresh_secs: u64,

    /// How often to re-price the mirrored book against the latest oracle accounts.
    ///
    /// This is the **fast** loop and it sets the liquidation latency floor, so Phase 7's
    /// "sub-2s" target is really a statement about this number plus one block.
    #[arg(long, default_value_t = 400, env = "SOLFX_SCAN_MS")]
    pub scan_ms: u64,

    /// Funding and session cranks are hourly and daily jobs; polling them fast is waste.
    #[arg(long, default_value_t = 60, env = "SOLFX_CRANK_SECS")]
    pub crank_secs: u64,

    /// Micro-lamports per compute unit. A liquidation competes for blockspace during exactly
    /// the congestion spike that caused it, so bidding zero means arriving after the move.
    #[arg(long, default_value_t = 50_000, env = "SOLFX_PRIORITY_FEE")]
    pub priority_fee_micro_lamports: u64,

    #[arg(long, default_value_t = 400_000, env = "SOLFX_CU_LIMIT")]
    pub compute_unit_limit: u32,

    /// Report a position as a near miss once its health factor drops below this, so an
    /// operator sees pressure building rather than only the liquidations that resulted.
    #[arg(long, default_value_t = 15_000, env = "SOLFX_WATCH_HF_BPS")]
    pub watch_health_factor_bps: u64,

    /// Refuse to act on a book older than this. A keeper that cannot reach its RPC must go
    /// quiet, not keep firing decisions based on a snapshot from ten minutes ago.
    #[arg(long, default_value_t = 20, env = "SOLFX_MAX_BOOK_AGE_SECS")]
    pub max_book_age_secs: u64,

    /// Log every evaluation rather than only actions. Very loud; for a single position under
    /// investigation.
    #[arg(long, env = "SOLFX_TRACE_EVAL")]
    pub trace_eval: bool,

    /// Evaluate and log, but never send a transaction. The honest way to try a new build
    /// against mainnet.
    #[arg(long, env = "SOLFX_DRY_RUN")]
    pub dry_run: bool,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Liquidations and auto-deleveraging.
    Liquidator,
    /// Take-profit and stop-loss execution.
    Triggers,
    /// Funding, session and price cranks.
    Cranks,
    /// Compares on-chain price accounts against Hermes and complains when they diverge.
    /// Read-only: it sends no transactions.
    Watchdog,
}

impl Config {
    pub fn wants(&self, s: Service) -> bool {
        self.service.contains(&s)
    }

    pub fn refresh_interval(&self) -> Duration {
        Duration::from_secs(self.refresh_secs.max(1))
    }

    pub fn scan_interval(&self) -> Duration {
        Duration::from_millis(self.scan_ms.max(50))
    }

    pub fn crank_interval(&self) -> Duration {
        Duration::from_secs(self.crank_secs.max(5))
    }

    /// `processed` deliberately.
    ///
    /// A liquidator waiting for `confirmed` is a liquidator reacting to a book two blocks
    /// stale, which during a fast move is the difference between a penalty and a bad debt.
    /// The cost of being wrong is one rejected transaction — the program re-checks every
    /// condition, so a keeper's optimism is never authoritative.
    pub fn commitment(&self) -> CommitmentConfig {
        CommitmentConfig::processed()
    }

    pub fn load_keypair(&self) -> Result<Keypair> {
        let raw = std::fs::read_to_string(&self.keypair)
            .with_context(|| format!("reading keypair {}", self.keypair.display()))?;
        let bytes: Vec<u8> = serde_json::from_str(&raw)
            .with_context(|| format!("parsing keypair {}", self.keypair.display()))?;
        Keypair::try_from(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("invalid keypair {}: {e}", self.keypair.display()))
    }
}
