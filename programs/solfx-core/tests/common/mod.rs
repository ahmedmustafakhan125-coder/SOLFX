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
    Direction, FeedKind, InsuranceFund, LpPool, Market, MarketStatus, Position, PriceSource,
    Protocol, QuoteConversionKind, TriggerKind, UserAccount,
};

/// The Pyth pull-oracle receiver. Taken from the SDK rather than written out, so the tests
/// and the program can never disagree about which program owns a price account.
pub const PYTH_RECEIVER_ID: Pubkey = pyth_solana_receiver_sdk::ID;

/// Fixed wall-clock for the suite. Absolute value is irrelevant; what matters is that tests
/// can move it relative to a price's `publish_time`.
pub const T0: i64 = 1_800_000_000;

pub const ONE_USDC: u64 = 1_000_000;

/// One standard lot: 100,000 units of base currency at `BASE_PRECISION`.
pub const ONE_LOT: u64 = 100_000 * 1_000_000_000;

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
                // Base units at BASE_PRECISION, not USDC: 1 lot = 1e14.
                //
                // The floor is set very low on purpose. A base-unit minimum is
                // asset-specific — a sane minimum for EUR/USD is ~40,000x too large for
                // gold, because an ounce is worth ~3,700 euros — so the *money* floor is
                // `pricing::validate_notional` ($1 of notional), which applies uniformly.
                // This bound exists to stop dust sizes, not to set a minimum trade value.
                max_position_size: 100 * ONE_LOT,
                min_position_size: ONE_LOT / 1_000_000,

                max_staleness_seconds: 10,
                max_conf_bps: 15,
                // 3%, against a 15 bps trading ceiling — a factor of 200.
                //
                // That gap looks extreme until you size it against the event it exists for.
                // During the CHF depeg, quoted spreads went from ~2 pips to 500+; on a 1.20
                // price that is over 400 bps of uncertainty. A liquidation ceiling set at
                // "a few times normal" simply does not fire during a depeg, and a position
                // that cannot be liquidated keeps falling.
                //
                // This ceiling is a **tail-event parameter**. It is not there to judge price
                // quality — the trading ceiling does that — but to reject a feed so broken
                // its number is meaningless.
                liquidation_max_conf_bps: 300,
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

    /// The **widest account shape** the protocol can produce: a synthetic pair (two oracle
    /// legs) that is also non-USD-quoted (a third leg, for conversion — correction C-3).
    ///
    /// Not a product anyone would list. It exists so the packet-size bound in
    /// `compute_budget.rs` is measured against the worst case rather than a typical one, with
    /// three *distinct* feed ids so the transaction cannot quietly shrink by deduplicating a
    /// repeated account key.
    pub fn widest() -> Self {
        let mut s = Self::base("WIDEST", FEED_EUR_USD);
        s.params.price_source = PriceSource::Synthetic { invert_quote: true };
        s.params.secondary_feed_id = FEED_GBP_USD;
        s.params.quote_conversion_feed = FEED_USD_INR;
        s.params.quote_conversion_kind = QuoteConversionKind::QuotePerUsd;
        s.params.max_leverage = 20;
        s.params.imr_bps = 500;
        s.params.mmr_bps = 250;
        s.params.liquidation_fee_bps = 100;
        s.params.max_conf_bps = 50;
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

/// A liquidity provider. Separate from [`User`] because the two hold different token
/// accounts and can never be the same role in a test.
pub struct LiquidityProvider {
    pub keypair: Keypair,
    pub token_account: Pubkey,
    pub lp_account: Pubkey,
}

impl LiquidityProvider {
    pub fn pubkey(&self) -> Pubkey {
        self.keypair.pubkey()
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
    positions: Vec<Pubkey>,
    /// Insurance-fund seeding. The only inflow the program does not record itself —
    /// trader collateral is on `Protocol` and LP capital is on `LpPool`.
    external_deposits: u64,
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

        // The referral programme, loaded alongside core so the Phase 6 suite can exercise
        // the real CPI rather than a stand-in. Optional: every earlier suite runs without it.
        if let Ok(ref_so) = std::fs::read(referral_so_path()) {
            svm.add_program(solfx_referral::ID, &ref_so).unwrap();
        }

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
            positions: Vec::new(),
            external_deposits: 0,
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
            lp_performance_fee_bps: 1_000, // § 8.1: 10% of LP profit above the mark
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

    /// `expected_current` is a compare-and-swap on the stored feed id, so callers must state
    /// what they believe the market currently points at.
    pub fn set_oracle_ix(
        &self,
        index: u16,
        expected_current: [u8; 32],
        new_feed_id: [u8; 32],
    ) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AdminMarket {
                admin: self.admin.pubkey(),
                protocol: self.protocol,
                market: Self::market_pda(index),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::SetMarketOracle {
                expected_current,
                new_feed_id,
            }
            .data(),
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

    /// Write a price stamped at the **current clock**.
    ///
    /// `PriceSpec::default()` carries `publish_time: T0`, so any test that advances the clock
    /// — every funding and carry test does — would otherwise post a price the staleness gate
    /// correctly rejects. Wanting a stale price is the unusual case, so it stays explicit via
    /// `published_at`.
    pub fn post_price_now(&mut self, feed_id: [u8; 32], spec: PriceSpec) -> Pubkey {
        let now = self.now;
        self.post_price(feed_id, spec.published_at(now))
    }

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
    /// All three price accounts for [`MarketSpec::widest`], posted at the current clock.
    pub fn post_widest_now(&mut self, spec: PriceSpec) -> (Pubkey, Pubkey, Pubkey) {
        let primary = self.post_price_now(FEED_EUR_USD, spec);
        let secondary = self.post_price_now(FEED_GBP_USD, spec);
        let conversion = self.post_price_now(FEED_USD_INR, spec);
        (primary, secondary, conversion)
    }

    /// Serialized size of a keeper transaction, built the way a keeper actually builds it.
    ///
    /// The compute-budget instructions are included because they are not optional in
    /// production — a liquidation sent without a priority-fee bid does not land during the
    /// congestion that made it necessary — and they cost bytes like anything else.
    pub fn measure_keeper_tx(&self, ix: Instruction, keeper: &Keypair) -> usize {
        let ixs = vec![
            solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(
                400_000,
            ),
            solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_price(
                50_000,
            ),
            ix,
        ];
        let message = solana_message::Message::new(&ixs, Some(&keeper.pubkey()));
        let signatures = message.header.num_required_signatures as usize;
        // Legacy wire format: a short-vec length byte, then 64 bytes per signature, then the
        // message. Every keeper transaction has one signer, so the length fits in one byte.
        1 + signatures * 64 + message.serialize().len()
    }

    pub fn crank(&mut self, index: u16, price: Pubkey) -> TestResult {
        let keeper = self.admin.insecure_clone();
        let ix = self.crank_ix(index, price, None, None, keeper.pubkey());
        self.send(ix, &[&keeper])
    }

    // --- reading state -----------------------------------------------------------------

    /// Read an account that may have been closed. `close_position` reclaims the rent, so a
    /// closed position deserialises as `None` rather than panicking the invariant sweep.
    pub fn try_read<T: AccountDeserialize>(&self, key: &Pubkey) -> Option<T> {
        let acct = self.svm.get_account(key)?;
        if acct.data.len() < 8 || acct.owner != solfx_core::ID {
            return None;
        }
        T::try_deserialize(&mut acct.data.as_slice()).ok()
    }

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

    /// Lamports held by an account. Used for the trigger-keeper tip, which is paid as rent
    /// rather than USDC.
    pub fn sol_balance(&self, key: &Pubkey) -> u64 {
        self.svm.get_account(key).map_or(0, |a| a.lamports)
    }

    pub fn token_balance(&self, key: &Pubkey) -> u64 {
        let acct = self.svm.get_account(key).expect("token account missing");
        spl_token::state::Account::unpack(&acct.data)
            .expect("not a token account")
            .amount
    }

    // --- positions ---------------------------------------------------------------------

    pub fn position_pda(user_account: &Pubkey, market_index: u16, nonce: u8) -> Pubkey {
        pda(&[
            POSITION_SEED,
            user_account.as_ref(),
            &market_index.to_le_bytes(),
            &[nonce],
        ])
    }

    /// The eight vault/pool accounts every position instruction carries.
    fn settlement_keys(&self) -> (Pubkey, Pubkey, Pubkey, Pubkey, Pubkey, Pubkey) {
        (
            self.collateral_vault,
            self.lp_pool,
            self.lp_vault,
            self.insurance_fund,
            self.insurance_vault,
            self.fee_vault,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_ix(
        &self,
        user: &User,
        market_index: u16,
        nonce: u8,
        direction: Direction,
        size_base: u64,
        collateral: u64,
        price_limit: i64,
        price: Pubkey,
        secondary: Option<Pubkey>,
        quote_conv: Option<Pubkey>,
    ) -> Instruction {
        let (collateral_vault, lp_pool, lp_vault, insurance_fund, insurance_vault, fee_vault) =
            self.settlement_keys();
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::OpenPosition {
                authority: user.pubkey(),
                protocol: self.protocol,
                user_account: user.account,
                market: Self::market_pda(market_index),
                position: Self::position_pda(&user.account, market_index, nonce),
                collateral_vault,
                lp_pool,
                lp_vault,
                insurance_fund,
                insurance_vault,
                fee_vault,
                price_update: price,
                secondary_price_update: secondary,
                quote_conversion_price_update: quote_conv,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::OpenPosition {
                market_index,
                nonce,
                direction,
                size_base,
                collateral,
                price_limit,
            }
            .data(),
        }
    }

    /// The `DecreasePosition` account set, shared by decrease, increase and both collateral
    /// adjustments — they touch exactly the same accounts.
    pub fn modify_metas(
        &self,
        user: &User,
        market_index: u16,
        nonce: u8,
        price: Pubkey,
        secondary: Option<Pubkey>,
        quote_conv: Option<Pubkey>,
    ) -> Vec<anchor_lang::solana_program::instruction::AccountMeta> {
        let (collateral_vault, lp_pool, lp_vault, insurance_fund, insurance_vault, fee_vault) =
            self.settlement_keys();
        solfx_core::accounts::DecreasePosition {
            authority: user.pubkey(),
            protocol: self.protocol,
            user_account: user.account,
            market: Self::market_pda(market_index),
            position: Self::position_pda(&user.account, market_index, nonce),
            collateral_vault,
            lp_pool,
            lp_vault,
            insurance_fund,
            insurance_vault,
            fee_vault,
            price_update: price,
            secondary_price_update: secondary,
            quote_conversion_price_update: quote_conv,
            token_program: spl_token::ID,
        }
        .to_account_metas(None)
    }

    pub fn close_metas(
        &self,
        user: &User,
        market_index: u16,
        nonce: u8,
        price: Pubkey,
        secondary: Option<Pubkey>,
        quote_conv: Option<Pubkey>,
    ) -> Vec<anchor_lang::solana_program::instruction::AccountMeta> {
        let (collateral_vault, lp_pool, lp_vault, insurance_fund, insurance_vault, fee_vault) =
            self.settlement_keys();
        solfx_core::accounts::ClosePosition {
            authority: user.pubkey(),
            protocol: self.protocol,
            user_account: user.account,
            market: Self::market_pda(market_index),
            position: Self::position_pda(&user.account, market_index, nonce),
            collateral_vault,
            lp_pool,
            lp_vault,
            insurance_fund,
            insurance_vault,
            fee_vault,
            price_update: price,
            secondary_price_update: secondary,
            quote_conversion_price_update: quote_conv,
            token_program: spl_token::ID,
        }
        .to_account_metas(None)
    }

    /// Open a position, registering it so the invariant sweep sees it.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        user: &User,
        market_index: u16,
        nonce: u8,
        direction: Direction,
        size_base: u64,
        collateral: u64,
        price_limit: i64,
        price: Pubkey,
    ) -> TestResult {
        let ix = self.open_ix(
            user,
            market_index,
            nonce,
            direction,
            size_base,
            collateral,
            price_limit,
            price,
            None,
            None,
        );
        let kp = user.keypair.insecure_clone();
        let key = Self::position_pda(&user.account, market_index, nonce);
        let result = self.send(ix, &[&kp]);
        if result.is_ok() && !self.positions.contains(&key) {
            self.positions.push(key);
        }
        result
    }

    pub fn close(
        &mut self,
        user: &User,
        market_index: u16,
        nonce: u8,
        price_limit: i64,
        price: Pubkey,
    ) -> TestResult {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: self.close_metas(user, market_index, nonce, price, None, None),
            data: solfx_core::instruction::ClosePosition { price_limit }.data(),
        };
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn decrease(
        &mut self,
        user: &User,
        market_index: u16,
        nonce: u8,
        size_delta: u64,
        price_limit: i64,
        price: Pubkey,
    ) -> TestResult {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: self.modify_metas(user, market_index, nonce, price, None, None),
            data: solfx_core::instruction::DecreasePosition {
                size_delta,
                price_limit,
            }
            .data(),
        };
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn increase(
        &mut self,
        user: &User,
        market_index: u16,
        nonce: u8,
        size_delta: u64,
        collateral_delta: u64,
        price_limit: i64,
        price: Pubkey,
    ) -> TestResult {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: self.modify_metas(user, market_index, nonce, price, None, None),
            data: solfx_core::instruction::IncreasePosition {
                size_delta,
                collateral_delta,
                price_limit,
            }
            .data(),
        };
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn add_margin(
        &mut self,
        user: &User,
        market_index: u16,
        nonce: u8,
        amount: u64,
        price: Pubkey,
    ) -> TestResult {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: self.modify_metas(user, market_index, nonce, price, None, None),
            data: solfx_core::instruction::AddPositionCollateral { amount }.data(),
        };
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn remove_margin(
        &mut self,
        user: &User,
        market_index: u16,
        nonce: u8,
        amount: u64,
        price: Pubkey,
    ) -> TestResult {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: self.modify_metas(user, market_index, nonce, price, None, None),
            data: solfx_core::instruction::RemovePositionCollateral { amount }.data(),
        };
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn position_state(&self, user: &User, market_index: u16, nonce: u8) -> Position {
        self.read(&Self::position_pda(&user.account, market_index, nonce))
    }

    pub fn position_exists(&self, user: &User, market_index: u16, nonce: u8) -> bool {
        self.try_read::<Position>(&Self::position_pda(&user.account, market_index, nonce))
            .is_some()
    }

    /// Register a position key with the invariant sweep without opening it here.
    pub fn track_position(&mut self, key: Pubkey) {
        if !self.positions.contains(&key) {
            self.positions.push(key);
        }
    }

    // --- liquidity -----------------------------------------------------------------------

    /// Seed the counterparty pool. Without it the vault cannot pay a winning trade, so only
    /// losing trades would be testable.
    pub fn seed_pool(&mut self, amount: u64) {
        let provider = Keypair::new();
        self.svm
            .airdrop(&provider.pubkey(), 100 * 1_000_000_000)
            .unwrap();

        let usdc_account = Pubkey::new_unique();
        let lp_account = Pubkey::new_unique();
        let mint = self.usdc_mint;
        let lp_mint = self.lp_mint;
        self.write_token_account(usdc_account, mint, provider.pubkey(), amount);
        self.write_token_account(lp_account, lp_mint, provider.pubkey(), 0);

        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AddLiquidity {
                provider: provider.pubkey(),
                protocol: self.protocol,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                lp_mint: self.lp_mint,
                provider_token_account: usdc_account,
                provider_lp_account: lp_account,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::AddLiquidity {
                amount,
                min_lp_out: 0,
            }
            .data(),
        };
        self.send(ix, &[&provider]).expect("add_liquidity failed");
    }

    // --- risk engine (Phase 4) -----------------------------------------------------------

    pub fn crank_funding_ix(&self, index: u16, keeper: Pubkey) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::CrankFunding {
                keeper,
                protocol: self.protocol,
                market: Self::market_pda(index),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::CrankFunding {}.data(),
        }
    }

    pub fn crank_funding(&mut self, index: u16) -> TestResult {
        let keeper = self.admin.insecure_clone();
        let ix = self.crank_funding_ix(index, keeper.pubkey());
        self.send(ix, &[&keeper])
    }

    pub fn crank_session_ix(
        &self,
        index: u16,
        keeper: Pubkey,
        price: Option<Pubkey>,
    ) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::CrankMarketSession {
                keeper,
                protocol: self.protocol,
                market: Self::market_pda(index),
                price_update: price,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::CrankMarketSession {}.data(),
        }
    }

    /// Crank the session. `price: None` models a feed that has stopped publishing entirely —
    /// which is what a closed market looks like from on chain.
    pub fn crank_session(&mut self, index: u16, price: Option<Pubkey>) -> TestResult {
        let keeper = self.admin.insecure_clone();
        let ix = self.crank_session_ix(index, keeper.pubkey(), price);
        self.send(ix, &[&keeper])
    }

    /// A liquidator with a funded USDC account, ready to be paid.
    pub fn new_liquidator(&mut self) -> (Keypair, Pubkey) {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 100 * 1_000_000_000).unwrap();
        let token = Pubkey::new_unique();
        let mint = self.usdc_mint;
        self.write_token_account(token, mint, kp.pubkey(), 0);
        (kp, token)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn liquidate_ix(
        &self,
        owner: &User,
        market_index: u16,
        nonce: u8,
        liquidator: Pubkey,
        liquidator_token: Pubkey,
        price: Pubkey,
        secondary: Option<Pubkey>,
        quote_conv: Option<Pubkey>,
    ) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::LiquidatePosition {
                liquidator,
                protocol: self.protocol,
                user_account: owner.account,
                market: Self::market_pda(market_index),
                position: Self::position_pda(&owner.account, market_index, nonce),
                rent_destination: owner.pubkey(),
                liquidator_token_account: liquidator_token,
                collateral_vault: self.collateral_vault,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                insurance_fund: self.insurance_fund,
                insurance_vault: self.insurance_vault,
                fee_vault: self.fee_vault,
                price_update: price,
                secondary_price_update: secondary,
                quote_conversion_price_update: quote_conv,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::LiquidatePosition {}.data(),
        }
    }

    pub fn liquidate(
        &mut self,
        owner: &User,
        market_index: u16,
        nonce: u8,
        liquidator: &Keypair,
        liquidator_token: Pubkey,
        price: Pubkey,
    ) -> TestResult {
        let ix = self.liquidate_ix(
            owner,
            market_index,
            nonce,
            liquidator.pubkey(),
            liquidator_token,
            price,
            None,
            None,
        );
        let kp = liquidator.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn adl_ix(
        &self,
        owner: &User,
        market_index: u16,
        nonce: u8,
        keeper: Pubkey,
        price: Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AutoDeleverage {
                keeper,
                protocol: self.protocol,
                user_account: owner.account,
                market: Self::market_pda(market_index),
                position: Self::position_pda(&owner.account, market_index, nonce),
                rent_destination: owner.pubkey(),
                collateral_vault: self.collateral_vault,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                insurance_fund: self.insurance_fund,
                insurance_vault: self.insurance_vault,
                fee_vault: self.fee_vault,
                price_update: price,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::AutoDeleverage {}.data(),
        }
    }

    pub fn auto_deleverage(
        &mut self,
        owner: &User,
        market_index: u16,
        nonce: u8,
        price: Pubkey,
    ) -> TestResult {
        let keeper = self.admin.insecure_clone();
        let ix = self.adl_ix(owner, market_index, nonce, keeper.pubkey(), price);
        self.send(ix, &[&keeper])
    }

    /// Fund the insurance fund. § 6.9: never launch with this empty — the first gap event
    /// hits LPs directly and they do not come back.
    pub fn seed_insurance(&mut self, amount: u64) {
        let payer = Keypair::new();
        self.svm
            .airdrop(&payer.pubkey(), 100 * 1_000_000_000)
            .unwrap();
        let token = Pubkey::new_unique();
        let mint = self.usdc_mint;
        self.write_token_account(token, mint, payer.pubkey(), amount);

        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::DepositInsuranceFund {
                payer: payer.pubkey(),
                protocol: self.protocol,
                insurance_fund: self.insurance_fund,
                insurance_vault: self.insurance_vault,
                payer_token_account: token,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::DepositInsuranceFund { amount }.data(),
        };
        self.send(ix, &[&payer]).expect("insurance deposit failed");
        self.external_deposits += amount;
    }

    /// Rewrite a market's risk and rate parameters in place.
    ///
    /// Some Phase 4 fields have no admin instruction yet (funding `k`, the two interest
    /// rates), so tests set them directly. That is a harness affordance, not a protocol
    /// hole — an admin instruction for them belongs with the frontend that would use it.
    pub fn patch_market(&mut self, index: u16, f: impl FnOnce(&mut Market)) {
        let mut m = self.market_state(index);
        f(&mut m);
        let key = Self::market_pda(index);
        let mut data = Market::DISCRIMINATOR.to_vec();
        m.serialize(&mut data).unwrap();
        let existing = self.svm.get_account(&key).unwrap();
        self.svm
            .set_account(
                key,
                SvmAccount {
                    lamports: existing.lamports,
                    data,
                    owner: existing.owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    /// Sum of every vault the protocol controls. The figure conservation is measured on.
    pub fn total_vault_balance(&self) -> u64 {
        self.token_balance(&self.collateral_vault)
            + self.token_balance(&self.lp_vault)
            + self.token_balance(&self.insurance_vault)
            + self.token_balance(&self.fee_vault)
    }

    /// Crank a market's recorded price. Named to distinguish it from `crank_session`.
    pub fn crank_price(&mut self, index: u16, price: Pubkey) -> TestResult {
        self.crank(index, price)
    }

    // --- liquidity exit (Phase 5) --------------------------------------------------------

    pub fn lp_withdraw_request_pda(authority: &Pubkey) -> Pubkey {
        pda(&[LP_WITHDRAW_SEED, authority.as_ref()])
    }

    pub fn request_remove_ix(&self, lp: &LiquidityProvider, shares: u64) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::RequestRemoveLiquidity {
                provider: lp.pubkey(),
                protocol: self.protocol,
                lp_pool: self.lp_pool,
                withdraw_request: Self::lp_withdraw_request_pda(&lp.pubkey()),
                provider_lp_account: lp.lp_account,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::RequestRemoveLiquidity { lp_amount: shares }.data(),
        }
    }

    pub fn request_remove(&mut self, lp: &LiquidityProvider, shares: u64) -> TestResult {
        let ix = self.request_remove_ix(lp, shares);
        let kp = lp.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn cancel_remove_ix(&self, lp: &LiquidityProvider) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::CancelRemoveLiquidity {
                provider: lp.pubkey(),
                lp_pool: self.lp_pool,
                withdraw_request: Self::lp_withdraw_request_pda(&lp.pubkey()),
                authority: lp.pubkey(),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::CancelRemoveLiquidity {}.data(),
        }
    }

    pub fn cancel_remove(&mut self, lp: &LiquidityProvider) -> TestResult {
        let ix = self.cancel_remove_ix(lp);
        let kp = lp.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn remove_liquidity_ix(&self, lp: &LiquidityProvider, min_out: u64) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::RemoveLiquidity {
                provider: lp.pubkey(),
                protocol: self.protocol,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                lp_mint: self.lp_mint,
                fee_vault: self.fee_vault,
                withdraw_request: Self::lp_withdraw_request_pda(&lp.pubkey()),
                provider_token_account: lp.token_account,
                provider_lp_account: lp.lp_account,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::RemoveLiquidity {
                min_usdc_out: min_out,
            }
            .data(),
        }
    }

    pub fn remove_liquidity(&mut self, lp: &LiquidityProvider, min_out: u64) -> TestResult {
        let ix = self.remove_liquidity_ix(lp, min_out);
        let kp = lp.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    /// A liquidity provider with their own USDC and `slpUSD` accounts.
    ///
    /// Distinct from [`Env::seed_pool`], which deposits anonymously to make the pool solvent
    /// for trading tests. This one can be tracked, redeemed and measured.
    pub fn new_lp(&mut self, usdc: u64) -> LiquidityProvider {
        let keypair = Keypair::new();
        let authority = keypair.pubkey();
        self.svm.airdrop(&authority, 100 * 1_000_000_000).unwrap();

        let token_account = Pubkey::new_unique();
        let mint = self.usdc_mint;
        self.write_token_account(token_account, mint, authority, usdc);

        let lp_account = Pubkey::new_unique();
        let lp_mint = self.lp_mint;
        self.write_token_account(lp_account, lp_mint, authority, 0);

        LiquidityProvider {
            keypair,
            token_account,
            lp_account,
        }
    }

    /// Deposit and receive shares, tracking the inflow for invariant I7.
    pub fn lp_deposit(&mut self, lp: &LiquidityProvider, amount: u64) -> TestResult {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AddLiquidity {
                provider: lp.pubkey(),
                protocol: self.protocol,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                lp_mint: self.lp_mint,
                provider_token_account: lp.token_account,
                provider_lp_account: lp.lp_account,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::AddLiquidity {
                amount,
                min_lp_out: 0,
            }
            .data(),
        };
        let kp = lp.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn lp_deposit_ix(&self, lp: &LiquidityProvider, amount: u64) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AddLiquidity {
                provider: lp.pubkey(),
                protocol: self.protocol,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                lp_mint: self.lp_mint,
                provider_token_account: lp.token_account,
                provider_lp_account: lp.lp_account,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::AddLiquidity {
                amount,
                min_lp_out: 0,
            }
            .data(),
        }
    }

    /// Drop USDC straight into the fee vault.
    ///
    /// Only for measuring the sweep — there is no instruction that credits the treasury
    /// directly, and generating the fee through a trade would measure the trade instead.
    pub fn seed_fee_vault(&mut self, amount: u64) {
        let current = self.token_balance(&self.fee_vault);
        let key = self.fee_vault;
        let mint = self.usdc_mint;
        let protocol = self.protocol;
        self.write_token_account(key, mint, protocol, current + amount);
        self.external_deposits += amount;
    }

    pub fn lp_shares(&self, lp: &LiquidityProvider) -> u64 {
        self.token_balance(&lp.lp_account)
    }

    pub fn nav_per_share(&self) -> u64 {
        let pool = self.lp_state();
        solfx_math::lp::nav_per_share(pool.aum, pool.lp_token_supply).unwrap()
    }

    pub fn withdraw_treasury_ix(&self, destination: Pubkey, amount: u64) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::WithdrawTreasuryFees {
                admin: self.admin.pubkey(),
                protocol: self.protocol,
                fee_vault: self.fee_vault,
                destination,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::WithdrawTreasuryFees { amount }.data(),
        }
    }

    /// Rewrite protocol config in place.
    ///
    /// The § 7.2 and § 7.4 breaker thresholds have no admin instruction yet — they belong
    /// with the frontend that would tune them (Phase 8) — so tests set them directly.
    pub fn patch_protocol(&mut self, f: impl FnOnce(&mut Protocol)) {
        let mut p = self.protocol_state();
        f(&mut p);
        let key = self.protocol;
        let mut data = Protocol::DISCRIMINATOR.to_vec();
        p.serialize(&mut data).unwrap();
        let existing = self.svm.get_account(&key).unwrap();
        self.svm
            .set_account(
                key,
                SvmAccount {
                    lamports: existing.lamports,
                    data,
                    owner: existing.owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    // --- referral programme (Phase 6) ----------------------------------------------------

    pub fn referral_config_pda() -> Pubkey {
        Pubkey::find_program_address(&[solfx_referral::CONFIG_SEED], &solfx_referral::ID).0
    }

    pub fn ib_pda(authority: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[solfx_referral::IB_SEED, authority.as_ref()],
            &solfx_referral::ID,
        )
        .0
    }

    pub fn link_pda(ib: &Pubkey, user_account: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[
                solfx_referral::LINK_SEED,
                ib.as_ref(),
                user_account.as_ref(),
            ],
            &solfx_referral::ID,
        )
        .0
    }

    /// Stand up the referral programme and register it with core, in the two steps the
    /// design requires: the referral admin creates the config, then the *core* admin decides
    /// to trust its PDA.
    pub fn init_referral(&mut self, override_bps: u16) {
        let admin = self.admin.insecure_clone();
        let ix = Instruction {
            program_id: solfx_referral::ID,
            accounts: solfx_referral::accounts::InitializeReferral {
                admin: admin.pubkey(),
                config: Self::referral_config_pda(),
                protocol: self.protocol,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_referral::instruction::InitializeReferral { override_bps }.data(),
        };
        self.send(ix, &[&admin])
            .expect("initialize_referral failed");

        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AdminOnly {
                admin: admin.pubkey(),
                protocol: self.protocol,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::SetReferralAuthority {
                authority: Self::referral_config_pda(),
            }
            .data(),
        };
        self.send(ix, &[&admin])
            .expect("set_referral_authority failed");
    }

    /// Register an introducing broker, optionally under a parent.
    pub fn register_ib(&mut self, parent: Pubkey) -> Keypair {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 100 * 1_000_000_000).unwrap();
        let ix = Instruction {
            program_id: solfx_referral::ID,
            accounts: solfx_referral::accounts::RegisterIb {
                authority: kp.pubkey(),
                config: Self::referral_config_pda(),
                ib_account: Self::ib_pda(&kp.pubkey()),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_referral::instruction::RegisterIb { parent }.data(),
        };
        self.send(ix, &[&kp]).expect("register_ib failed");
        kp
    }

    pub fn sync_trader_ix(
        &self,
        ib_authority: &Pubkey,
        parent_authority: Option<&Pubkey>,
        user: &User,
        payer: Pubkey,
    ) -> Instruction {
        let ib = Self::ib_pda(ib_authority);
        Instruction {
            program_id: solfx_referral::ID,
            accounts: solfx_referral::accounts::SyncTrader {
                payer,
                config: Self::referral_config_pda(),
                ib_account: ib,
                parent_ib: parent_authority.map(Self::ib_pda),
                user_account: user.account,
                link: Self::link_pda(&ib, &user.account),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_referral::instruction::SyncTrader {}.data(),
        }
    }

    /// Anyone may sync; the harness uses the protocol admin as a convenient stranger.
    pub fn sync_trader(
        &mut self,
        ib_authority: &Pubkey,
        parent_authority: Option<&Pubkey>,
        user: &User,
    ) -> TestResult {
        let payer = self.admin.insecure_clone();
        let ix = self.sync_trader_ix(ib_authority, parent_authority, user, payer.pubkey());
        self.send(ix, &[&payer])
    }

    pub fn claim_rebate_ix(&self, ib: &Keypair, destination: Pubkey) -> Instruction {
        Instruction {
            program_id: solfx_referral::ID,
            accounts: solfx_referral::accounts::Claim {
                authority: ib.pubkey(),
                config: Self::referral_config_pda(),
                ib_account: Self::ib_pda(&ib.pubkey()),
                protocol: self.protocol,
                fee_vault: self.fee_vault,
                destination,
                solfx_core: solfx_core::ID,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_referral::instruction::Claim {}.data(),
        }
    }

    pub fn claim_rebate(&mut self, ib: &Keypair, destination: Pubkey) -> TestResult {
        let ix = self.claim_rebate_ix(ib, destination);
        let kp = ib.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn ib_state(&self, authority: &Pubkey) -> solfx_referral::IbAccount {
        self.read(&Self::ib_pda(authority))
    }

    pub fn referral_config(&self) -> solfx_referral::ReferralConfig {
        self.read(&Self::referral_config_pda())
    }

    /// A USDC account an IB can be paid into.
    pub fn new_token_account_for(&mut self, owner: Pubkey) -> Pubkey {
        let key = Pubkey::new_unique();
        let mint = self.usdc_mint;
        self.write_token_account(key, mint, owner, 0);
        key
    }

    /// Rewrite a trader's account in place.
    ///
    /// Used to fast-forward `lifetime_volume` to a tier boundary: reaching $5M honestly
    /// would take ~230 round trips, which measures LiteSVM rather than the tier logic.
    pub fn patch_user(&mut self, user: &User, f: impl FnOnce(&mut UserAccount)) {
        let mut acct = self.user_state(user);
        f(&mut acct);
        self.overwrite_account(user.account, UserAccount::DISCRIMINATOR, &acct);
    }

    /// Rewrite an IB account in place — used to prove core's pool cap holds even when the
    /// referral programme asks for more than it should.
    pub fn patch_ib(&mut self, authority: &Pubkey, f: impl FnOnce(&mut solfx_referral::IbAccount)) {
        let key = Self::ib_pda(authority);
        let mut acct: solfx_referral::IbAccount = self.read(&key);
        f(&mut acct);
        self.overwrite_account(key, solfx_referral::IbAccount::DISCRIMINATOR, &acct);
    }

    fn overwrite_account<T: AnchorSerialize>(&mut self, key: Pubkey, disc: &[u8], value: &T) {
        let mut data = disc.to_vec();
        value.serialize(&mut data).unwrap();
        let existing = self.svm.get_account(&key).unwrap();
        self.svm
            .set_account(
                key,
                SvmAccount {
                    lamports: existing.lamports,
                    data,
                    owner: existing.owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    // --- trigger orders (Phase 7) --------------------------------------------------------

    pub fn trigger_pda(position: &Pubkey, order_id: u8) -> Pubkey {
        pda(&[TRIGGER_SEED, position.as_ref(), &[order_id]])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn place_trigger_ix(
        &self,
        user: &User,
        market_index: u16,
        nonce: u8,
        order_id: u8,
        kind: TriggerKind,
        trigger_price: i64,
        size_base: u64,
        price: Pubkey,
    ) -> Instruction {
        let position = Self::position_pda(&user.account, market_index, nonce);
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::PlaceTriggerOrder {
                authority: user.pubkey(),
                protocol: self.protocol,
                user_account: user.account,
                market: Self::market_pda(market_index),
                position,
                trigger_order: Self::trigger_pda(&position, order_id),
                price_update: price,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::PlaceTriggerOrder {
                order_id,
                kind,
                trigger_price,
                size_base,
            }
            .data(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn place_trigger(
        &mut self,
        user: &User,
        market_index: u16,
        nonce: u8,
        order_id: u8,
        kind: TriggerKind,
        trigger_price: i64,
        size_base: u64,
        price: Pubkey,
    ) -> TestResult {
        let ix = self.place_trigger_ix(
            user,
            market_index,
            nonce,
            order_id,
            kind,
            trigger_price,
            size_base,
            price,
        );
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn cancel_trigger_ix(&self, user: &User, position: Pubkey, order_id: u8) -> Instruction {
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::CancelTriggerOrder {
                authority: user.pubkey(),
                trigger_order: Self::trigger_pda(&position, order_id),
            }
            .to_account_metas(None),
            data: solfx_core::instruction::CancelTriggerOrder {}.data(),
        }
    }

    pub fn cancel_trigger(&mut self, user: &User, position: Pubkey, order_id: u8) -> TestResult {
        let ix = self.cancel_trigger_ix(user, position, order_id);
        let kp = user.keypair.insecure_clone();
        self.send(ix, &[&kp])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute_trigger_ix(
        &self,
        user: &User,
        market_index: u16,
        nonce: u8,
        order_id: u8,
        keeper: Pubkey,
        price: Pubkey,
    ) -> Instruction {
        let position = Self::position_pda(&user.account, market_index, nonce);
        let (collateral_vault, lp_pool, lp_vault, insurance_fund, insurance_vault, fee_vault) =
            self.settlement_keys();
        Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::ExecuteTriggerOrder {
                keeper,
                protocol: self.protocol,
                user_account: user.account,
                market: Self::market_pda(market_index),
                position,
                trigger_order: Self::trigger_pda(&position, order_id),
                collateral_vault,
                lp_pool,
                lp_vault,
                insurance_fund,
                insurance_vault,
                fee_vault,
                price_update: price,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::ExecuteTriggerOrder {}.data(),
        }
    }

    /// Fire a trigger as a third party — the case that matters, since a trader's stop must
    /// not depend on the operator's bot being up.
    pub fn execute_trigger_as(
        &mut self,
        keeper: &Keypair,
        user: &User,
        market_index: u16,
        nonce: u8,
        order_id: u8,
        price: Pubkey,
    ) -> TestResult {
        let ix =
            self.execute_trigger_ix(user, market_index, nonce, order_id, keeper.pubkey(), price);
        let kp = keeper.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn trigger_state(
        &self,
        position: &Pubkey,
        order_id: u8,
    ) -> solfx_core::state::TriggerOrder {
        self.read(&Self::trigger_pda(position, order_id))
    }

    pub fn trigger_exists(&self, position: &Pubkey, order_id: u8) -> bool {
        self.svm
            .get_account(&Self::trigger_pda(position, order_id))
            .is_some_and(|a| !a.data.is_empty())
    }

    // --- invariants (§ 12.3) ------------------------------------------------------------

    /// **I1**: `CollateralVault.amount == Σ(UserAccount.free) + Σ(Position.collateral)`.
    ///
    /// Checked three ways rather than two. The vault's token balance, the sum over every
    /// account, and the running total the program maintains on `Protocol` must all agree.
    /// Comparing only two of the three would let a bug that corrupts both in the same
    /// direction pass unnoticed — and a settlement bug corrupts them in the same direction
    /// by construction, because one function writes both.
    pub fn assert_i1(&self) {
        let vault = self.token_balance(&self.collateral_vault);
        let protocol = self.protocol_state();

        let free: u64 = self
            .users
            .iter()
            .map(|k| self.read::<UserAccount>(k).free_collateral)
            .sum();
        let margin: u64 = self
            .positions
            .iter()
            .filter_map(|k| self.try_read::<Position>(k))
            .map(|p| p.collateral)
            .sum();
        let summed = free + margin;

        assert_eq!(
            vault, summed,
            "I1 broken: vault holds {vault}, accounts sum to {summed} \
             (free {free} + position margin {margin})"
        );
        assert_eq!(
            vault, protocol.total_user_collateral,
            "I1 broken: vault holds {vault} but Protocol.total_user_collateral is {}",
            protocol.total_user_collateral
        );
    }

    /// **I2**: `LpVault.amount == LpPool.aum`.
    ///
    /// The pool's accounting and its tokens must never diverge. A settlement that moved one
    /// without the other would show up here and nowhere else.
    pub fn assert_i2(&self) {
        let vault = self.token_balance(&self.lp_vault);
        let aum = self.lp_state().aum;
        assert_eq!(vault, aum, "I2 broken: lp_vault {vault} vs aum {aum}");
    }

    /// **I4**: `market.oi_long == Σ(long positions' entry notional)`, same for short, and the
    /// base-unit counters match the open sizes exactly.
    ///
    /// The quote-unit counters are compared with a small tolerance because
    /// `remove_open_interest` saturates rather than failing a close over a rounding unit
    /// (see its doc comment). The **base**-unit counters are compared exactly, because those
    /// carry no price and drive funding — they have to balance to the unit.
    pub fn assert_i4(&self, market_index: u16) {
        let m = self.market_state(market_index);
        let mut long_notional = 0u64;
        let mut short_notional = 0u64;
        let mut long_base = 0i128;
        let mut short_base = 0i128;

        for key in &self.positions {
            let Some(p) = self.try_read::<Position>(key) else {
                continue;
            };
            if p.market_index != market_index || p.size_base == 0 {
                continue;
            }
            match p.direction {
                Direction::Long => {
                    long_notional += p.entry_notional;
                    long_base += i128::from(p.size_base);
                }
                Direction::Short => {
                    short_notional += p.entry_notional;
                    short_base += i128::from(p.size_base);
                }
            }
        }

        assert_eq!(
            m.base_oi_long, long_base,
            "I4 broken: market {market_index} base_oi_long {} vs positions {long_base}",
            m.base_oi_long
        );
        assert_eq!(
            m.base_oi_short, short_base,
            "I4 broken: market {market_index} base_oi_short {} vs positions {short_base}",
            m.base_oi_short
        );
        assert!(
            m.oi_long.abs_diff(long_notional) <= 2,
            "I4 broken: market {market_index} oi_long {} vs positions {long_notional}",
            m.oi_long
        );
        assert!(
            m.oi_short.abs_diff(short_notional) <= 2,
            "I4 broken: market {market_index} oi_short {} vs positions {short_notional}",
            m.oi_short
        );
    }

    /// **I5**: no position holds size with zero collateral.
    ///
    /// Such a position would be unliquidatable in the sense that matters — there is nothing
    /// to seize — while still contributing risk to the book.
    pub fn assert_i5(&self) {
        for key in &self.positions {
            let Some(p) = self.try_read::<Position>(key) else {
                continue;
            };
            if p.size_base > 0 {
                assert!(
                    p.collateral > 0,
                    "I5 broken: position {key} has size {} and zero collateral",
                    p.size_base
                );
            }
        }
    }

    /// **I6**: `InsuranceVault.amount == InsuranceFund.balance`.
    pub fn assert_i6(&self) {
        let vault = self.token_balance(&self.insurance_vault);
        let balance = self.insurance_state().balance;
        assert_eq!(
            vault, balance,
            "I6 broken: insurance_vault {vault} vs balance {balance}"
        );
    }

    /// **I8**: LP shares exist if and only if the pool holds assets.
    ///
    /// A supply with no assets behind it is worthless paper; assets with no supply are
    /// unclaimable. Either one means the accounting has come apart.
    pub fn assert_i8(&self) {
        let pool = self.lp_state();
        assert_eq!(
            pool.lp_token_supply > 0,
            pool.aum > 0,
            "I8 broken: supply {} against aum {}",
            pool.lp_token_supply,
            pool.aum
        );
    }

    /// **I7**: every USDC the program holds is accounted for.
    ///
    /// ```text
    /// Σ(vaults) == Σ(collateral deposits)   − Σ(collateral withdrawals)
    ///            + Σ(LP deposits)           − Σ(LP withdrawals)
    ///            + Σ(insurance deposits)
    ///                                       − Σ(liquidator rewards)
    ///                                       − Σ(treasury sweeps)
    ///                                       − Σ(referral claims)
    /// ```
    ///
    /// Nothing is created and nothing is destroyed. Trades only move value *between* the
    /// four vaults — the property `trader::flows::Flows::is_conservative` enforces before any
    /// token moves — so the only ways USDC crosses the protocol boundary are a trader
    /// withdrawing, someone funding the pool or the insurance fund, and a liquidator being
    /// paid. Every one of those has a term here.
    pub fn assert_i7(&self) {
        let total = self.token_balance(&self.collateral_vault)
            + self.token_balance(&self.lp_vault)
            + self.token_balance(&self.insurance_vault)
            + self.token_balance(&self.fee_vault);
        let p = self.protocol_state();
        let pool = self.lp_state();
        let expected = p.total_deposits + pool.total_deposited + self.external_deposits
            - p.total_withdrawals
            - pool.total_withdrawn
            - p.total_liquidator_paid
            - p.total_treasury_withdrawn
            - p.total_referral_claimed;
        assert_eq!(
            total,
            expected,
            "I7 broken: vaults hold {total}, expected {expected} \
             (trader in {} out {} | LP in {} out {} | seeded {} | liquidators {} | \
             treasury {} | referral {})",
            p.total_deposits,
            p.total_withdrawals,
            pool.total_deposited,
            pool.total_withdrawn,
            self.external_deposits,
            p.total_liquidator_paid,
            p.total_treasury_withdrawn,
            p.total_referral_claimed
        );
    }

    /// Every invariant Phase 3 can check, in one call. Cheap enough to run after each
    /// instruction, which is the point — an invariant checked only at the end of a scenario
    /// tells you something broke but not what.
    pub fn assert_invariants(&self) {
        self.assert_i1();
        self.assert_i2();
        self.assert_i5();
        self.assert_i6();
        self.assert_i7();
        self.assert_i8();
        for i in 0..self.protocol_state().num_markets {
            self.assert_i4(i);
        }
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

fn referral_so_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/solfx_referral.so")
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
