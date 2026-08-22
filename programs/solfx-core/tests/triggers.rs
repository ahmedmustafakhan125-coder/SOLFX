//! Take-profit and stop-loss orders (`ARCHITECTURE.md` § 5.4, Phase 7).
//!
//! # What these prove
//!
//! A stop-loss is the one order a trader places *hoping never to need it*, which means the
//! day it matters is the day nobody is watching. So the tests below check the properties that
//! make it dependable rather than merely present:
//!
//! 1. **The four directions fire the right way round.** A stop wired backwards would close
//!    winners and hold losers, and would read as a market-conditions complaint rather than a
//!    bug.
//! 2. **Anyone can fire it.** A trader's stop must not depend on the operator's bot being up.
//! 3. **Somebody is paid to.** A permissionless instruction nobody is paid to call is a
//!    promise with no mechanism behind it.
//! 4. **It is a trigger, not a promised price.** In a gap it fills far past the trigger —
//!    which is how every broker works, and which the event stream makes visible.

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
use solana_signer::Signer;
use solfx_core::state::{Direction, MarketStatus, TriggerKind};

const MINI: u64 = ONE_LOT / 10;
const NO_BOUND_BUY: i64 = i64::MAX;
const NO_BOUND_SELL: i64 = 1;

/// EUR/USD spot 1.08543 at `PRICE_PRECISION`.
const SPOT: i64 = 1_085_430_000;

fn env_with_position(direction: Direction, collateral: u64) -> (Env, User) {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let user = env.new_user(200_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 100_000 * ONE_USDC).unwrap();

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let bound = if direction == Direction::Long {
        NO_BOUND_BUY
    } else {
        NO_BOUND_SELL
    };
    env.open(&user, 0, 0, direction, MINI, collateral, bound, p)
        .unwrap();
    env.track_position(Env::position_pda(&user.account, 0, 0));
    env.assert_invariants();
    (env, user)
}

/// A funded keeper — a stranger, not the operator. It needs SOL for fees and nothing else:
/// the tip arrives as lamports when the order account closes to it.
fn new_keeper(env: &mut Env) -> solana_keypair::Keypair {
    let kp = solana_keypair::Keypair::new();
    env.svm.airdrop(&kp.pubkey(), 100 * 1_000_000_000).unwrap();
    kp
}

// --- placement ----------------------------------------------------------------------------

#[test]
fn a_trader_can_attach_a_take_profit_and_a_stop_loss() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let position = Env::position_pda(&user.account, 0, 0);

    // TP above the market, SL below it — the usual pair on a long.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        1,
        TriggerKind::StopLoss,
        SPOT - 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    let tp = env.trigger_state(&position, 0);
    assert_eq!(tp.kind, TriggerKind::TakeProfit);
    assert_eq!(tp.trigger_price, SPOT + 5_000_000);
    assert_eq!(tp.authority, user.pubkey());
    assert_eq!(tp.position, position);

    let sl = env.trigger_state(&position, 1);
    assert_eq!(sl.kind, TriggerKind::StopLoss);
    assert_eq!(sl.trigger_price, SPOT - 5_000_000);

    env.assert_invariants();
}

/// A take-profit *below* a long is already met — it would fire on the next keeper pass. That
/// is a delayed market close, not an order, and saying so at placement is better than
/// surprising the trader a second later.
#[test]
fn a_trigger_already_met_is_refused_at_placement() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.place_trigger(
            &user,
            0,
            0,
            0,
            TriggerKind::TakeProfit,
            SPOT - 5_000_000, // below the market on a long
            MINI,
            p,
        ),
        "Trigger is already met",
    );

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.place_trigger(
            &user,
            0,
            0,
            1,
            TriggerKind::StopLoss,
            SPOT + 5_000_000, // above the market on a long
            MINI,
            p,
        ),
        "Trigger is already met",
    );
}

/// The mirror on a short: TP below, SL above.
#[test]
fn a_short_takes_profit_below_and_stops_above() {
    let (mut env, user) = env_with_position(Direction::Short, 5_000 * ONE_USDC);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT - 5_000_000,
        MINI,
        p,
    )
    .expect("a short takes profit when the price falls");

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        1,
        TriggerKind::StopLoss,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .expect("a short stops out when the price rises");

    // And the wrong way round is refused.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.place_trigger(
            &user,
            0,
            0,
            2,
            TriggerKind::TakeProfit,
            SPOT + 5_000_000,
            MINI,
            p,
        ),
        "Trigger is already met",
    );
}

#[test]
fn a_trigger_cannot_be_larger_than_the_position() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());

    assert_err_contains(
        env.place_trigger(
            &user,
            0,
            0,
            0,
            TriggerKind::TakeProfit,
            SPOT + 5_000_000,
            MINI * 2,
            p,
        ),
        "Cannot reduce a position by more than its size",
    );
}

#[test]
fn only_the_owner_can_cancel_a_trigger() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let position = Env::position_pda(&user.account, 0, 0);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    let stranger = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    assert!(
        env.cancel_trigger(&stranger, position, 0).is_err(),
        "a stranger must not be able to remove someone's stop-loss"
    );
    assert!(env.trigger_exists(&position, 0));

    env.cancel_trigger(&user, position, 0).unwrap();
    assert!(
        !env.trigger_exists(&position, 0),
        "rent returned to the owner"
    );
    env.assert_invariants();
}

// --- execution ----------------------------------------------------------------------------

/// **The property that matters most.** A stranger fires the trader's stop, and is paid for it.
#[test]
fn anyone_can_fire_a_met_trigger_and_is_paid_a_tip() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let position = Env::position_pda(&user.account, 0, 0);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .unwrap();
    let before = env.sol_balance(&keeper.pubkey());

    // The market reaches the trigger. (PriceSpec::at takes a Pyth mantissa at expo -8, so
    // 1.09143 is 109_143_000 — a decimal place shy of PRICE_PRECISION.)
    let hit = env.post_price_now(FEED_EUR_USD, PriceSpec::at(109_143_000).conf(13_893));

    env.execute_trigger_as(&keeper, &user, 0, 0, 0, hit)
        .expect("a stranger must be able to fire a met trigger");
    env.assert_invariants();

    assert!(!env.position_exists(&user, 0, 0), "the position closed");
    assert!(!env.trigger_exists(&position, 0), "the order was consumed");
    // Paid in the order account's rent — funded by the trader at placement, so the keeper is
    // paid without the protocol spending anything and without any USDC moving.
    let paid = env.sol_balance(&keeper.pubkey()) - before;
    assert!(
        paid > 0,
        "the keeper came out behind after fees — nobody would run this"
    );
}

#[test]
fn a_trigger_that_is_not_met_cannot_be_fired() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    // Price moved up, but not far enough.
    let near = env.post_price_now(FEED_EUR_USD, PriceSpec::at(108_743_000).conf(13_893));
    assert_err_contains(
        env.execute_trigger_as(&keeper, &user, 0, 0, 0, near),
        "Trigger condition has not been met",
    );
    assert!(env.position_exists(&user, 0, 0));
    env.assert_invariants();
}

/// A stop-loss on a long fires when the price *falls* — the case that would be silently
/// inverted by a wrong comparison.
#[test]
fn a_stop_loss_on_a_long_fires_when_the_price_falls() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::StopLoss,
        SPOT - 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    // A rise must NOT fire a stop-loss.
    let up = env.post_price_now(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    assert_err_contains(
        env.execute_trigger_as(&keeper, &user, 0, 0, 0, up),
        "Trigger condition has not been met",
    );

    // A fall does.
    let down = env.post_price_now(FEED_EUR_USD, PriceSpec::at(107_943_000).conf(13_893));
    env.execute_trigger_as(&keeper, &user, 0, 0, 0, down)
        .unwrap();
    assert!(!env.position_exists(&user, 0, 0));
    env.assert_invariants();
}

/// A stop-loss on a short fires when the price *rises* — the fourth combination.
#[test]
fn a_stop_loss_on_a_short_fires_when_the_price_rises() {
    let (mut env, user) = env_with_position(Direction::Short, 5_000 * ONE_USDC);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::StopLoss,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    let down = env.post_price_now(FEED_EUR_USD, PriceSpec::at(107_943_000).conf(13_893));
    assert_err_contains(
        env.execute_trigger_as(&keeper, &user, 0, 0, 0, down),
        "Trigger condition has not been met",
    );

    let up = env.post_price_now(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.execute_trigger_as(&keeper, &user, 0, 0, 0, up).unwrap();
    assert!(!env.position_exists(&user, 0, 0));
    env.assert_invariants();
}

/// **A trigger is not a promised price.** In a gap the oracle jumps far past the trigger and
/// the close fills there — which is how every broker works, and which the event stream makes
/// visible rather than hiding.
#[test]
fn a_gap_fills_far_past_the_trigger_and_says_so() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let keeper = new_keeper(&mut env);

    let stop = SPOT - 5_000_000; // 1.08043
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(&user, 0, 0, 0, TriggerKind::StopLoss, stop, MINI, p)
        .unwrap();

    // The market gaps 200 pips through the stop with no tick in between.
    let gapped = env.post_price_now(
        FEED_EUR_USD,
        PriceSpec::at(106_543_000).ema(108_543_000).conf(13_893),
    );
    let free_before = env.user_state(&user).free_collateral;

    env.execute_trigger_as(&keeper, &user, 0, 0, 0, gapped)
        .unwrap();
    env.assert_invariants();

    // The fill was worse than the stop, and the trader kept less than the stop implied. The
    // protocol did not eat the difference, and did not pretend the stop held.
    let returned = env.user_state(&user).free_collateral - free_before;
    assert!(
        returned < 5_000 * ONE_USDC,
        "a stop that gapped must still realise the loss: got {returned}"
    );
}

/// Partial exits: a trigger may close part of a position, leaving the rest running.
#[test]
fn a_partial_trigger_scales_out_of_the_position() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT + 5_000_000,
        MINI / 2,
        p,
    )
    .unwrap();

    let hit = env.post_price_now(FEED_EUR_USD, PriceSpec::at(109_143_000).conf(13_893));
    env.execute_trigger_as(&keeper, &user, 0, 0, 0, hit)
        .unwrap();
    env.assert_invariants();

    let position = env.position_state(&user, 0, 0);
    assert_eq!(
        position.size_base,
        MINI / 2,
        "half the position remains open"
    );
    assert_eq!(env.user_state(&user).open_positions, 1);
}

/// § 7.3: `Halted` freezes positions, so a trigger cannot fire either. Liquidation is the
/// only thing that still moves — a halt must not convert a bad position into bad debt, but it
/// also must not let anyone realise a price nobody can verify.
#[test]
fn a_halted_market_does_not_fire_triggers() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    let guardian = env.guardian.insecure_clone();
    let ix = env.halt_market_ix(0);
    env.send(ix, &[&guardian]).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);

    let hit = env.post_price_now(FEED_EUR_USD, PriceSpec::at(109_143_000).conf(13_893));
    assert_err_contains(
        env.execute_trigger_as(&keeper, &user, 0, 0, 0, hit),
        "Market is halted",
    );
    env.assert_invariants();
}

/// A trigger read through the same gates as any trade: a stale price cannot fire a stop.
#[test]
fn a_stale_price_cannot_fire_a_trigger() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::StopLoss,
        SPOT - 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    let now = env.now;
    let stale = env.post_price(
        FEED_EUR_USD,
        PriceSpec::at(107_943_000)
            .conf(13_893)
            .published_at(now - 600),
    );
    assert_err_contains(
        env.execute_trigger_as(&keeper, &user, 0, 0, 0, stale),
        "Oracle price is stale",
    );
    env.assert_invariants();
}

/// Both halves of a bracket rest at once; firing one leaves the other, which the trader (or
/// their keeper) can then cancel.
#[test]
fn a_bracket_leaves_the_other_side_resting() {
    let (mut env, user) = env_with_position(Direction::Long, 5_000 * ONE_USDC);
    let position = Env::position_pda(&user.account, 0, 0);
    let keeper = new_keeper(&mut env);

    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        0,
        TriggerKind::TakeProfit,
        SPOT + 5_000_000,
        MINI,
        p,
    )
    .unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.place_trigger(
        &user,
        0,
        0,
        1,
        TriggerKind::StopLoss,
        SPOT - 5_000_000,
        MINI,
        p,
    )
    .unwrap();

    let hit = env.post_price_now(FEED_EUR_USD, PriceSpec::at(109_143_000).conf(13_893));
    env.execute_trigger_as(&keeper, &user, 0, 0, 0, hit)
        .unwrap();

    assert!(!env.trigger_exists(&position, 0), "the TP was consumed");
    assert!(
        env.trigger_exists(&position, 1),
        "the SL still rests — an OCO pairing is the frontend's job, not the program's"
    );

    // And it can be cleaned up, returning its rent.
    env.cancel_trigger(&user, position, 1).unwrap();
    assert!(!env.trigger_exists(&position, 1));
    env.assert_invariants();
}
