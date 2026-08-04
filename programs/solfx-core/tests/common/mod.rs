//! LiteSVM harness for the Phase 2 integration suite.
//!
//! Runs the real BPF binary in-process — no validator, no ledger, no RPC. That is what makes
//! it cheap enough to re-check invariant I1 after **every** instruction rather than once at
//! the end of a scenario.
//!
//! # Oracle accounts are synthesised, not fetched
//!
//! [`Env::post_price`] writes a `PriceUpdateV2` account directly into the SVM with the Pyth
//! receiver program as its owner. That is deliberate: it lets the suite reproduce the exact
//! oracle conditions Phase 0b *measured* — a Friday close read 21.4 hours later, USD/IDR's
//! 30.11 bps band, a price dated ahead of the cluster clock — none of which can be summoned
//! on demand from a live feed. Anchor's ownership check is not bypassed; the account really
//! is owned by the receiver program, so the program under test validates it exactly as it
//! would in production.

// Test code asserts against known values and unwraps expected-Ok results. The workspace
// denies `unwrap`, `expect` and `panic` because a panic in a *program* is a failed
// transaction with no named error; in a test a panic is the reporting mechanism, and
// routing every assertion through a Result would make the suite unreadable without making
// anything safer.
#![allow(
    dead_code,
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, Discriminator, InstructionData, ToAccountMetas};
use litesvm::LiteSVM;
use pyth_solana_receiver_sdk::price_update::{PriceFeedMessage, PriceUpdateV2, VerificationLevel};
use solana_account::Account as SvmAccount;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

use solfx_core::constants::*;
use solfx_core::instructions::admin::{
    InitializeMarketParams, InitializeProtocolParams, UpdateFeeParams, UpdateRiskParams,
};
use solfx_core::state::{
    FeedKind, InsuranceFund, LpPool, Market, MarketStatus, PriceSource, Protocol,
    QuoteConversionKind, UserAccount,
};

/// The Pyth pull-oracle receiver. Taken from the SDK rather than written out, so the tests
/// and the program can never disagree about which program owns a price account.
pub const PYTH_RECEIVER_ID: Pubkey = pyth_solana_receiver_sdk::ID;

/// Fixed wall-clock for the suite. Absolute value is irrelevant; what matters is that tests
/// can move it relative to a price's `publish_time`.
pub const T0: i64 = 1_800_000_000;

pub const ONE_USDC: u64 = 1_000_000;

/// `anchor_lang::prelude::Result` is a single-parameter alias, so the standard two-parameter
/// form needs its own name inside a module that glob-imports the prelude.
pub type TestResult = core::result::Result<(), String>;

// Feed ids are deterministic fixtures, not real Pyth ids. The engine does not care what the
// bytes are — only that the id on `Market` matches the id in the price update, which is the
// property under test. Validating a *real* id against live Hermes data is the Phase 9
// listing checklist's job and cannot be done from a unit test.
pub const FEED_EUR_USD: [u8; 32] = [0x11; 32];
pub const FEED_GBP_USD: [u8; 32] = [0x22; 32];
pub const FEED_XAU_USD: [u8; 32] = [0x33; 32];
pub const FEED_USD_JPY: [u8; 32] = [0x44; 32];
pub const FEED_USD_INR: [u8; 32] = [0x55; 32];
pub const FEED_BTC_USD: [u8; 32] = [0x66; 32];

/// How a synthetic price update should look. Defaults to a healthy EUR/USD tick.
#[derive(Clone, Copy)]
pub struct PriceSpec {
    /// Pyth mantissa (before `exponent` is applied).
    pub price: i64,
    pub conf: u64,
    pub exponent: i32,
    pub publish_time: i64,
    /// Pyth's own EMA, the deviation reference. Zero disables the deviation check.
    pub ema_price: i64,
    pub verification_level: VerificationLevel,
}

impl Default for PriceSpec {
    fn default() -> Self {
        // 1.08543 at expo -8, with a 1.28 bps band — EUR/USD's measured p50.
        Self {
            price: 108_543_000,
            conf: 13_893,
            exponent: -8,
            publish_time: T0,
            ema_price: 108_543_000,
            verification_level: VerificationLevel::Full,
        }
    }
}

impl PriceSpec {
    pub fn at(price: i64) -> Self {
        Self {
            price,
            ema_price: price,
            ..Default::default()
        }
    }
    pub fn conf(mut self, conf: u64) -> Self {
        self.conf = conf;
        self
    }
    pub fn published_at(mut self, ts: i64) -> Self {
        self.publish_time = ts;
        self
    }
    pub fn ema(mut self, ema: i64) -> Self {
        self.ema_price = ema;
        self
    }
    pub fn verification(mut self, v: VerificationLevel) -> Self {
        self.verification_level = v;
        self
    }
}

/// A market configuration, with presets drawn from the Phase 0b measurements.
#[derive(Clone)]
pub struct MarketSpec {
    pub params: InitializeMarketParams,
}

impl MarketSpec {
    /// Baseline: direct feed, USD-quoted, Tier 1 risk envelope.
    fn base(symbol: &str, feed: [u8; 32]) -> Self {
        Self {
            params: InitializeMarketParams {
                symbol: symbol.to_string(),
                feed_kind: FeedKind::SpotFx,
                price_source: PriceSource::Direct,
                pyth_feed_id: feed,
                secondary_feed_id: [0u8; 32],
                quote_conversion_feed: [0u8; 32],
                quote_conversion_kind: QuoteConversionKind::None,

                // Interbank week: Sunday 21:00 UTC to Friday 21:00 UTC.
                session_open_dow: 0,
                session_open_seconds: 75_600,
                session_close_dow: 5,
                session_close_seconds: 75_600,

                weekend_max_leverage: 0,
                weekend_oi_cap_bps: 0,
                weekend_spread_bps: 0,
                weekend_max_conf_bps: 0,

                max_leverage: 50,
                imr_bps: 200,
                mmr_bps: 100,
                liquidation_fee_bps: 50,
                max_oi_long: 1_000_000 * ONE_USDC,
                max_oi_short: 1_000_000 * ONE_USDC,
                max_position_size: 100_000 * ONE_USDC,
                min_position_size: ONE_USDC,

                max_staleness_seconds: 10,
                max_conf_bps: 15,
                max_deviation_bps: 300,

                base_spread_bps: 1,
                conf_spread_multiplier_bps: 10_000,
                skew_impact_bps_per_unit: 0,

                open_fee_rate: 100_000, // 1.0 bps at RATE_PRECISION
                close_fee_rate: 100_000,
                funding_rate_cap_per_hour: 1_000_000,
                carry_rate_per_hour: 0,
            },
        }
    }

    /// EUR/USD — direct, USD-quoted. Measured conf p50 1.28 bps.
    pub fn eur_usd() -> Self {
        Self::base("EURUSD", FEED_EUR_USD)
    }

    /// XAU/USD — the tightest feed measured (1.43 bps p50), and **weekday only**: Phase 0b
    /// found every Pyth metals feed frozen at the Friday close.
    pub fn xau_usd() -> Self {
        let mut s = Self::base("XAUUSD", FEED_XAU_USD);
        s.params.max_conf_bps = 10;
        s
    }

    /// USD/INR — non-USD-quoted, so PnL lands in Rupees and needs conversion (C-3).
    /// EM leverage is a fraction of majors: the risk shape is a policy gap, not a range.
    pub fn usd_inr() -> Self {
        let mut s = Self::base("USDINR", FEED_USD_INR);
        s.params.quote_conversion_feed = FEED_USD_INR;
        s.params.quote_conversion_kind = QuoteConversionKind::QuotePerUsd;
        s.params.max_leverage = 20;
        s.params.imr_bps = 500;
        s.params.mmr_bps = 250;
        s.params.liquidation_fee_bps = 100;
        s.params.max_conf_bps = 15;
        s
    }

    /// EUR/GBP composed from EUR/USD ÷ GBP/USD. Off the v1 critical path — Pyth carries the
    /// cross natively and tighter — but the path must work before it is ever needed.
    pub fn eur_gbp_synthetic() -> Self {
        let mut s = Self::base("EURGBP", FEED_EUR_USD);
        s.params.price_source = PriceSource::Synthetic { invert_quote: true };
        s.params.secondary_feed_id = FEED_GBP_USD;
        s.params.max_leverage = 25;
        s.params.imr_bps = 400;
        s.params.mmr_bps = 200;
        s.params.liquidation_fee_bps = 100;
        s.params.max_conf_bps = 30;
        s
    }

    /// BTC/USD — the generic-engine proof. Crypto needs no new maths, only a regime.
    pub fn btc_usd() -> Self {
        let mut s = Self::base("BTCUSD", FEED_BTC_USD);
        s.params.feed_kind = FeedKind::Crypto;
        s.params.max_leverage = 20;
        s.params.imr_bps = 500;
        s.params.mmr_bps = 250;
        s.params.liquidation_fee_bps = 100;
        s.params.max_conf_bps = 50;
        s
    }
}

pub struct User {
    pub keypair: Keypair,
    pub account: Pubkey,
    pub token_account: Pubkey,
}

impl User {
    pub fn pubkey(&self) -> Pubkey {
        self.keypair.pubkey()
    }
}

pub struct Env {
    pub svm: LiteSVM,
    pub admin: Keypair,
    pub guardian: Keypair,
    pub usdc_mint: Pubkey,
    pub protocol: Pubkey,
    pub collateral_vault: Pubkey,
    pub fee_vault: Pubkey,
    pub insurance_fund: Pubkey,
    pub insurance_vault: Pubkey,
    pub lp_pool: Pubkey,
    pub lp_vault: Pubkey,
    pub lp_mint: Pubkey,
    pub now: i64,
    users: Vec<Pubkey>,
}

impl Env {
    /// Boot the SVM, load the built program, create a 6-decimal collateral mint.
    ///
    /// Panics if `target/deploy/solfx_core.so` is missing — run `anchor build` first.
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();

        let so = std::fs::read(so_path()).unwrap_or_else(|e| {
            panic!(
                "could not read the program binary at {:?}: {e}.\n\
                 Run `anchor build` before `cargo test -p solfx-core`.",
                so_path()
            )
        });
        svm.add_program(solfx_core::ID, &so).unwrap();

        let admin = Keypair::new();
        let guardian = Keypair::new();
        svm.airdrop(&admin.pubkey(), 1_000 * 1_000_000_000).unwrap();
        svm.airdrop(&guardian.pubkey(), 1_000_000_000).unwrap();

        let usdc_mint = Pubkey::new_unique();

        let mut env = Self {
            svm,
            admin,
            guardian,
            usdc_mint,
            protocol: pda(&[PROTOCOL_SEED]),
            collateral_vault: pda(&[COLLATERAL_VAULT_SEED]),
            fee_vault: pda(&[FEE_VAULT_SEED]),
            insurance_fund: pda(&[INSURANCE_FUND_SEED]),
            insurance_vault: pda(&[INSURANCE_VAULT_SEED]),
            lp_pool: pda(&[LP_POOL_SEED]),
            lp_vault: pda(&[LP_VAULT_SEED]),
            lp_mint: pda(&[LP_MINT_SEED]),
            now: T0,
            users: Vec::new(),
        };

        env.write_mint_with_decimals(usdc_mint, USDC_DECIMALS);
        env.set_clock(T0);
        env
    }

    // --- clock -------------------------------------------------------------------------

    pub fn set_clock(&mut self, unix_timestamp: i64) {
        self.now = unix_timestamp;
        let clock = Clock {
            slot: 1,
            epoch_start_timestamp: unix_timestamp,
            epoch: 1,
            leader_schedule_epoch: 1,
            unix_timestamp,
        };
        self.svm.set_sysvar::<Clock>(&clock);
    }

    pub fn advance_clock(&mut self, seconds: i64) {
        self.set_clock(self.now + seconds);
    }

    // --- transactions ------------------------------------------------------------------

    /// Send one instruction as its own transaction.
    ///
    /// The blockhash is expired first. Two identical instructions from the same signer on the
    /// same blockhash produce the same transaction signature, and the runtime rejects the
    /// second as `AlreadyProcessed` — a dedup response that looks nothing like a program
    /// error and would otherwise be mistaken for one. Tests here legitimately repeat
    /// instructions (setting a status twice, depositing the same amount twice), so every send
    /// gets a fresh blockhash.
    pub fn send(&mut self, ix: Instruction, signers: &[&Keypair]) -> TestResult {
        self.svm.expire_blockhash();
        let payer = signers.first().expect("at least one signer").pubkey();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer),
            signers,
            self.svm.latest_blockhash(),
        );
        self.svm.send_transaction(tx).map(|_| ()).map_err(|e| {
            // The whole log is kept: an Anchor error name usually appears there rather than
            // in the top-level error, and a test that can only say "it failed" is a test
            // that cannot distinguish a real regression from a changed error code.
            format!("{:?}\n{}", e.err, e.meta.logs.join("\n"))
        })
    }

    /// Send and return the consumed compute units — the input to `docs/compute-budget.md`.
    pub fn send_metered(&mut self, ix: Instruction, signers: &[&Keypair]) -> u64 {
        self.svm.expire_blockhash();
        let payer = signers.first().expect("at least one signer").pubkey();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer),
            signers,
            self.svm.latest_blockhash(),
        );
        match self.svm.send_transaction(tx) {
            Ok(meta) => meta.compute_units_consumed,
            Err(e) => panic!("{:?}\n{}", e.err, e.meta.logs.join("\n")),
        }
    }

    // --- protocol ----------------------------------------------------------------------

    pub fn default_protocol_params(&self) -> InitializeProtocolParams {
        InitializeProtocolParams {
            guardian: self.guardian.pubkey(),
            fee_split_lp_bps: 5_500,
            fee_split_treasury_bps: 2_500,
            fee_split_insurance_bps: 1_000,
            fee_split_referral_bps: 1_000,
            lp_withdrawal_cooldown_seconds: 86_400,
            lp_exit_fee_bps: 5,
            insurance_target_balance: 100_000 * ONE_USDC,
        }
    }

    pub fn init_protocol_ix(&self, params: InitializeProtocolParams) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::InitializeProtocol {
                admin: self.admin.pubkey(),
                protocol: self.protocol,
                usdc_mint: self.usdc_mint,
                collateral_vault: self.collateral_vault,
                fee_vault: self.fee_vault,
                insurance_fund: self.insurance_fund,
                insurance_vault: self.insurance_vault,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                lp_mint: self.lp_mint,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                rent: <Rent as anchor_lang::solana_program::sysvar::SysvarId>::id(),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::InitializeProtocol { params }.data(),
        }
    }

    pub fn init_protocol(&mut self) {
        let ix = self.init_protocol_ix(self.default_protocol_params());
        let admin = self.admin.insecure_clone();
        self.send(ix, &[&admin])
            .expect("initialize_protocol failed");
    }

    // --- markets -----------------------------------------------------------------------

    pub fn market_pda(index: u16) -> Pubkey {
        pda(&[MARKET_SEED, &index.to_le_bytes()])
    }

    pub fn init_market_ix(&self, index: u16, spec: &MarketSpec) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::InitializeMarket {
                admin: self.admin.pubkey(),
                protocol: self.protocol,
                market: Self::market_pda(index),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::InitializeMarket {
                market_index: index,
                params: spec.params.clone(),
            }
            .data(),
        }
    }

    pub fn init_market(&mut self, index: u16, spec: &MarketSpec) -> Pubkey {
        let ix = self.init_market_ix(index, spec);
        let admin = self.admin.insecure_clone();
        self.send(ix, &[&admin])
            .unwrap_or_else(|e| panic!("initialize_market({index}) failed: {e}"));
        Self::market_pda(index)
    }

    pub fn set_status_ix(&self, index: u16, status: MarketStatus) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AdminMarket {
                admin: self.admin.pubkey(),
                protocol: self.protocol,
                market: Self::market_pda(index),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::SetMarketStatus { new_status: status }.data(),
        }
    }

    pub fn activate_market(&mut self, index: u16) {
        let ix = self.set_status_ix(index, MarketStatus::Active);
        let admin = self.admin.insecure_clone();
        self.send(ix, &[&admin]).expect("activate failed");
    }

    /// List a market and activate it in one step.
    pub fn list_and_activate(&mut self, index: u16, spec: &MarketSpec) -> Pubkey {
        let m = self.init_market(index, spec);
        self.activate_market(index);
        m
    }

    pub fn update_risk_ix(&self, index: u16, params: UpdateRiskParams) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AdminMarket {
                admin: self.admin.pubkey(),
                protocol: self.protocol,
                market: Self::market_pda(index),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::UpdateMarketRiskParams { params }.data(),
        }
    }

    pub fn update_fees_ix(&self, index: u16, params: UpdateFeeParams) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AdminMarket {
                admin: self.admin.pubkey(),
                protocol: self.protocol,
                market: Self::market_pda(index),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::UpdateMarketFeeParams { params }.data(),
        }
    }

    // --- authority ---------------------------------------------------------------------

    fn admin_only_metas(
        &self,
        signer: Pubkey,
    ) -> Vec<anchor_lang::solana_program::instruction::AccountMeta> {
        solfx_core::accounts::AdminOnly {
            admin: signer,
            protocol: self.protocol,
        }
        .to_account_metas(None)
    }

    pub fn transfer_admin_ix(&self, new_admin: Pubkey) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: self.admin_only_metas(self.admin.pubkey()),
            data: solfx_core::instruction::TransferAdmin { new_admin }.data(),
        }
    }

    pub fn accept_admin_ix(&self, signer: Pubkey) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AcceptAdmin {
                pending_admin: signer,
                protocol: self.protocol,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::AcceptAdmin {}.data(),
        }
    }

    pub fn set_guardian_ix(&self, new_guardian: Pubkey) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: self.admin_only_metas(self.admin.pubkey()),
            data: solfx_core::instruction::SetGuardian { new_guardian }.data(),
        }
    }

    pub fn fee_splits_ix(&self, params: solfx_core::instructions::FeeSplitParams) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: self.admin_only_metas(self.admin.pubkey()),
            data: solfx_core::instruction::UpdateFeeSplits { params }.data(),
        }
    }

    pub fn pause_ix(&self) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::GuardianPause {
                guardian: self.guardian.pubkey(),
                protocol: self.protocol,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::EmergencyPause {}.data(),
        }
    }

    pub fn unpause_ix(&self, signer: Pubkey) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AdminUnpause {
                admin: signer,
                protocol: self.protocol,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::Unpause {}.data(),
        }
    }

    pub fn halt_market_ix(&self, index: u16) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::GuardianHaltMarket {
                guardian: self.guardian.pubkey(),
                protocol: self.protocol,
                market: Self::market_pda(index),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::HaltMarket {}.data(),
        }
    }

    // --- users -------------------------------------------------------------------------

    pub fn user_pda(authority: &Pubkey) -> Pubkey {
        pda(&[USER_SEED, authority.as_ref()])
    }

    pub fn create_user_ix(&self, authority: &Pubkey, referrer: Pubkey) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::InitializeUserAccount {
                authority: *authority,
                protocol: self.protocol,
                user_account: Self::user_pda(authority),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::InitializeUserAccount { referrer }.data(),
        }
    }

    /// Create a funded trader with `usdc` in their wallet token account.
    pub fn new_user(&mut self, usdc: u64, referrer: Pubkey) -> User {
        let keypair = Keypair::new();
        let authority = keypair.pubkey();
        self.svm.airdrop(&authority, 100 * 1_000_000_000).unwrap();

        let token_account = Pubkey::new_unique();
        self.write_token_account(token_account, self.usdc_mint, authority, usdc);

        let ix = self.create_user_ix(&authority, referrer);
        self.send(ix, &[&keypair]).expect("create user failed");

        let account = Self::user_pda(&authority);
        self.users.push(account);
        User {
            keypair,
            account,
            token_account,
        }
    }

    fn move_collateral_metas(
        &self,
        user: &User,
    ) -> Vec<anchor_lang::solana_program::instruction::AccountMeta> {
        solfx_core::accounts::MoveCollateral {
            authority: user.pubkey(),
            protocol: self.protocol,
            user_account: user.account,
            collateral_mint: self.usdc_mint,
            collateral_vault: self.collateral_vault,
            user_token_account: user.token_account,
            token_program: spl_token::ID,
        }
        .to_account_metas(None)
    }

    pub fn deposit_ix(&self, user: &User, amount: u64) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: self.move_collateral_metas(user),
            data: solfx_core::instruction::DepositCollateral { amount }.data(),
        }
    }

    pub fn withdraw_ix(&self, user: &User, amount: u64) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: self.move_collateral_metas(user),
            data: solfx_core::instruction::WithdrawCollateral { amount }.data(),
        }
    }

    pub fn deposit(&mut self, user: &User, amount: u64) -> TestResult {
        let ix = self.deposit_ix(user, amount);
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn withdraw(&mut self, user: &User, amount: u64) -> TestResult {
        let ix = self.withdraw_ix(user, amount);
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    // --- oracle ------------------------------------------------------------------------

    /// Write a `PriceUpdateV2` account owned by the Pyth receiver program.
    pub fn post_price(&mut self, feed_id: [u8; 32], spec: PriceSpec) -> Pubkey {
        let key = Pubkey::new_unique();
        self.overwrite_price(key, feed_id, spec);
        key
    }

    /// Write a price account owned by an arbitrary program, to prove the ownership gate.
    pub fn post_price_owned_by(
        &mut self,
        feed_id: [u8; 32],
        spec: PriceSpec,
        owner: Pubkey,
    ) -> Pubkey {
        let key = Pubkey::new_unique();
        self.write_price_account(key, feed_id, spec, owner);
        key
    }

    pub fn overwrite_price(&mut self, key: Pubkey, feed_id: [u8; 32], spec: PriceSpec) {
        self.write_price_account(key, feed_id, spec, PYTH_RECEIVER_ID);
    }

    fn write_price_account(
        &mut self,
        key: Pubkey,
        feed_id: [u8; 32],
        spec: PriceSpec,
        owner: Pubkey,
    ) {
        let update = PriceUpdateV2 {
            write_authority: Pubkey::default(),
            verification_level: spec.verification_level,
            price_message: PriceFeedMessage {
                feed_id,
                price: spec.price,
                conf: spec.conf,
                exponent: spec.exponent,
                publish_time: spec.publish_time,
                prev_publish_time: spec.publish_time - 1,
                ema_price: spec.ema_price,
                ema_conf: spec.conf,
            },
            posted_slot: 1,
        };

        let mut data = PriceUpdateV2::DISCRIMINATOR.to_vec();
        update.serialize(&mut data).unwrap();

        self.svm
            .set_account(
                key,
                SvmAccount {
                    lamports: 1_000_000_000,
                    data,
                    owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    pub fn crank_ix(
        &self,
        index: u16,
        price: Pubkey,
        secondary: Option<Pubkey>,
        quote_conv: Option<Pubkey>,
        keeper: Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::CrankMarketPrice {
                keeper,
                protocol: self.protocol,
                market: Self::market_pda(index),
                price_update: price,
                secondary_price_update: secondary,
                quote_conversion_price_update: quote_conv,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::CrankMarketPrice {}.data(),
        }
    }

    /// Crank a direct, USD-quoted market.
    pub fn crank(&mut self, index: u16, price: Pubkey) -> TestResult {
        let keeper = self.admin.insecure_clone();
        let ix = self.crank_ix(index, price, None, None, keeper.pubkey());
        self.send(ix, &[&keeper])
    }

    // --- reading state -----------------------------------------------------------------

    pub fn read<T: AccountDeserialize>(&self, key: &Pubkey) -> T {
        let acct = self
            .svm
            .get_account(key)
            .unwrap_or_else(|| panic!("account {key} does not exist"));
        T::try_deserialize(&mut acct.data.as_slice())
            .unwrap_or_else(|e| panic!("could not deserialize {key}: {e}"))
    }

    pub fn protocol_state(&self) -> Protocol {
        self.read(&self.protocol)
    }
    pub fn market_state(&self, index: u16) -> Market {
        self.read(&Self::market_pda(index))
    }
    pub fn user_state(&self, user: &User) -> UserAccount {
        self.read(&user.account)
    }
    pub fn lp_state(&self) -> LpPool {
        self.read(&self.lp_pool)
    }
    pub fn insurance_state(&self) -> InsuranceFund {
        self.read(&self.insurance_fund)
    }

    pub fn token_balance(&self, key: &Pubkey) -> u64 {
        let acct = self.svm.get_account(key).expect("token account missing");
        spl_token::state::Account::unpack(&acct.data)
            .expect("not a token account")
            .amount
    }

    // --- invariants --------------------------------------------------------------------

    /// Invariant I1 (`ARCHITECTURE.md` § 12.3):
    /// `CollateralVault.amount == Σ(UserAccount.free) + Σ(Position.collateral)`.
    ///
    /// Checked three ways rather than two. The vault's token balance, the sum over every
    /// user account, and the running total the program maintains on `Protocol` must all
    /// agree. Comparing only two of the three would let a bug that corrupts both in the same
    /// direction pass unnoticed.
    ///
    /// Phase 2 has no positions, so the position term is zero. Phase 3 adds it here.
    pub fn assert_i1(&self) {
        let vault = self.token_balance(&self.collateral_vault);
        let protocol = self.protocol_state();

        let summed: u64 = self
            .users
            .iter()
            .map(|k| self.read::<UserAccount>(k).free_collateral)
            .sum();

        assert_eq!(
            vault, summed,
            "I1 broken: vault holds {vault} but user accounts sum to {summed}"
        );
        assert_eq!(
            vault, protocol.total_user_collateral,
            "I1 broken: vault holds {vault} but Protocol.total_user_collateral is {}",
            protocol.total_user_collateral
        );
        assert_eq!(
            protocol.total_deposits - protocol.total_withdrawals,
            vault,
            "I7 broken: deposits {} minus withdrawals {} does not equal the vault balance {vault}",
            protocol.total_deposits,
            protocol.total_withdrawals,
        );
    }

    // --- raw account writing -----------------------------------------------------------

    pub fn write_mint_with_decimals(&mut self, key: Pubkey, decimals: u8) {
        let mint = spl_token::state::Mint {
            mint_authority: COption::None,
            supply: u64::MAX / 2,
            decimals,
            is_initialized: true,
            freeze_authority: COption::None,
        };
        let mut data = vec![0u8; spl_token::state::Mint::LEN];
        spl_token::state::Mint::pack(mint, &mut data).unwrap();
        self.svm
            .set_account(
                key,
                SvmAccount {
                    lamports: 1_000_000_000,
                    data,
                    owner: spl_token::ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    pub fn write_token_account(&mut self, key: Pubkey, mint: Pubkey, owner: Pubkey, amount: u64) {
        let acct = spl_token::state::Account {
            mint,
            owner,
            amount,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut data = vec![0u8; spl_token::state::Account::LEN];
        spl_token::state::Account::pack(acct, &mut data).unwrap();
        self.svm
            .set_account(
                key,
                SvmAccount {
                    lamports: 1_000_000_000,
                    data,
                    owner: spl_token::ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }
}

use anchor_lang::solana_program::program_option::COption;
use anchor_lang::solana_program::program_pack::Pack;

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &solfx_core::ID).0
}

fn so_path() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is programs/solfx-core; the workspace root is two levels up.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/solfx_core.so")
}

/// Assert that a failure names a specific `SolfxError`.
///
/// Matching on the message rather than the code because Anchor renders the `#[msg]` string
/// into the transaction log, and a test that only asserts "some error" would pass even if
/// the program started rejecting for entirely the wrong reason.
pub fn assert_err_contains(result: TestResult, needle: &str) {
    match result {
        Ok(()) => panic!("expected failure containing {needle:?}, but the call succeeded"),
        Err(log) => assert!(
            log.contains(needle),
            "expected failure containing {needle:?}, got:\n{log}"
        ),
    }
}
