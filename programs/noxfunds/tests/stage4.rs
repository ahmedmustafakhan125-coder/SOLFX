//! Stage 4 — the track record, and the tier it earns.
//!
//! # What this stage is for
//!
//! Stages 1, 2 and 6 make a single mandate behave. This one makes a *trader* mean something
//! across mandates: what they have actually done, accumulated on chain, and the tier that
//! record earns them.
//!
//! The boundary arithmetic — every threshold, in both directions, and the 90%-win-rate trap —
//! is pinned by pure unit tests in `state.rs`, which need no validator and run in
//! microseconds. What is tested here is the part those cannot reach: that the *program* writes
//! the record correctly, that it takes its realised PnL from the venue rather than recomputing
//! it, and that the tier actually binds at funding.
//!
//! # The claim this file has to hold up
//!
//! **A drawdown cannot be hidden by refusing to close the losing position.** That is the
//! gaming vector that matters most, and it is the reason `observe_mandate_equity` writes to the
//! profile at all.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

#[path = "../../solfx-core/tests/common/mod.rs"]
mod common;
#[allow(dead_code)]
mod support;

use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use common::*;
use noxfunds::instructions::investor::MandateRules;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solfx_core::state::Direction;

const MARKET_0: u128 = 1;
const SPOT: i64 = 1_085_430_000;

// --- fixture -----------------------------------------------------------------------------
//
// The same shape as `stage6.rs`, split so that every helper hands back the `Instruction`
// instead of sending it. Measuring requires the instruction in hand.

fn nox_so_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/noxfunds.so")
}

fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[noxfunds::constants::CONFIG_SEED], &noxfunds::ID).0
}

fn mandate_pda(investor: &Pubkey, trader: &Pubkey, seq: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::MANDATE_SEED,
            investor.as_ref(),
            trader.as_ref(),
            &[seq],
        ],
        &noxfunds::ID,
    )
    .0
}

fn profile_pda(trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::TRADER_SEED, trader.as_ref()],
        &noxfunds::ID,
    )
    .0
}

/// A trader needs a record before anyone can fund them: `fund_mandate` reads the tier to bound
/// the mandate's size and the trader's concurrent count. Anyone may pay for it, which is why
/// the admin does here.
fn create_profile(env: &mut Env, payer: &solana_keypair::Keypair, trader: &Pubkey) {
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeTraderProfile {
            payer: payer.pubkey(),
            authority: *trader,
            profile: profile_pda(trader),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeTraderProfile {}.data(),
    };
    env.send(ix, &[payer]).expect("create the trader profile");
}

fn signer_pda(mandate: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::MANDATE_SIGNER_SEED, mandate.as_ref()],
        &noxfunds::ID,
    )
    .0
}

struct Nox {
    env: Env,
    investor: Keypair,
    trader: Keypair,
    mandate: Pubkey,
    signer: Pubkey,
    vault: Pubkey,
}

/// Destination accounts for the three payees.
struct Payees {
    investor: Pubkey,
    trader: Pubkey,
    treasury: Pubkey,
}

fn base_rules() -> MandateRules {
    MandateRules {
        max_trade_notional: 2_000 * ONE_USDC,
        max_total_notional: 6_000 * ONE_USDC,
        max_drawdown_bps: 1_000,
        max_daily_loss_bps: 1_000,
        max_risk_per_trade_bps: 100,
        max_stop_distance_bps: 200,
        max_concurrent_positions: 5,
        allowed_markets: MARKET_0,
        min_hold_slots: 0,
    }
}

/// $1,000 principal, $1,000 deposited, generous drawdown — a mandate that survives a round
/// trip.
fn roomy_mandate() -> Nox {
    mandate_of(1_000 * ONE_USDC)
}

/// The same, at whatever principal the test needs. Size is what the tier gate is about.
fn mandate_of(principal: u64) -> Nox {
    let rules = base_rules();

    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
    // Loaded the way a real deploy leaves it: the deployer holds the upgrade authority, which
    // is what `initialize_config` checks.
    let upgrade_authority = env.admin.pubkey();
    support::set_upgrade_authority(&mut env, Some(upgrade_authority));

    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let investor = Keypair::new();
    let trader = Keypair::new();
    for k in [&investor, &trader] {
        env.svm.airdrop(&k.pubkey(), 100 * 1_000_000_000).unwrap();
    }

    let admin = env.admin.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeConfig {
            program: noxfunds::ID,
            program_data: support::program_data_address(),
            admin: admin.pubkey(),
            config: config_pda(),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeConfig {
            guardian: env.guardian.pubkey(),
            treasury: admin.pubkey(),
            usdc_mint: env.usdc_mint,
        }
        .data(),
    };
    env.send(ix, &[&admin]).unwrap();

    create_profile(&mut env, &admin, &trader.pubkey());

    let mandate = mandate_pda(&investor.pubkey(), &trader.pubkey(), 0);
    let signer = signer_pda(&mandate);
    // The investor holds USDC, and `fund_mandate` moves the principal out of it into the
    // mandate's own vault. Before the funding fix these tests wrote the vault balance directly,
    // which is exactly why a mandate with a principal nobody deposited went unnoticed.
    let investor_token = Pubkey::new_unique();
    env.write_token_account(
        investor_token,
        env.usdc_mint,
        investor.pubkey(),
        support::INVESTOR_START,
    );
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundMandate {
            trader_profile: profile_pda(&trader.pubkey()),
            usdc_mint: env.usdc_mint,
            investor_token,
            mandate_vault: support::vault_pda(&mandate),
            token_program: spl_token::ID,
            investor: investor.pubkey(),
            config: config_pda(),
            trader: trader.pubkey(),
            mandate,
            mandate_signer: signer,
            solfx_user_account: Env::user_pda(&signer),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundMandate {
            seq: 0,
            principal,
            rules,
        }
        .data(),
    };
    let inv = investor.insecure_clone();
    env.send(ix, &[&inv]).unwrap();

    let mut nox = Nox {
        env,
        investor,
        trader,
        mandate,
        signer,
        vault: Pubkey::default(),
    };
    nox.fund_for_trading(1_000 * ONE_USDC);
    nox
}

impl Nox {
    fn fund_for_trading(&mut self, usdc: u64) {
        self.env.svm.airdrop(&self.signer, 1_000_000_000).unwrap();
        // Filled by `fund_mandate`; this only moves what is already there into SolFX.
        let vault = support::vault_pda(&self.mandate);
        self.vault = vault;
        let payer = self.investor.insecure_clone();
        let user_account = Env::user_pda(&self.signer);

        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::CreateSolfxAccount {
                payer: payer.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::CreateSolfxAccount {}.data(),
        };
        self.env.send(ix, &[&payer]).expect("create solfx account");
        self.env.track_user(user_account);

        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundSolfxCollateral {
                payer: payer.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                collateral_mint: self.env.usdc_mint,
                collateral_vault: self.env.collateral_vault,
                mandate_vault: vault,
                token_program: spl_token::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundSolfxCollateral { amount: usdc }.data(),
        };
        self.env.send(ix, &[&payer]).expect("fund collateral");
    }

    /// The trader's own transaction, so the trader is the fee payer.
    ///
    /// Stage 0 first measured this at 1,056 bytes rather than the derived 959 because the test
    /// used a fresh fee payer — a 25th account key and a second signature that no real client
    /// pays for. Keeping the signer and the payer the same keeps the measurement honest.
    fn open_ix(&mut self, nonce: u8, size_base: u64, stop: i64, collateral: u64) -> Instruction {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        let position = Env::position_pda(&user_account, 0, nonce);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedOpenPosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position,
                trigger_order: Env::trigger_pda(&position, nonce),
                collateral_vault: self.env.collateral_vault,
                lp_pool: self.env.lp_pool,
                lp_vault: self.env.lp_vault,
                insurance_fund: self.env.insurance_fund,
                insurance_vault: self.env.insurance_vault,
                fee_vault: self.env.fee_vault,
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedOpenPosition {
                market_index: 0,
                nonce,
                direction: Direction::Long,
                size_base,
                collateral,
                price_limit: i64::MAX,
                order_id: nonce,
                stop_loss_price: stop,
            }
            .data(),
        }
    }

    /// The price is a parameter because an earlier version posted the default inside the
    /// crank, silently overwriting the adverse price a drawdown test had just set.
    fn observe_ix_at(&mut self, spec: PriceSpec, nonces: &[u8]) -> (Instruction, Keypair) {
        let p = self.env.post_price_now(FEED_EUR_USD, spec);
        let observer = Keypair::new();
        self.env
            .svm
            .airdrop(&observer.pubkey(), 1_000_000_000)
            .unwrap();
        let user_account = Env::user_pda(&self.signer);

        let mut metas = noxfunds::accounts::ObserveMandateEquity {
            trader_profile: profile_pda(&self.trader.pubkey()),
            observer: observer.pubkey(),
            config: config_pda(),
            mandate: self.mandate,
            mandate_signer: self.signer,
            user_account,
        }
        .to_account_metas(None);
        for n in nonces {
            metas.push(AccountMeta::new_readonly(
                Env::position_pda(&user_account, 0, *n),
                false,
            ));
            metas.push(AccountMeta::new_readonly(Env::market_pda(0), false));
            metas.push(AccountMeta::new_readonly(p, false));
        }

        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: metas,
            data: noxfunds::instruction::ObserveMandateEquity {}.data(),
        };
        (ix, observer)
    }

    fn payees(&mut self) -> Payees {
        let mint = self.env.usdc_mint;
        let (investor, trader, treasury) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let inv = self.investor.pubkey();
        let tr = self.trader.pubkey();
        let admin = self.env.admin.pubkey();
        self.env.write_token_account(investor, mint, inv, 0);
        self.env.write_token_account(trader, mint, tr, 0);
        self.env.write_token_account(treasury, mint, admin, 0);
        Payees {
            investor,
            trader,
            treasury,
        }
    }

    fn request_settlement(&mut self) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::RequestSettlement {
                investor: self.investor.pubkey(),
                mandate: self.mandate,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::RequestSettlement {}.data(),
        };
        let k = self.investor.insecure_clone();
        self.env.send(ix, &[&k])
    }

    /// A stranger settles — the investor must never have to wait on anyone.
    fn claim_ix(&mut self, p: &Payees) -> (Instruction, Keypair) {
        let settler = Keypair::new();
        self.env
            .svm
            .airdrop(&settler.pubkey(), 1_000_000_000)
            .unwrap();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::ClaimSettlement {
                trader_profile: profile_pda(&self.trader.pubkey()),
                settler: settler.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: Env::user_pda(&self.signer),
                usdc_mint: self.env.usdc_mint,
                collateral_vault: self.env.collateral_vault,
                mandate_vault: self.vault,
                investor_token: p.investor,
                trader_token: p.trader,
                treasury_token: p.treasury,
                token_program: spl_token::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::ClaimSettlement {}.data(),
        };
        (ix, settler)
    }
}

// --- reading the record ---------------------------------------------------------------------

impl Nox {
    fn mandate_state(&self) -> noxfunds::state::Mandate {
        self.env.read(&self.mandate)
    }

    fn profile(&self) -> noxfunds::state::TraderProfile {
        self.env.read(&profile_pda(&self.trader.pubkey()))
    }

    fn open(&mut self, nonce: u8, stop: i64) -> TestResult {
        let ix = self.open_ix(nonce, ONE_LOT / 100, stop, 500 * ONE_USDC);
        let trader = self.trader.insecure_clone();
        let r = self.env.send(ix, &[&trader]);
        if r.is_ok() {
            self.env
                .track_position(Env::position_pda(&Env::user_pda(&self.signer), 0, nonce));
        }
        r
    }

    /// Close at a chosen price, which is how a test decides whether the trade won or lost.
    fn close_at(&mut self, spec: PriceSpec, nonce: u8) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, spec);
        let user_account = Env::user_pda(&self.signer);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedClosePosition {
                trader_profile: profile_pda(&self.trader.pubkey()),
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position: Env::position_pda(&user_account, 0, nonce),
                collateral_vault: self.env.collateral_vault,
                lp_pool: self.env.lp_pool,
                lp_vault: self.env.lp_vault,
                insurance_fund: self.env.insurance_fund,
                insurance_vault: self.env.insurance_vault,
                fee_vault: self.env.fee_vault,
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedClosePosition {
                market_index: 0,
                nonce,
                price_limit: 1,
            }
            .data(),
        };
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    fn observe_at(&mut self, spec: PriceSpec, nonces: &[u8]) -> TestResult {
        let (ix, observer) = self.observe_ix_at(spec, nonces);
        self.env.send(ix, &[&observer])
    }
}

/// 250 bps above spot — a real profit for a long.
fn favourable() -> PriceSpec {
    PriceSpec::at(111_250_000).conf(13_893)
}
/// 250 bps below spot — a real loss.
fn adverse() -> PriceSpec {
    PriceSpec::at(105_830_000).conf(13_893)
}

const STOP: i64 = SPOT - (SPOT * 30) / 10_000;

// --- what a closed trade puts on the record --------------------------------------------------

/// **The realised PnL is the venue's number, not this program's.**
///
/// `funded_close_position` reads `free_collateral` before the CPI and again after, and
/// subtracts the margin the position released. That difference is what `solfx-core` credited,
/// net of its own fees. This test does not check a formula — it checks that the figure on the
/// record is the same one the trader's balance actually moved by.
#[test]
fn a_winning_trade_records_the_venues_own_figure() {
    let mut nox = roomy_mandate();
    let user_account = Env::user_pda(&nox.signer);
    // Read *before the open*, not between the open and the close: the open fee comes out at the
    // open, and measuring after it would exclude the fee from both sides and prove nothing.
    let before = nox
        .env
        .read::<solfx_core::state::UserAccount>(&user_account)
        .free_collateral;

    nox.open(0, STOP).expect("open");
    nox.close_at(favourable(), 0).expect("close in profit");

    let after = nox
        .env
        .read::<solfx_core::state::UserAccount>(&user_account)
        .free_collateral;
    let moved = i128::from(after) - i128::from(before);

    let p = nox.profile();
    assert_eq!(p.trades, 1);
    assert_eq!(p.wins, 1, "a profitable close is a win");
    assert_eq!(p.losses, 0);
    assert_eq!(
        i128::from(p.gross_profit),
        moved,
        "the record must equal what the mandate's collateral actually did, to the unit"
    );
    assert_eq!(p.largest_win, p.gross_profit);
    assert_eq!(p.gross_loss, 0);
    nox.env.assert_invariants();
}

/// The other half. A loss lands in `gross_loss`, not as a smaller win.
#[test]
fn a_losing_trade_records_as_a_loss() {
    let mut nox = roomy_mandate();
    nox.open(0, STOP).expect("open");
    nox.close_at(adverse(), 0).expect("close at a loss");

    let p = nox.profile();
    assert_eq!((p.trades, p.wins, p.losses), (1, 0, 1));
    assert!(p.gross_loss > 0, "the loss is recorded");
    assert_eq!(p.gross_profit, 0);
    assert_eq!(p.largest_loss, p.gross_loss);
    // No wins at all, so the profit factor is zero rather than undefined.
    assert_eq!(p.profit_factor_bps(), 0);
    assert_eq!(p.win_rate_bps(), 0);
    nox.env.assert_invariants();
}

/// **The record equals what the mandate actually lost — both fees, not one.**
///
/// The recorded figure is the change in SolFX free collateral across the close, and the open fee
/// was taken when the position *opened*, so it is already out of the "before" reading. Measured
/// on devnet 2026-09-19: a mandate lost 0.202126 USDC on one round trip while its trader's
/// record showed 0.192117, short by exactly the 0.010009 open fee. This asserts the two agree
/// to the unit, which is the only version of a track record worth publishing.
#[test]
fn the_record_matches_the_collateral_the_mandate_actually_lost() {
    let mut nox = roomy_mandate();
    let user_account = Env::user_pda(&nox.signer);
    let before = nox
        .env
        .read::<solfx_core::state::UserAccount>(&user_account)
        .free_collateral;

    nox.open(0, STOP).expect("open");
    nox.close_at(PriceSpec::default(), 0).expect("close");

    let after = nox
        .env
        .read::<solfx_core::state::UserAccount>(&user_account)
        .free_collateral;
    let actually_lost = before - after;
    let p = nox.profile();
    assert_eq!(p.losses, 1);
    assert_eq!(
        p.gross_loss, actually_lost,
        "the record says {} but the mandate is down {}",
        p.gross_loss, actually_lost
    );
    nox.env.assert_invariants();
}

/// Even a round trip at the same price loses — the spread and the fees are real — so the
/// record shows a loss rather than a wash. This is `round_trip_at_the_same_price_always_loses`
/// from `solfx-math`, observed one layer up.
#[test]
fn a_flat_round_trip_still_registers_as_a_loss() {
    let mut nox = roomy_mandate();
    nox.open(0, STOP).expect("open");
    nox.close_at(PriceSpec::default(), 0).expect("close flat");

    let p = nox.profile();
    assert_eq!(p.losses, 1, "crossing the spread twice is not free");
    assert!(p.gross_loss > 0);
    nox.env.assert_invariants();
}

/// Hold time accumulates, because it is the anti-scalping statistic and a copy-trader cannot
/// fake it without actually holding.
#[test]
fn hold_time_accumulates_across_trades() {
    let mut nox = roomy_mandate();
    nox.open(0, STOP).expect("open");
    nox.env.advance_slot(2);
    nox.close_at(PriceSpec::default(), 0).expect("close");

    let p = nox.profile();
    assert!(p.total_hold_slots >= 2, "{} slots", p.total_hold_slots);
    assert_eq!(
        p.avg_hold_slots(),
        u64::try_from(p.total_hold_slots).unwrap()
    );
    nox.env.assert_invariants();
}

// --- the gaming vector that matters most ------------------------------------------------------

/// **A drawdown cannot be hidden by refusing to close the losing position.**
///
/// Nothing but a close writes realised PnL to a profile, so a trader sitting on a loser would
/// otherwise show a spotless record for as long as they held it. The permissionless equity
/// crank marks the position anyway and puts the observed drawdown on the record — while the
/// position is still open and the trader has done nothing.
#[test]
fn the_crank_records_a_drawdown_the_trader_never_closed() {
    let mut nox = roomy_mandate();
    nox.open(0, STOP).expect("open");

    // A stranger observes at the entry price. The mandate is *already* a few bps down against
    // its funding high-water mark, because opening crossed the spread and paid the open fee.
    // That cost is real, and the record shows it rather than starting the clock at zero.
    nox.observe_at(PriceSpec::default(), &[0]).expect("peak");
    let spread_cost = nox.profile().max_drawdown_bps;
    assert!(
        spread_cost > 0 && spread_cost < 50,
        "entering costs the spread, and only the spread: {spread_cost} bps"
    );

    nox.observe_at(adverse(), &[0]).expect("the fall");

    let p = nox.profile();
    assert!(
        p.max_drawdown_bps > spread_cost * 4,
        "the adverse move is on the record while the position is still open: \
         {spread_cost} bps → {} bps",
        p.max_drawdown_bps
    );
    assert_eq!(p.trades, 0, "…and the trader has still closed nothing");
    assert_eq!(nox.mandate_state().open_positions, 1);
    nox.env.assert_invariants();
}

/// The high-water mark on the profile only ever rises. A trader cannot walk a drawdown off the
/// record by waiting for the price to come back.
#[test]
fn a_recorded_drawdown_never_shrinks() {
    let mut nox = roomy_mandate();
    nox.open(0, STOP).expect("open");
    nox.observe_at(PriceSpec::default(), &[0]).expect("peak");
    nox.observe_at(adverse(), &[0]).expect("the fall");
    let worst = nox.profile().max_drawdown_bps;
    assert!(worst > 0);

    nox.observe_at(favourable(), &[0]).expect("the recovery");
    assert_eq!(
        nox.profile().max_drawdown_bps,
        worst,
        "recovering does not erase what happened"
    );
    nox.env.assert_invariants();
}

// --- what the tier actually binds ---------------------------------------------------------------

/// A new trader is Bronze, and Bronze is capped at $10,000. The block half.
#[test]
fn a_mandate_larger_than_the_tier_permits_is_refused() {
    use noxfunds::state::TraderTier;
    let over = TraderTier::Bronze.max_mandate() + 1;
    let err = std::panic::catch_unwind(move || mandate_of(over))
        .err()
        .expect("funding above the tier ceiling must fail");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".to_owned());
    assert!(msg.contains("MandateExceedsTierLimit"), "{msg}");
}

/// …and the permit half, one unit below the ceiling.
#[test]
fn a_mandate_exactly_at_the_tier_ceiling_is_accepted() {
    use noxfunds::state::TraderTier;
    let nox = mandate_of(TraderTier::Bronze.max_mandate());
    assert_eq!(
        nox.mandate_state().principal,
        TraderTier::Bronze.max_mandate()
    );
    assert_eq!(nox.profile().active_mandates, 1);
    assert_eq!(nox.profile().mandates_funded, 1);
}

/// Bronze permits one mandate at a time. A second is refused — the trader's attention, not
/// their capital, is what this rule is about.
#[test]
fn a_second_concurrent_mandate_is_refused_at_bronze() {
    let mut nox = roomy_mandate();
    let investor2 = solana_keypair::Keypair::new();
    nox.env
        .svm
        .airdrop(&investor2.pubkey(), 100 * 1_000_000_000)
        .unwrap();

    let mandate2 = mandate_pda(&investor2.pubkey(), &nox.trader.pubkey(), 0);
    let investor2_token = Pubkey::new_unique();
    let mint = nox.env.usdc_mint;
    nox.env.write_token_account(
        investor2_token,
        mint,
        investor2.pubkey(),
        support::INVESTOR_START,
    );
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundMandate {
            trader_profile: profile_pda(&nox.trader.pubkey()),
            usdc_mint: mint,
            investor_token: investor2_token,
            mandate_vault: support::vault_pda(&mandate2),
            token_program: spl_token::ID,
            investor: investor2.pubkey(),
            config: config_pda(),
            trader: nox.trader.pubkey(),
            mandate: mandate2,
            mandate_signer: signer_pda(&mandate2),
            solfx_user_account: Env::user_pda(&signer_pda(&mandate2)),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundMandate {
            seq: 0,
            principal: 500 * ONE_USDC,
            rules: base_rules(),
        }
        .data(),
    };
    let err = nox
        .env
        .send(ix, &[&investor2])
        .expect_err("Bronze holds one mandate at a time");
    assert!(
        format!("{err:?}").contains("TooManyActiveMandates"),
        "{err:?}"
    );
}

/// Settling frees the slot. A breach must not leave a trader permanently one mandate below
/// their tier's limit, so the count comes down on every terminal outcome.
#[test]
fn settlement_frees_the_slot_and_counts_a_profitable_mandate() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    assert_eq!(nox.profile().active_mandates, 1);

    nox.open(0, STOP).expect("open");
    nox.close_at(favourable(), 0).expect("close in profit");
    nox.request_settlement().expect("request");
    let (ix, settler) = nox.claim_ix(&payees);
    nox.env.send(ix, &[&settler]).expect("settle");

    let p = nox.profile();
    assert_eq!(p.active_mandates, 0, "the slot is free again");
    assert_eq!(p.mandates_funded, 1, "…but the history is not rewritten");
    assert_eq!(
        p.mandates_settled_in_profit, 1,
        "it finished above principal"
    );
    nox.env.assert_invariants();
}

/// A mandate that ends below principal is still settled, and still frees the slot — it simply
/// does not count toward Platinum.
#[test]
fn a_losing_mandate_frees_its_slot_without_counting_as_a_win() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();

    nox.open(0, STOP).expect("open");
    nox.close_at(adverse(), 0).expect("close at a loss");
    nox.request_settlement().expect("request");
    let (ix, settler) = nox.claim_ix(&payees);
    nox.env.send(ix, &[&settler]).expect("settle");

    let p = nox.profile();
    assert_eq!(p.active_mandates, 0);
    assert_eq!(p.mandates_settled_in_profit, 0);
    nox.env.assert_invariants();
}

// --- the tier itself ------------------------------------------------------------------------

/// Anyone may recompute a tier, and doing so when nothing has changed is a no-op rather than
/// an error — an investor who suspects a listing is stale should not need the trader's help,
/// and a crank that finds nothing to do should not fail.
#[test]
fn recomputing_a_tier_is_permissionless_and_idempotent() {
    use noxfunds::state::TraderTier;
    let mut nox = roomy_mandate();
    let stranger = solana_keypair::Keypair::new();
    nox.env
        .svm
        .airdrop(&stranger.pubkey(), 1_000_000_000)
        .unwrap();

    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::RecomputeTier {
            caller: stranger.pubkey(),
            profile: profile_pda(&nox.trader.pubkey()),
        }
        .to_account_metas(None),
        data: noxfunds::instruction::RecomputeTier {}.data(),
    };
    nox.env
        .send(ix.clone(), &[&stranger])
        .expect("a stranger may recompute");
    assert_eq!(nox.profile().tier, TraderTier::Bronze);

    nox.env.send(ix, &[&stranger]).expect("and again");
    assert_eq!(nox.profile().tier, TraderTier::Bronze);
}

/// A trader cannot direct their losses onto somebody else's record: the profile is bound to
/// the mandate's trader by its seed, and a mismatched one is refused.
#[test]
fn a_trade_cannot_be_recorded_against_another_traders_profile() {
    let mut nox = roomy_mandate();
    nox.open(0, STOP).expect("open");

    let other = solana_keypair::Keypair::new();
    let admin = nox.env.admin.insecure_clone();
    create_profile(&mut nox.env, &admin, &other.pubkey());

    let p = nox.env.post_price_now(FEED_EUR_USD, adverse());
    let user_account = Env::user_pda(&nox.signer);
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundedClosePosition {
            trader_profile: profile_pda(&other.pubkey()),
            trader: nox.trader.pubkey(),
            config: config_pda(),
            mandate: nox.mandate,
            mandate_signer: nox.signer,
            protocol: nox.env.protocol,
            user_account,
            market: Env::market_pda(0),
            position: Env::position_pda(&user_account, 0, 0),
            collateral_vault: nox.env.collateral_vault,
            lp_pool: nox.env.lp_pool,
            lp_vault: nox.env.lp_vault,
            insurance_fund: nox.env.insurance_fund,
            insurance_vault: nox.env.insurance_vault,
            fee_vault: nox.env.fee_vault,
            price_update: p,
            secondary_price_update: None,
            quote_conversion_price_update: None,
            token_program: spl_token::ID,
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundedClosePosition {
            market_index: 0,
            nonce: 0,
            price_limit: 1,
        }
        .data(),
    };
    let trader = nox.trader.insecure_clone();
    let err = nox
        .env
        .send(ix, &[&trader])
        .expect_err("the seed binds the profile to the mandate's trader");
    // The seeds constraint fires before the explicit one, so either is the right refusal.
    let msg = format!("{err:?}");
    assert!(
        msg.contains("ConstraintSeeds") || msg.contains("ProfileMismatch"),
        "{msg}"
    );
}

/// **A trader's slot cannot be taken by someone who deposits nothing.**
///
/// Measured against the pre-fix program on 2026-09-17: a wallet holding no USDC funded a
/// "$10,000" mandate, and the Bronze trader's only slot was gone — a real investor was then
/// refused with `TooManyActiveMandates`, and nothing in the protocol could free it, because
/// settlement is the investor's to request and the griefer simply never would.
///
/// `fund_mandate` now moves the principal in the same instruction, so the slot costs exactly
/// what it claims to be worth.
#[test]
fn a_traders_slot_cannot_be_taken_without_the_principal() {
    let mut nox = roomy_mandate();
    let trader = Keypair::new();
    let admin = nox.env.admin.insecure_clone();
    create_profile(&mut nox.env, &admin, &trader.pubkey());
    let mint = nox.env.usdc_mint;

    // A griefer with SOL for fees and an empty USDC account.
    let griefer = Keypair::new();
    nox.env
        .svm
        .airdrop(&griefer.pubkey(), 1_000_000_000)
        .unwrap();
    let griefer_token = Pubkey::new_unique();
    nox.env
        .write_token_account(griefer_token, mint, griefer.pubkey(), 0);

    let fund = |investor: &Pubkey, token: Pubkey, principal: u64| {
        let mandate = mandate_pda(investor, &trader.pubkey(), 0);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundMandate {
                trader_profile: profile_pda(&trader.pubkey()),
                usdc_mint: mint,
                investor_token: token,
                mandate_vault: support::vault_pda(&mandate),
                token_program: spl_token::ID,
                investor: *investor,
                config: config_pda(),
                trader: trader.pubkey(),
                mandate,
                mandate_signer: signer_pda(&mandate),
                solfx_user_account: Env::user_pda(&signer_pda(&mandate)),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundMandate {
                seq: 0,
                principal,
                rules: base_rules(),
            }
            .data(),
        }
    };

    let ix = fund(&griefer.pubkey(), griefer_token, 10_000 * ONE_USDC);
    let err = nox
        .env
        .send(ix, &[&griefer])
        .expect_err("a mandate cannot be funded with money the investor does not have");
    assert!(
        format!("{err:?}").contains("InsufficientPrincipal"),
        "{err:?}"
    );

    let profile: noxfunds::state::TraderProfile = nox.env.read(&profile_pda(&trader.pubkey()));
    assert_eq!(
        profile.active_mandates, 0,
        "the refused attempt took no slot"
    );

    // …and the slot is still there for someone who actually pays.
    let real = Keypair::new();
    nox.env.svm.airdrop(&real.pubkey(), 1_000_000_000).unwrap();
    let real_token = Pubkey::new_unique();
    nox.env
        .write_token_account(real_token, mint, real.pubkey(), support::INVESTOR_START);
    let ix = fund(&real.pubkey(), real_token, 500 * ONE_USDC);
    nox.env.send(ix, &[&real]).expect("a real investor funds");

    let profile: noxfunds::state::TraderProfile = nox.env.read(&profile_pda(&trader.pubkey()));
    assert_eq!(profile.active_mandates, 1);
    assert_eq!(
        nox.env.token_balance(&support::vault_pda(&mandate_pda(
            &real.pubkey(),
            &trader.pubkey(),
            0
        ))),
        500 * ONE_USDC,
        "the vault holds exactly the principal"
    );
    nox.env.assert_invariants();
}
