//! Stage 2 — the full rulebook, split between CPI and crank.
//!
//! Part 10's exit criterion: *each rule has a test that proves it blocks **and** one that
//! proves it permits; each names its own error.* A rule with only the first is
//! indistinguishable from a rule that blocks everything.
//!
//! Stage 1 covered the rules derivable from a single trade. This covers the three that are
//! not: total open notional across the book, minimum hold on a voluntary close, and drawdown —
//! which no trade-time check can ever catch, because **a trader who is losing can simply stop
//! trading**.

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
use noxfunds::state::MandateState;
use solana_signer::Signer;
use solfx_core::state::Direction;

const MARKET_0: u128 = 1;
const SPOT: i64 = 1_085_430_000;
const PRINCIPAL: u64 = 10_000 * ONE_USDC;

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
    investor: solana_keypair::Keypair,
    trader: solana_keypair::Keypair,
    mandate: Pubkey,
    signer: Pubkey,
}

fn setup_with(rules: MandateRules) -> Nox {
    // Deposit the whole principal. It used to ask for twice the principal, which the old
    // fixture could grant by writing a vault balance out of thin air; a mandate can now only
    // put into SolFX what the investor actually paid in.
    setup_funded(rules, PRINCIPAL, PRINCIPAL)
}

/// A mandate whose principal and deposit are whatever the test needs.
///
/// Size matters for the drawdown tests: a $20,000 mandate would need a $600 loss to breach a
/// 3% limit, which no position inside a $2,000 per-trade ceiling can produce. A small mandate
/// makes an ordinary adverse move a real breach rather than a contrived one.
fn setup_funded(rules: MandateRules, principal: u64, deposit: u64) -> Nox {
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

    let investor = solana_keypair::Keypair::new();
    let trader = solana_keypair::Keypair::new();
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
    };
    nox.fund_for_trading(deposit);
    nox
}

/// A mandate small enough that an ordinary adverse move breaches it.
fn small_mandate() -> Nox {
    setup_funded(
        MandateRules {
            max_drawdown_bps: 100,
            max_daily_loss_bps: 100,
            max_risk_per_trade_bps: 100,
            min_hold_slots: 0,
            ..base_rules()
        },
        1_000 * ONE_USDC,
        1_000 * ONE_USDC,
    )
}

/// 250 bps below spot — a real loss, and inside the market's 300 bps deviation breaker, so the
/// oracle accepts it rather than halting.
fn adverse() -> PriceSpec {
    PriceSpec::at(105_830_000).conf(13_893)
}

fn base_rules() -> MandateRules {
    MandateRules {
        max_trade_notional: 2_000 * ONE_USDC,
        max_total_notional: 6_000 * ONE_USDC,
        max_drawdown_bps: 300,
        max_daily_loss_bps: 300,
        max_risk_per_trade_bps: 100,
        max_stop_distance_bps: 200,
        max_concurrent_positions: 5,
        allowed_markets: MARKET_0,
        min_hold_slots: 0,
    }
}

impl Nox {
    fn fund_for_trading(&mut self, usdc: u64) {
        self.env.svm.airdrop(&self.signer, 1_000_000_000).unwrap();
        // Filled by `fund_mandate`; this only moves what is already there into SolFX.
        let vault = support::vault_pda(&self.mandate);
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

    #[allow(clippy::too_many_arguments)]
    fn open(&mut self, nonce: u8, size_base: u64, stop: i64) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        let position = Env::position_pda(&user_account, 0, nonce);
        let ix = Instruction {
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
                collateral: 500 * ONE_USDC,
                price_limit: i64::MAX,
                order_id: nonce,
                stop_loss_price: stop,
            }
            .data(),
        };
        let trader = self.trader.insecure_clone();
        let r = self.env.send(ix, &[&trader]);
        if r.is_ok() {
            self.env.track_position(position);
        }
        r
    }

    fn close(&mut self, nonce: u8) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
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

    /// Run the permissionless crank at the current market price.
    fn observe(&mut self, nonces: &[u8]) -> TestResult {
        self.observe_at(PriceSpec::default(), nonces)
    }

    /// Run the crank at a chosen price.
    ///
    /// The price is a parameter rather than always the default because an earlier version
    /// posted `PriceSpec::default()` inside `observe`, which silently overwrote the adverse
    /// price a drawdown test had just set — the mandate then observed as healthy and the test
    /// failed for a reason that had nothing to do with the rule.
    fn observe_at(&mut self, spec: PriceSpec, nonces: &[u8]) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, spec);
        let observer = solana_keypair::Keypair::new();
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
        self.env.send(ix, &[&observer])
    }

    fn mandate(&self) -> noxfunds::state::Mandate {
        self.env.read(&self.mandate)
    }
}

// --- total open notional: blocks, and permits ----------------------------------------------

/// Three trades at the per-trade ceiling would be three times the exposure the investor
/// authorised. The book limit is what stops it.
#[test]
fn the_book_limit_blocks_a_fourth_trade_and_permits_the_third() {
    // $2,000 a trade, $6,000 across the book. ~$1,085 notional each, so five would fit the
    // count limit but not the book.
    let mut nox = setup_with(MandateRules {
        max_total_notional: 3_000 * ONE_USDC,
        ..base_rules()
    });
    let stop = SPOT - (SPOT * 100) / 10_000;
    let size = ONE_LOT / 100; // ~$1,085

    nox.open(0, size, stop).expect("first is inside the book");
    nox.open(1, size, stop).expect("second is inside the book");
    let err = nox
        .open(2, size, stop)
        .expect_err("a third takes the book past $3,000");
    assert!(
        format!("{err:?}").contains("TotalNotionalExceeded"),
        "refused for the right reason: {err:?}"
    );

    // And the permit half: the book records exactly what is open, so closing frees room.
    assert_eq!(nox.mandate().open_positions, 2);
    nox.close(0).expect("close the first");
    assert!(
        nox.mandate().open_notional < 2_000 * ONE_USDC,
        "closing must return its notional to the book"
    );
    nox.open(2, size, stop)
        .expect("the freed room must be usable");
    nox.env.assert_invariants();
}

// --- minimum hold: blocks a scalp, permits a stop-out and a patient close -------------------

/// A voluntary close inside the mandate's hold floor is refused.
#[test]
fn a_scalp_is_blocked_and_a_patient_close_permitted() {
    let mut nox = setup_with(MandateRules {
        min_hold_slots: 50,
        ..base_rules()
    });
    let stop = SPOT - (SPOT * 100) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    let err = nox.close(0).expect_err("closing immediately is a scalp");
    assert!(
        format!("{err:?}").contains("MinimumHoldNotMet"),
        "refused for the right reason: {err:?}"
    );

    // Wait it out, and the same close is permitted. Without this the rule would be
    // indistinguishable from one that blocks every close.
    nox.env.advance_slot(60);
    nox.close(0).expect("a patient close must be allowed");
    assert_eq!(nox.mandate().open_positions, 0);
    nox.env.assert_invariants();
}

/// **A stop-out is exempt, and by construction rather than by an exception.**
///
/// A stop fires through `solfx-core`'s own `execute_trigger_order`, which never enters
/// NOXFUNDS. So a trader stopped out three minutes into a ten-minute floor does not breach a
/// rule through something they did not do.
#[test]
fn a_stop_out_is_not_subject_to_the_hold_floor() {
    let mut nox = setup_with(MandateRules {
        min_hold_slots: 1_000,
        ..base_rules()
    });
    // 30 bps below entry, so a modest move reaches it without troubling the deviation breaker.
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    let user_account = Env::user_pda(&nox.signer);
    let position = Env::position_pda(&user_account, 0, 0);
    assert!(nox.env.trigger_exists(&position, 0));

    // Move the market through the stop and fire it as a stranger would, immediately.
    let keeper = solana_keypair::Keypair::new();
    nox.env
        .svm
        .airdrop(&keeper.pubkey(), 100 * 1_000_000_000)
        .unwrap();
    // 1.081 — below the 1.08217 stop, and 41 bps from the EMA, well inside the breaker.
    let hit = nox
        .env
        .post_price_now(FEED_EUR_USD, PriceSpec::at(108_100_000).conf(13_893));
    // Built inline rather than through the harness helper, which wants a `User` with a
    // keypair — a mandate's SolFX account is authorised by a PDA and has none.
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::ExecuteTriggerOrder {
            keeper: keeper.pubkey(),
            protocol: nox.env.protocol,
            user_account,
            market: Env::market_pda(0),
            position,
            trigger_order: Env::trigger_pda(&position, 0),
            collateral_vault: nox.env.collateral_vault,
            lp_pool: nox.env.lp_pool,
            lp_vault: nox.env.lp_vault,
            insurance_fund: nox.env.insurance_fund,
            insurance_vault: nox.env.insurance_vault,
            fee_vault: nox.env.fee_vault,
            price_update: hit,
            secondary_price_update: None,
            quote_conversion_price_update: None,
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::ExecuteTriggerOrder {}.data(),
    };
    nox.env
        .send(ix, &[&keeper])
        .expect("a stop must fire regardless of the mandate's hold floor");

    assert!(
        !nox.env.trigger_exists(&position, 0),
        "the order was consumed"
    );
    nox.env.assert_invariants();
}

// --- drawdown: the rule no trade-time check can catch ---------------------------------------

/// A healthy mandate observes clean and stays `Active`. The permit half.
#[test]
fn observing_a_healthy_mandate_leaves_it_active() {
    let mut nox = setup_with(base_rules());
    nox.observe(&[]).expect("an idle mandate observes fine");

    let m = nox.mandate();
    assert_eq!(m.state, MandateState::Active);
    assert!(m.last_observed_at > 0, "the observation is recorded");
    assert_eq!(
        m.peak_equity,
        m.last_equity.max(PRINCIPAL),
        "peak tracks the high-water mark"
    );
    nox.env.assert_invariants();
}

/// Equity is counted across open positions, not just free collateral.
#[test]
fn an_open_position_is_counted_in_equity() {
    let mut nox = setup_with(base_rules());
    let stop = SPOT - (SPOT * 100) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    nox.observe(&[0]).expect("observe with one position");
    let m = nox.mandate();
    // Deposited $10,000 — the whole principal; $500 of it is now position margin. If positions
    // were ignored the observation would read ~$9,500 and the mandate would look like it had
    // lost money.
    assert!(
        m.last_equity > 9_500 * ONE_USDC,
        "position equity must be counted, got {}",
        m.last_equity
    );
    assert_eq!(m.state, MandateState::Active);
    nox.env.assert_invariants();
}

/// The crank refuses to guess. A caller who omits an open position would understate equity and
/// could breach a healthy mandate.
#[test]
fn the_crank_requires_every_open_position() {
    let mut nox = setup_with(base_rules());
    let stop = SPOT - (SPOT * 100) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    assert!(
        nox.observe(&[]).is_err(),
        "omitting an open position must be refused, not silently under-counted"
    );
    nox.observe(&[0]).expect("supplying it works");
    nox.env.assert_invariants();
}

/// **The block half, and the whole reason the crank exists.**
///
/// A trader who is losing can simply stop trading — no trade-time check ever fires again. Here
/// the position is left open, the market moves against it, and a **stranger** breaches the
/// mandate without the trader or the operator lifting a finger.
#[test]
fn a_mandate_past_its_drawdown_is_breached_by_a_stranger() {
    let mut nox = small_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    nox.observe(&[0]).expect("first observation sets the peak");
    let peak = nox.mandate().peak_equity;
    assert!(peak > 0);
    assert_eq!(nox.mandate().state, MandateState::Active);

    // The market moves 250 bps against the position and the trader does nothing — which is
    // exactly the behaviour no trade-time check can catch.
    nox.observe_at(adverse(), &[0]).expect("anyone may observe");

    let m = nox.mandate();
    assert!(
        m.last_equity < peak,
        "equity {} must have fallen below the peak {}",
        m.last_equity,
        peak
    );
    assert_eq!(
        m.state,
        MandateState::Breached,
        "equity {} against peak {} must breach a 100 bp limit",
        m.last_equity,
        m.peak_equity
    );
    nox.env.assert_invariants();
}

/// A breached mandate stops trading. The transition has teeth.
#[test]
fn a_breached_mandate_cannot_open_anything() {
    let mut nox = small_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.observe(&[0]).expect("peak");
    nox.observe_at(adverse(), &[0]).expect("observe");
    assert_eq!(nox.mandate().state, MandateState::Breached);

    let err = nox
        .open(1, ONE_LOT / 100, stop)
        .expect_err("a breached mandate is done trading");
    assert!(
        format!("{err:?}").contains("MandateNotActive"),
        "refused for the right reason: {err:?}"
    );
    nox.env.assert_invariants();
}

/// Breaching happens once. A second observation must not re-transition, or the event stream
/// would lie about when the mandate broke.
#[test]
fn a_breach_is_recorded_once() {
    let mut nox = small_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.observe(&[0]).expect("peak");
    nox.observe_at(adverse(), &[0]).expect("breach");
    assert_eq!(nox.mandate().state, MandateState::Breached);

    nox.observe_at(adverse(), &[0])
        .expect("observing a breached mandate is still allowed");
    assert_eq!(
        nox.mandate().state,
        MandateState::Breached,
        "and leaves it where it was"
    );
    nox.env.assert_invariants();
}
