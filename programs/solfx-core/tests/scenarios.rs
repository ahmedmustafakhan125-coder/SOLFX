//! Historical crisis replays (`ARCHITECTURE.md` § 12.4).
//!
//! > *If the engine survives a synthetic CHF-depeg replay with the insurance fund intact, you
//! > have something. Until then you have a demo.*
//!
//! This file is Phase 4's exit criterion. Every scenario replays a real event through the
//! real instructions and asserts that the protocol is still solvent and still adds up
//! afterwards.
//!
//! # What "survives" means here, precisely
//!
//! Not "nothing went wrong" — in a 30% gap a great deal goes wrong, and a protocol that
//! claimed otherwise would be lying. It means:
//!
//! 1. **Every invariant still holds.** No USDC was created or destroyed (I1, I2, I6, I7).
//! 2. **The waterfall absorbed the shortfall in the right order** — insurance first, then
//!    ADL, and LPs only after both (§ 6.9).
//! 3. **Liquidations remained profitable**, so a real keeper would have run them.
//! 4. **The protocol can still be traded** afterwards: it degrades, it does not brick.

// Test code asserts against known values and unwraps expected-Ok results. The workspace
// denies `unwrap`, `expect` and `panic` because a panic in a *program* is a failed
// transaction with no named error; in a test a panic is the reporting mechanism.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

mod common;

use anchor_lang::prelude::Pubkey;
use common::*;
use solfx_core::state::{Direction, MarketStatus};

const MINI: u64 = ONE_LOT / 10;
const NO_BOUND_BUY: i64 = i64::MAX;
const NO_BOUND_SELL: i64 = 1;

/// EUR/CHF at 1.2010, the peg the SNB defended until 15 January 2015.
const CHF_PEG: i64 = 120_100_000;
/// Where it landed. −30% in minutes, with no tradeable prices in between.
const CHF_FLOOR: i64 = 84_070_000;

/// A market configured like EUR/CHF under the peg: quiet, tight, and apparently safe.
///
/// That is the point. Every risk parameter here would have looked conservative on
/// 14 January 2015.
fn pegged_market() -> MarketSpec {
    let mut s = MarketSpec::eur_usd();
    s.params.symbol = "EURCHF".into();
    s.params.max_leverage = 50;
    s.params.imr_bps = 200;
    s.params.mmr_bps = 100;
    s.params.liquidation_fee_bps = 50;
    // A 3% deviation breaker — generous for a currency that had not moved 1% in three years.
    s.params.max_deviation_bps = 300;
    // 10%. Sized for the tail, not the average: during the depeg, quoted spreads went from
    // ~2 pips to 500+, which on a 1.20 price is over 400 bps of uncertainty. A ceiling set
    // at a few multiples of normal would have blocked every liquidation on the day it
    // mattered — see `Market::liquidation_max_conf_bps`.
    s.params.liquidation_max_conf_bps = 1_000;
    s
}

// --- the hard gate ---------------------------------------------------------------------------

/// **CHF depeg, 15 January 2015.**
///
/// The SNB removed the 1.20 floor without warning and EUR/CHF fell ~30% in minutes. There
/// were no tradeable prices on the way down. It bankrupted Alpari UK — the broker
/// `SolFX_Decentralized_Forex_Brokerage_Proposal.pdf` cites as a cautionary example — and
/// left FXCM needing a $300m rescue.
///
/// The engine cannot prevent the loss. **No liquidation engine outruns a gap** (§ 6.3): the
/// only defences are conservative leverage and a fund that absorbs what is left. What it must
/// do is stay *correct* — conserve every unit, cover the shortfall in the documented order,
/// and remain usable afterwards.
#[test]
fn chf_depeg_the_engine_stays_solvent_and_correct() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &pegged_market());
    env.seed_pool(2_000_000 * ONE_USDC);
    // § 6.9: target 2% of maximum OI. The whole point of this test is whether that is enough.
    env.seed_insurance(100_000 * ONE_USDC);

    // Five traders, all long EUR/CHF at the peg — the crowded trade of 2014, because the SNB
    // had promised to defend the floor and everyone believed it.
    let longs: Vec<User> = (0..5)
        .map(|_| {
            let u = env.new_user(100_000 * ONE_USDC, Pubkey::default());
            env.deposit(&u, 50_000 * ONE_USDC).unwrap();
            u
        })
        .collect();

    for (i, u) in longs.iter().enumerate() {
        let p = env.post_price_now(FEED_EUR_USD, PriceSpec::at(CHF_PEG).conf(60_000));
        // Escalating leverage: $500 to $2,500 of margin against ~$12k of notional.
        let collateral = (500 + 500 * i as u64) * ONE_USDC;
        env.open(u, 0, 0, Direction::Long, MINI, collateral, NO_BOUND_BUY, p)
            .unwrap_or_else(|e| panic!("trader {i} could not open: {e}"));
    }
    env.assert_invariants();

    let aum_before = env.lp_state().aum;
    let insurance_before = env.insurance_state().balance;
    let vaults_before = env.total_vault_balance();

    // --- 09:30 CET: the announcement ---
    //
    // The price gaps straight through every liquidation level. There is no intermediate tick
    // to liquidate at, which is exactly what makes a depeg different from a crash.
    env.advance_clock(60);
    let crash = env.post_price_now(
        FEED_EUR_USD,
        PriceSpec::at(CHF_FLOOR).ema(CHF_PEG).conf(600_000),
    );

    // The deviation breaker fires: 30% is a hundred times the 3% limit.
    env.crank_price(0, crash).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "a 30% dislocation must halt the market for review, not be traded through"
    );

    // --- liquidation ---
    //
    // A halt never blocks a liquidation. Every position is now deeply underwater.
    let mut total_reward = 0u64;
    for (i, u) in longs.iter().enumerate() {
        let (liq, token) = env.new_liquidator();
        let p = env.post_price_now(
            FEED_EUR_USD,
            PriceSpec::at(CHF_FLOOR).ema(CHF_PEG).conf(600_000),
        );
        env.liquidate(u, 0, 0, &liq, token, p)
            .unwrap_or_else(|e| panic!("trader {i} could not be liquidated: {e}"));
        env.assert_invariants();
        total_reward += env.token_balance(&token);
    }

    // --- what the engine must have done ---

    // 1. Everything is accounted for. This is the assertion that matters most: a 30% gap is
    //    where an accounting bug would hide.
    env.assert_invariants();
    let vaults_after = env.total_vault_balance();
    assert_eq!(
        vaults_after + total_reward,
        vaults_before,
        "USDC was created or destroyed: vaults went {vaults_before} -> {vaults_after} \
         with {total_reward} paid to liquidators"
    );

    // 2. The book is empty and the counters returned to zero.
    let m = env.market_state(0);
    assert_eq!(m.open_position_count, 0);
    assert_eq!(m.base_oi_long, 0);
    assert_eq!(m.base_oi_short, 0);

    // 3. There was real bad debt — a depeg that produced none would mean the scenario was
    //    not severe enough to be a test.
    let bad_debt = env.protocol_state().total_bad_debt;
    assert!(
        bad_debt > 0,
        "a 30% gap against 50x positions must produce bad debt; this scenario is not \
         testing what it claims to"
    );

    // 4. The waterfall ran in the documented order: insurance absorbed it, so LPs did not.
    assert!(
        env.insurance_state().balance < insurance_before,
        "the insurance fund must have been drawn on"
    );
    assert_eq!(
        env.insurance_state().total_bad_debt_covered,
        bad_debt,
        "the fund must have covered the whole shortfall — that is what 2% of OI is for"
    );
    assert_eq!(
        env.market_state(0).pending_adl_debt,
        0,
        "with the fund intact, nothing should be left for ADL"
    );

    // 5. LPs came out ahead: they were the counterparty to five losing trades, and the
    //    insurance fund made them whole for the part the traders could not pay.
    assert!(
        env.lp_state().aum > aum_before,
        "the pool was the counterparty to five losers and must be up: {} vs {}",
        env.lp_state().aum,
        aum_before
    );

    // 6. Liquidations were worth running. A liquidator who loses money does not show up, and
    //    a position nobody liquidates is a position that keeps falling.
    assert!(
        total_reward > 0,
        "no liquidator was paid across five liquidations — nobody would have run them"
    );

    // 7. The protocol still works. An admin reviews the halt and reopens; trading resumes.
    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Active);
    env.send(ix, &[&admin]).unwrap();

    let survivor = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&survivor, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::at(CHF_FLOOR).conf(60_000));
    env.open(
        &survivor,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .expect("the protocol must be usable after a depeg, not bricked");
    env.assert_invariants();
}

/// The same event with the insurance fund **empty** — the configuration § 6.9 warns against.
///
/// The protocol must still conserve every unit; the difference is only *who* pays. The
/// shortfall queues for ADL instead of being absorbed, which is the visible symptom of
/// launching under-capitalised.
#[test]
fn chf_depeg_without_an_insurance_fund_queues_the_shortfall_instead() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &pegged_market());
    env.seed_pool(2_000_000 * ONE_USDC);
    // No insurance. § 6.9: "Do not launch with an empty insurance fund."

    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::at(CHF_PEG).conf(60_000));
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        500 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();

    let vaults_before = env.total_vault_balance();

    env.advance_clock(60);
    let (liq, token) = env.new_liquidator();
    let crash = env.post_price_now(
        FEED_EUR_USD,
        PriceSpec::at(CHF_FLOOR).ema(CHF_PEG).conf(600_000),
    );
    env.liquidate(&user, 0, 0, &liq, token, crash).unwrap();
    env.assert_invariants();

    assert!(
        env.market_state(0).pending_adl_debt > 0,
        "with no insurance the shortfall must queue for ADL rather than disappear"
    );
    assert_eq!(
        env.total_vault_balance() + env.token_balance(&token),
        vaults_before,
        "conservation must hold whether or not the fund is capitalised"
    );
}

// --- GBP flash crash, 7 October 2016 ---------------------------------------------------------

/// GBP/USD fell ~6% in two minutes and recovered most of it within the hour.
///
/// The lesson is the opposite of the depeg: the danger is liquidating **into the wick**.
/// A trader whose position was solvent before and after the spike should not have been closed
/// at the bottom of it. The deviation breaker is what prevents that — it halts on the spike
/// rather than pricing against it.
#[test]
fn gbp_flash_crash_the_deviation_breaker_halts_rather_than_trading_the_wick() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    // A well-margined position: ~$10.8k of notional on $3,000. It should survive a 6% wick.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        3_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();

    // The wick: −6% in two minutes.
    env.advance_clock(120);
    let wick = env.post_price_now(
        FEED_EUR_USD,
        PriceSpec::at(102_030_000).ema(108_543_000).conf(500_000),
    );
    env.crank_price(0, wick).unwrap();

    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "a 6% dislocation is 20x the deviation limit and must halt"
    );

    // The position is still solvent — it was never liquidatable, and the halt is what stopped
    // it from being closed at the worst tick of the day.
    let (liq, token) = env.new_liquidator();
    let p = env.post_price_now(
        FEED_EUR_USD,
        PriceSpec::at(102_030_000).ema(108_543_000).conf(500_000),
    );
    assert_err_contains(
        env.liquidate(&user, 0, 0, &liq, token, p),
        "Position is not liquidatable",
    );

    // The price recovers. The trader closes at something close to where they started.
    env.advance_clock(3_600);
    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Active);
    env.send(ix, &[&admin]).unwrap();

    let recovered = env.post_price_now(FEED_EUR_USD, PriceSpec::at(108_000_000).conf(13_893));
    env.close(&user, 0, 0, NO_BOUND_SELL, recovered).unwrap();
    env.assert_invariants();

    let free = env.user_state(&user).free_collateral;
    assert!(
        free > 45_000 * ONE_USDC,
        "a trader who was solvent throughout a wick should come out roughly whole: {free}"
    );
}

// --- the weekend gap ---------------------------------------------------------------------------

/// The C-1 exploit, attempted end to end.
///
/// A trader sees a weekend event that will gap EUR/USD on Monday and tries to open maximum
/// size against Friday's frozen price. **This is the single hole that drains the vault**, and
/// the whole session state machine exists to close it.
#[test]
fn the_weekend_stale_price_exploit_is_refused() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);

    let attacker = env.new_user(200_000 * ONE_USDC, Pubkey::default());
    env.deposit(&attacker, 100_000 * ONE_USDC).unwrap();

    // Friday 20:59 UTC — one minute before the interbank close.
    let saturday = 1_785_542_400;
    let friday_close = saturday - 86_400 + 21 * 3_600;
    env.set_clock(friday_close - 60);
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::ReduceOnly,
        "the market winds down before the close"
    );

    // Attempt one: open in the final minute. Refused — ReduceOnly takes no new risk.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.open(
            &attacker,
            0,
            0,
            Direction::Long,
            MINI * 5,
            50_000 * ONE_USDC,
            NO_BOUND_BUY,
            p,
        ),
        "Market does not permit opening positions",
    );

    // Saturday: the feed has stopped. The last price on chain is Friday's.
    env.set_clock(saturday + 12 * 3_600);
    env.crank_session(0, None).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);

    // Attempt two: the exploit proper. Post Friday's close — a genuine, fully-verified Pyth
    // update — and open maximum size at a price that cannot move against them.
    let friday_price = env.post_price(
        FEED_EUR_USD,
        PriceSpec::default().published_at(friday_close),
    );
    assert_err_contains(
        env.open(
            &attacker,
            0,
            0,
            Direction::Long,
            MINI * 5,
            50_000 * ONE_USDC,
            NO_BOUND_BUY,
            friday_price,
        ),
        "Market does not permit opening positions",
    );

    // Attempt three: the same, on a market someone forgot to crank. The staleness gate is a
    // second, independent defence — it does not depend on the session machine having run.
    env.patch_market(0, |m| m.status = MarketStatus::Active);
    assert_err_contains(
        env.open(
            &attacker,
            0,
            0,
            Direction::Long,
            MINI * 5,
            50_000 * ONE_USDC,
            NO_BOUND_BUY,
            friday_price,
        ),
        "Oracle price is stale",
    );

    env.assert_invariants();
    assert_eq!(env.market_state(0).open_position_count, 0);
}

/// Monday's gap arrives while positions are frozen through the weekend.
///
/// The position was opened on Friday and could not be managed over the weekend — that is the
/// risk § 7.3 requires disclosing before a Friday trade. What the protocol must do is reopen
/// through a gap window rather than straight into normal trading, and liquidate cleanly if
/// the gap took the position out.
#[test]
fn a_weekend_gap_is_handled_through_the_gap_window() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let saturday = 1_785_542_400;
    let friday = saturday - 86_400 + 12 * 3_600;

    // Friday midday: a leveraged long.
    env.set_clock(friday);
    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        300 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();

    // The weekend: frozen.
    env.set_clock(saturday + 12 * 3_600);
    env.crank_session(0, None).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);

    // Sunday 22:00: the feed returns 200 pips lower.
    let sunday_open = saturday + 86_400 + 22 * 3_600;
    env.set_clock(sunday_open);
    let gapped = env.post_price_now(
        FEED_EUR_USD,
        PriceSpec::at(106_543_000).ema(108_543_000).conf(40_000),
    );
    env.crank_session(0, Some(gapped)).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::GapWindow,
        "reopening must go through the gap window — the weekend's news arrives in one tick"
    );

    // No new positions during the window.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::at(106_543_000).conf(40_000));
    assert_err_contains(
        env.open(
            &user,
            0,
            1,
            Direction::Long,
            MINI,
            5_000 * ONE_USDC,
            NO_BOUND_BUY,
            p,
        ),
        "Market does not permit opening positions",
    );

    // The gapped position is liquidated cleanly.
    let (liq, token) = env.new_liquidator();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::at(106_543_000).conf(40_000));
    env.liquidate(&user, 0, 0, &liq, token, p).unwrap();
    env.assert_invariants();

    // The window clears and trading resumes.
    env.advance_clock(6 * 60);
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::at(106_543_000).conf(40_000));
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
    env.assert_invariants();
}

// --- an EM holiday -------------------------------------------------------------------------------

/// An emerging-market pair whose local exchange is shut while the interbank week runs on.
///
/// Phase 0b measured USD/BRL trading 14:00–21:00 UTC, USD/COP 14:00–18:00, USD/CLP 14:00–20:00
/// and USD/PEN 14:00–19:00. A protocol that assumed one global session would leave all four
/// open against a dead feed — the C-1 exploit on precisely the pairs where a central-bank
/// devaluation gap is most likely, and the reason § 7.3 requires a calendar per market.
#[test]
fn an_em_market_is_shut_outside_local_hours_while_majors_keep_trading() {
    let mut env = Env::new();
    env.init_protocol();

    // Market 0: EUR/USD on the interbank week.
    env.list_and_activate(0, &MarketSpec::eur_usd());

    // Market 1: USD/BRL, 14:00–21:00 UTC on a Wednesday.
    let mut brl = MarketSpec::eur_usd();
    brl.params.symbol = "USDBRL".into();
    brl.params.pyth_feed_id = FEED_USD_INR; // a distinct fixture feed
    brl.params.session_open_dow = 3;
    brl.params.session_open_seconds = 14 * 3_600;
    brl.params.session_close_dow = 3;
    brl.params.session_close_seconds = 21 * 3_600;
    env.list_and_activate(1, &brl);
    env.seed_pool(1_000_000 * ONE_USDC);

    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    let saturday = 1_785_542_400;
    let wednesday = saturday - 3 * 86_400;

    // 23:00 UTC Wednesday: the interbank market is open, São Paulo is not.
    let after_local_close = wednesday + 23 * 3_600;
    env.set_clock(after_local_close);

    let eur = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.crank_session(0, Some(eur)).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Active,
        "EUR/USD trades through the night"
    );

    let brl_price = env.post_price_now(FEED_USD_INR, PriceSpec::at(5_400_000_000).conf(2_000_000));
    env.crank_session(1, Some(brl_price)).unwrap();
    assert_eq!(
        env.market_state(1).status,
        MarketStatus::Halted,
        "USD/BRL follows São Paulo, not London — this is the C-1 exploit on the pairs where \
         a devaluation gap is most likely"
    );

    // The major is tradeable; the EM pair is not.
    let eur = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        eur,
    )
    .expect("a major must keep trading");

    let brl_price = env.post_price_now(FEED_USD_INR, PriceSpec::at(5_400_000_000).conf(2_000_000));
    assert_err_contains(
        env.open(
            &user,
            1,
            0,
            Direction::Long,
            MINI,
            5_000 * ONE_USDC,
            NO_BOUND_BUY,
            brl_price,
        ),
        "Market does not permit opening positions",
    );

    env.assert_invariants();
}

/// A local holiday, which is where the two signals earn their keep.
///
/// The calendar says the market should be open — it is a Wednesday afternoon — but the
/// exchange is shut for a public holiday and the feed has stopped. **The feed is the signal
/// that handles holidays**, with no calendar to maintain and nothing to forget to update.
#[test]
fn a_local_holiday_halts_the_market_even_though_the_calendar_says_open() {
    let mut env = Env::new();
    env.init_protocol();

    let mut brl = MarketSpec::eur_usd();
    brl.params.symbol = "USDBRL".into();
    brl.params.session_open_dow = 3;
    brl.params.session_open_seconds = 14 * 3_600;
    brl.params.session_close_dow = 3;
    brl.params.session_close_seconds = 21 * 3_600;
    env.list_and_activate(0, &brl);

    let saturday = 1_785_542_400;
    let wednesday_afternoon = saturday - 3 * 86_400 + 16 * 3_600;
    env.set_clock(wednesday_afternoon);

    // The calendar says open.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);

    // The holiday: the feed stops mid-session.
    env.advance_clock(600);
    env.crank_session(0, None).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "the feed is the primary signal precisely so holidays need no calendar entry"
    );
    env.assert_invariants();
}
