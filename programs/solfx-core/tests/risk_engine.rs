//! The risk engine (`ARCHITECTURE.md` § 6.7, § 6.8, § 6.9, § 7.3).
//!
//! Liquidation, funding, carry, the insurance fund, auto-deleveraging, and the regime state
//! machines — exercised through the real instructions with invariants re-checked after every
//! step.

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

/// EUR/USD, a funded pool, a seeded insurance fund and a trader with $50k.
fn env_with_position(direction: Direction, collateral: u64) -> (Env, User) {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let bound = if direction == Direction::Long {
        NO_BOUND_BUY
    } else {
        NO_BOUND_SELL
    };
    env.open(&user, 0, 0, direction, MINI, collateral, bound, price)
        .unwrap();
    env.assert_invariants();
    (env, user)
}

// --- liquidation (§ 6.8) -------------------------------------------------------------------

/// A healthy position must not be liquidatable. Strictly: a loose comparison here is
/// liquidation griefing (threat T7) — an attacker collecting a penalty from a solvent trader.
#[test]
fn a_healthy_position_cannot_be_liquidated() {
    let (mut env, user) = env_with_position(Direction::Long, 1_000 * ONE_USDC);
    let (liq, liq_token) = env.new_liquidator();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());

    assert_err_contains(
        env.liquidate(&user, 0, 0, &liq, liq_token, price),
        "Position is not liquidatable",
    );
    env.assert_invariants();
}

/// $10,854 of notional on $150 of margin is ~72x. The maintenance margin is 1%, so ~$108.
/// A 100-pip move against it costs ~$100 and takes equity under the requirement.
#[test]
fn an_underwater_long_is_liquidated_and_the_liquidator_is_paid() {
    let (mut env, user) = env_with_position(Direction::Long, 250 * ONE_USDC);
    let (liq, liq_token) = env.new_liquidator();

    let free_before = env.user_state(&user).free_collateral;
    let aum_before = env.lp_state().aum;

    // −150 pips.
    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(107_043_000).conf(13_893));
    env.liquidate(&user, 0, 0, &liq, liq_token, price).unwrap();
    env.assert_invariants();

    assert!(!env.position_exists(&user, 0, 0), "position must be closed");
    assert_eq!(env.market_state(0).open_position_count, 0);
    assert_eq!(env.user_state(&user).open_positions, 0);

    // The liquidator was paid, and paid enough to be worth doing.
    let reward = env.token_balance(&liq_token);
    assert!(reward > 0, "the liquidator was not paid");
    assert!(
        reward > 10_000,
        "reward {reward} is below the ~$0.01 a 200k-CU transaction costs; \
         an unprofitable liquidation is an unliquidated position (§ 6.8)"
    );

    // The pool took the trader's loss.
    assert!(
        env.lp_state().aum > aum_before,
        "the pool must have received"
    );

    // A liquidation takes the penalty, not the account: whatever survived went back.
    let returned = env.user_state(&user).free_collateral - free_before;
    assert!(
        returned < 250 * ONE_USDC,
        "the trader should not get their whole margin back"
    );
}

/// A short is liquidated by a rise — the mirror.
#[test]
fn an_underwater_short_is_liquidated() {
    let (mut env, user) = env_with_position(Direction::Short, 250 * ONE_USDC);
    let (liq, liq_token) = env.new_liquidator();

    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(110_043_000).conf(13_893));
    env.liquidate(&user, 0, 0, &liq, liq_token, price).unwrap();
    env.assert_invariants();

    assert!(!env.position_exists(&user, 0, 0));
    assert!(env.token_balance(&liq_token) > 0);
}

/// The penalty splits liquidator 40% / insurance 40% / treasury 20% (§ 6.8 step 6).
#[test]
fn the_liquidation_penalty_is_split_three_ways() {
    let (mut env, user) = env_with_position(Direction::Long, 250 * ONE_USDC);
    let (liq, liq_token) = env.new_liquidator();

    let insurance_before = env.insurance_state().balance;
    let treasury_before = env.token_balance(&env.fee_vault);

    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(107_043_000).conf(13_893));
    env.liquidate(&user, 0, 0, &liq, liq_token, price).unwrap();
    env.assert_invariants();

    let reward = env.token_balance(&liq_token);
    assert!(reward > 0, "liquidator share");
    assert!(
        env.insurance_state().balance > insurance_before,
        "the insurance fund must take its share of every penalty — that is how it grows"
    );
    assert!(
        env.token_balance(&env.fee_vault) > treasury_before,
        "treasury share"
    );
}

/// An underwater position must stay liquidatable while the market is halted. A halt that
/// also stopped liquidations would convert an oracle incident into a solvency incident.
#[test]
fn a_halted_market_still_permits_liquidation() {
    let (mut env, user) = env_with_position(Direction::Long, 250 * ONE_USDC);
    let (liq, liq_token) = env.new_liquidator();

    let guardian = env.guardian.insecure_clone();
    let ix = env.halt_market_ix(0);
    env.send(ix, &[&guardian]).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);

    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(107_043_000).conf(13_893));
    env.liquidate(&user, 0, 0, &liq, liq_token, price)
        .expect("a halted market must still permit liquidation");
    env.assert_invariants();
}

/// Threat T8: liquidating your own position must never be profitable. The penalty always
/// exceeds the reward, so the owner is net worse off.
#[test]
fn self_liquidation_is_not_profitable() {
    let (mut env, user) = env_with_position(Direction::Long, 250 * ONE_USDC);

    // The owner acts as their own liquidator.
    let own_token = user.token_account;
    let wallet_before = env.token_balance(&own_token);
    let free_before = env.user_state(&user).free_collateral;
    let margin = env.position_state(&user, 0, 0).collateral;

    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(107_043_000).conf(13_893));
    let ix = env.liquidate_ix(&user, 0, 0, user.pubkey(), own_token, price, None, None);
    let kp = user.keypair.insecure_clone();
    env.send(ix, &[&kp]).unwrap();
    env.assert_invariants();

    let recovered = (env.token_balance(&own_token) - wallet_before)
        + (env.user_state(&user).free_collateral - free_before);
    assert!(
        recovered < margin,
        "self-liquidation recovered {recovered} of {margin} — it must always be a net loss \
         to the owner (threat T8)"
    );
}

// --- the bad-debt waterfall (§ 6.9) --------------------------------------------------------

/// A gap straight through the liquidation price leaves the position owing more than its
/// margin. The insurance fund covers the shortfall so LPs do not.
#[test]
fn the_insurance_fund_absorbs_bad_debt() {
    let (mut env, user) = env_with_position(Direction::Long, 250 * ONE_USDC);
    let (liq, liq_token) = env.new_liquidator();

    let insurance_before = env.insurance_state().balance;

    // −500 pips in one step: ~$500 of loss against $250 of margin. No liquidation engine
    // outruns a gap — the only defence is a fund that absorbs it.
    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(103_543_000).conf(13_893));
    env.liquidate(&user, 0, 0, &liq, liq_token, price).unwrap();
    env.assert_invariants();

    assert!(
        env.protocol_state().total_bad_debt > 0,
        "a gap through the liquidation price must be recorded as bad debt"
    );
    assert!(
        env.insurance_state().balance < insurance_before,
        "the insurance fund must have paid out"
    );
    assert!(
        env.insurance_state().total_bad_debt_covered > 0,
        "the fund must record what it covered"
    );
    assert_eq!(
        env.market_state(0).pending_adl_debt,
        0,
        "a funded insurance fund should leave nothing for ADL to clear"
    );
}

/// With the insurance fund empty, the shortfall queues for auto-deleveraging instead.
#[test]
fn an_empty_insurance_fund_queues_the_shortfall_for_adl() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    // Deliberately unfunded — § 6.9 warns never to launch this way, so the behaviour when
    // someone does needs to be defined rather than discovered.

    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        250 * ONE_USDC,
        NO_BOUND_BUY,
        price,
    )
    .unwrap();

    let (liq, liq_token) = env.new_liquidator();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(103_543_000).conf(13_893));
    env.liquidate(&user, 0, 0, &liq, liq_token, price).unwrap();
    env.assert_invariants();

    assert!(
        env.market_state(0).pending_adl_debt > 0,
        "with no insurance, the shortfall must queue for ADL rather than vanish"
    );
}

/// ADL takes profit from a winner, never principal, and only while a shortfall is unpaid.
#[test]
fn auto_deleveraging_socialises_a_shortfall_onto_a_winner() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);

    let loser = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    let winner = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&loser, 50_000 * ONE_USDC).unwrap();
    env.deposit(&winner, 50_000 * ONE_USDC).unwrap();

    // A long that will gap, and a short that will profit from the same move.
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &loser,
        0,
        0,
        Direction::Long,
        MINI,
        250 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &winner,
        0,
        0,
        Direction::Short,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_SELL,
        p,
    )
    .unwrap();
    env.assert_invariants();

    // The gap.
    let crashed = env.post_price(FEED_EUR_USD, PriceSpec::at(103_543_000).conf(13_893));
    let (liq, liq_token) = env.new_liquidator();
    env.liquidate(&loser, 0, 0, &liq, liq_token, crashed)
        .unwrap();
    env.assert_invariants();

    let debt = env.market_state(0).pending_adl_debt;
    assert!(debt > 0, "expected an uncovered shortfall");

    let winner_margin = env.position_state(&winner, 0, 0).collateral;
    let free_before = env.user_state(&winner).free_collateral;

    let p = env.post_price(FEED_EUR_USD, PriceSpec::at(103_543_000).conf(13_893));
    env.auto_deleverage(&winner, 0, 0, p).unwrap();
    env.assert_invariants();

    let paid = env.user_state(&winner).free_collateral - free_before;
    assert!(
        paid >= winner_margin,
        "ADL must never take principal: paid {paid} against margin {winner_margin}"
    );
    assert!(
        env.market_state(0).pending_adl_debt < debt,
        "the shortfall must be drawn down"
    );
    assert!(!env.position_exists(&winner, 0, 0));
}

#[test]
fn auto_deleveraging_is_rejected_when_nothing_is_owed() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let price = env.post_price(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));

    assert_err_contains(
        env.auto_deleverage(&user, 0, 0, price),
        "no uncovered bad debt",
    );
    env.assert_invariants();
}

#[test]
fn anyone_can_top_up_the_insurance_fund() {
    let mut env = Env::new();
    env.init_protocol();
    assert_eq!(env.insurance_state().balance, 0);

    env.seed_insurance(25_000 * ONE_USDC);
    assert_eq!(env.insurance_state().balance, 25_000 * ONE_USDC);
    env.assert_i6();
}

// --- funding and carry (§ 6.7) --------------------------------------------------------------

/// Carry is interest on borrowed notional, charged in both directions. Its index only ever
/// increases.
#[test]
fn carry_accrues_over_time() {
    let (mut env, _user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);

    // EUR at 2%, USD at 4%, plus a markup — FOREX-EXPLAINED.md § 9's worked example.
    env.patch_market(0, |m| {
        m.rate_base_annual = 20_000_000; // 2%
        m.rate_quote_annual = 40_000_000; // 4%
        m.carry_rate_per_hour = 1_000; // markup
        m.last_funding_update_ts = env_now();
    });

    let before = env.market_state(0).cum_borrow_index;
    env.advance_clock(3_600);
    env.crank_funding(0).unwrap();

    assert!(
        env.market_state(0).cum_borrow_index > before,
        "an hour of carry must move the index"
    );
    env.assert_invariants();
}

/// Funding is zero while only one side of the book is populated. There is nobody to pay, and
/// charging anyway would route trader money into LP capital — exactly what I3 forbids.
#[test]
fn funding_is_zero_on_a_one_sided_book() {
    let (mut env, _user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    env.patch_market(0, |m| {
        m.funding_rate_k = 100_000_000;
        m.last_funding_update_ts = env_now();
    });

    env.advance_clock(3_600);
    env.crank_funding(0).unwrap();

    let m = env.market_state(0);
    assert_eq!(
        m.cum_funding_long, 0,
        "no shorts to pay, so longs must not be charged"
    );
    assert_eq!(m.cum_funding_short, 0);
}

/// With both sides populated and the book skewed long, longs pay and shorts receive.
#[test]
fn a_skewed_book_makes_the_heavy_side_pay() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);

    let long = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    let short = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&long, 50_000 * ONE_USDC).unwrap();
    env.deposit(&short, 50_000 * ONE_USDC).unwrap();

    let p = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &long,
        0,
        0,
        Direction::Long,
        MINI * 4,
        20_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &short,
        0,
        0,
        Direction::Short,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_SELL,
        p,
    )
    .unwrap();

    env.patch_market(0, |m| {
        m.funding_rate_k = 100_000_000; // 10% per hour at full skew
        m.last_funding_update_ts = env_now();
    });

    env.advance_clock(3_600);
    env.crank_funding(0).unwrap();
    env.assert_invariants();

    let m = env.market_state(0);
    assert!(
        m.cum_funding_long > 0,
        "the heavy side must pay: cum_funding_long {}",
        m.cum_funding_long
    );
    assert!(
        m.cum_funding_short < 0,
        "the light side must receive: cum_funding_short {}",
        m.cum_funding_short
    );
}

/// Funding must never touch LP capital (invariant I3). It moves between positions inside the
/// collateral vault and is held in `Market::funding_balance` until claimed.
#[test]
fn funding_never_touches_lp_capital() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);

    let long = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    let short = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&long, 50_000 * ONE_USDC).unwrap();
    env.deposit(&short, 50_000 * ONE_USDC).unwrap();

    let p = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &long,
        0,
        0,
        Direction::Long,
        MINI * 4,
        20_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &short,
        0,
        0,
        Direction::Short,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_SELL,
        p,
    )
    .unwrap();

    env.patch_market(0, |m| {
        m.funding_rate_k = 100_000_000;
        m.last_funding_update_ts = env_now();
    });
    env.advance_clock(3_600);

    let aum_before = env.lp_state().aum;
    env.crank_funding(0).unwrap();
    assert_eq!(
        env.lp_state().aum,
        aum_before,
        "cranking funding must not move LP capital at all (I3)"
    );

    // Closing the paying side moves funding into the market's pool, not the LP vault.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.close(&long, 0, 0, NO_BOUND_SELL, p).unwrap();
    env.assert_invariants();
}

/// Redundant keepers are the design (§ 9.2 wants three instances). A second crank in the
/// same second must be a no-op, not a failure that kills the other keeper's transaction.
#[test]
fn a_repeated_funding_crank_in_the_same_second_is_a_no_op() {
    let (mut env, _user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    env.patch_market(0, |m| m.last_funding_update_ts = env_now());
    env.advance_clock(3_600);

    env.crank_funding(0).unwrap();
    let after_first = env.market_state(0).cum_borrow_index;
    env.crank_funding(0)
        .expect("a redundant crank must succeed");
    assert_eq!(
        env.market_state(0).cum_borrow_index,
        after_first,
        "the second crank must change nothing"
    );
}

/// Carry is settled out of the position at close, and it is revenue: it reaches the LP vault
/// and the treasury rather than staying with the trader.
#[test]
fn carry_is_charged_when_the_position_closes() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    env.patch_market(0, |m| {
        m.rate_base_annual = 0;
        m.rate_quote_annual = 200_000_000; // 20% — large enough to be unmistakable
        m.carry_rate_per_hour = 0;
        m.last_funding_update_ts = env_now();
    });

    env.advance_clock(24 * 3_600);
    env.crank_funding(0).unwrap();

    let free_before = env.user_state(&user).free_collateral;
    let treasury_before = env.token_balance(&env.fee_vault);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.close(&user, 0, 0, NO_BOUND_SELL, p).unwrap();
    env.assert_invariants();

    let returned = env.user_state(&user).free_collateral - free_before;
    assert!(
        returned < 5_000 * ONE_USDC,
        "a day of carry at 20% must reduce what comes back: got {returned}"
    );
    assert!(
        env.token_balance(&env.fee_vault) > treasury_before,
        "carry is revenue and must reach the treasury"
    );
}

// --- regimes (§ 7.3) --------------------------------------------------------------------------

/// **The C-1 test, end to end.** A feed that has stopped publishing halts the market, and a
/// halted market refuses new positions. This is the hole that drains the vault.
#[test]
fn a_dead_feed_halts_the_market_and_blocks_new_positions() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    // No price account at all: the feed has stopped.
    env.crank_session(0, None).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "a feed that is not publishing must halt the market"
    );

    let stale = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.open(
            &user,
            0,
            0,
            Direction::Long,
            MINI,
            1_000 * ONE_USDC,
            NO_BOUND_BUY,
            stale,
        ),
        "Market does not permit opening positions",
    );
    env.assert_invariants();
}

/// A live feed **outside session hours** still halts. The calendar is an independent second
/// signal and either one closes the market.
#[test]
fn a_live_feed_on_a_saturday_still_halts_the_market() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    // Saturday 2026-08-01, 12:00 UTC — the day Phase 0b measured every metals feed frozen.
    let saturday = 1_785_542_400 + 12 * 3_600;
    env.set_clock(saturday);

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(saturday));
    env.crank_session(0, Some(price)).unwrap();

    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "the interbank week is closed on Saturday whatever the feed says"
    );
}

/// The full weekly cycle: open midweek, wind down before the close, halt over the weekend,
/// reopen through a gap window, then resume.
#[test]
fn a_market_walks_the_full_weekly_cycle() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let saturday = 1_785_542_400;
    let wednesday = saturday - 3 * 86_400 + 12 * 3_600;

    // Midweek: open.
    env.set_clock(wednesday);
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(wednesday));
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);

    // Friday 20:55 UTC — five minutes before the 21:00 close.
    let friday_pre_close = saturday - 86_400 + 20 * 3_600 + 55 * 60;
    env.set_clock(friday_pre_close);
    let p = env.post_price(
        FEED_EUR_USD,
        PriceSpec::default().published_at(friday_pre_close),
    );
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::ReduceOnly,
        "stop taking risk that cannot be managed until Monday"
    );

    // Saturday: the feed stops.
    env.set_clock(saturday + 6 * 3_600);
    env.crank_session(0, None).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);

    // Sunday 22:00 UTC: the feed is back and the calendar has opened.
    let sunday_open = saturday + 86_400 + 22 * 3_600;
    env.set_clock(sunday_open);
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(sunday_open));
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::GapWindow,
        "reopening goes through a gap window, not straight to normal trading"
    );

    // Five minutes later the window clears.
    env.set_clock(sunday_open + 6 * 60);
    let p = env.post_price(
        FEED_EUR_USD,
        PriceSpec::default().published_at(sunday_open + 6 * 60),
    );
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
}

/// Phase 0b measured USD/BRL trading 14:00–21:00 UTC. Outside that window the market must
/// halt even midweek — the C-1 exploit on the pairs where a devaluation gap is most likely.
#[test]
fn an_em_market_halts_outside_its_local_hours() {
    let mut env = Env::new();
    env.init_protocol();

    let mut spec = MarketSpec::eur_usd();
    spec.params.symbol = "USDBRL".into();
    spec.params.session_open_dow = 3; // Wednesday
    spec.params.session_open_seconds = 14 * 3_600;
    spec.params.session_close_dow = 3;
    spec.params.session_close_seconds = 21 * 3_600;
    env.list_and_activate(0, &spec);

    let saturday = 1_785_542_400;
    let wednesday = saturday - 3 * 86_400;

    // 16:00 — inside the local window.
    let inside = wednesday + 16 * 3_600;
    env.set_clock(inside);
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(inside));
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);

    // 23:00 — the local market has closed, though the interbank week has not.
    let outside = wednesday + 23 * 3_600;
    env.set_clock(outside);
    let p = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(outside));
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "an EM pair follows its own local hours, not the interbank week"
    );
}

/// § 7.3 defines `Halted` as *"nothing but liquidations of already-underwater positions.
/// Positions frozen."* — so a voluntary close is refused while a liquidation is not.
///
/// The asymmetry is deliberate and worth stating plainly, because it is the one place the
/// protocol holds a position against its owner's wishes. A halt happens when the oracle is
/// dislocated or the session has ended; letting someone close *into* that price would let
/// them realise a number nobody can verify. A liquidation at the same price is different:
/// it is not optional, and refusing it converts a bad position into bad debt.
///
/// `ReduceOnly` is the state that permits exits — see the Phase 3 suite.
#[test]
fn a_halted_market_freezes_voluntary_exits_but_not_liquidation() {
    let (mut env, user) = env_with_position(Direction::Long, 250 * ONE_USDC);

    env.crank_session(0, None).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);

    let p = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(env.close(&user, 0, 0, NO_BOUND_SELL, p), "Market is halted");

    // But an underwater position can still be liquidated.
    let (liq, liq_token) = env.new_liquidator();
    let crashed = env.post_price(FEED_EUR_USD, PriceSpec::at(107_043_000).conf(13_893));
    env.liquidate(&user, 0, 0, &liq, liq_token, crashed)
        .expect("a halt must never block a liquidation");
    env.assert_invariants();
}

/// Crypto has no session, so it never halts on the calendar — only on its feed.
#[test]
fn a_crypto_market_ignores_the_calendar_but_not_its_feed() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::btc_usd());

    let saturday = 1_785_542_400 + 12 * 3_600;
    env.set_clock(saturday);

    let p = env.post_price(
        FEED_BTC_USD,
        PriceSpec::at(9_500_000_000_000)
            .conf(2_000_000_000)
            .published_at(saturday),
    );
    env.crank_session(0, Some(p)).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Active,
        "crypto trades on a Saturday"
    );

    env.crank_session(0, None).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "24/7 is a property of the feed, not a promise the protocol makes on its behalf"
    );
}

/// Helper: the harness clock, for `patch_market` calls that need to reset a crank timestamp.
fn env_now() -> i64 {
    T0
}
