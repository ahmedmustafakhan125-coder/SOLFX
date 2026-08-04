//! The position lifecycle, end to end (`ARCHITECTURE.md` § 6, § 15 Phase 3).
//!
//! Exit criterion: *"Lifecycle tested on a direct pair, a synthetic cross **and** a
//! non-USD-quoted pair; I1–I5 hold."*
//!
//! Invariants are re-checked after **every** instruction rather than at the end of a
//! scenario. An invariant checked once at the end tells you something broke; checked after
//! each step it tells you which step broke it.

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

/// Sizes are chosen per market to land near **$10,000 of notional**, because the same base
/// size is a completely different exposure on gold than on EUR/USD — 0.1 lot is ~$10.8k of
/// EUR/USD but ~$40m of gold. Opening at a sane leverage is what these constants encode.
///
/// EUR/USD at 1.08543: 1e13 base units -> ~$10,854.
const MINI: u64 = ONE_LOT / 10;
/// XAU/USD at 4046.95: 2.5e9 base units -> ~$10,117.
const GOLD_SIZE: u64 = 2_500_000_000;
/// USD/INR at 88.42: 1e12 base units -> ₹88,420 -> ~$1,000 after conversion.
const INR_SIZE: u64 = 1_000_000_000_000;

/// EUR/USD at 1.08543, the measured p50 tick. No slippage bound.
const NO_BOUND_BUY: i64 = i64::MAX;
const NO_BOUND_SELL: i64 = 1;

fn eur_env() -> (Env, User) {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();
    env.assert_invariants();
    (env, user)
}

// --- the happy path ----------------------------------------------------------------------

#[test]
fn a_long_opens_with_the_expected_entry_margin_and_fee() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let before = env.user_state(&user).free_collateral;

    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        price,
    )
    .unwrap();
    env.assert_invariants();

    let p = env.position_state(&user, 0, 0);
    assert_eq!(p.size_base, MINI);
    assert_eq!(p.collateral, 1_000 * ONE_USDC);
    assert_eq!(p.direction, Direction::Long);

    // Opening a long is a buy, so the fill must be **above** the oracle mid (§ 6.6).
    assert!(
        p.entry_price > 1_085_430_000,
        "a long filled at or below the mid — the adverse spread was not applied: {}",
        p.entry_price
    );

    // 0.1 lot at ~1.0854 is ~$10,854 notional.
    assert!(
        (10_800 * ONE_USDC..11_000 * ONE_USDC).contains(&p.entry_notional),
        "notional {} outside the expected band",
        p.entry_notional
    );

    // Margin plus fee left free collateral; the fee is small but strictly positive.
    let after = env.user_state(&user).free_collateral;
    let spent = before - after;
    assert!(
        spent > 1_000 * ONE_USDC,
        "no fee was charged: spent {spent}"
    );
    assert!(spent < 1_002 * ONE_USDC, "fee looks far too large: {spent}");

    assert_eq!(env.market_state(0).open_position_count, 1);
    assert_eq!(env.user_state(&user).open_positions, 1);
}

/// Opening a short is a **sell**, so it fills below the mid — the mirror of the long.
#[test]
fn a_short_fills_below_the_mid() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());

    env.open(
        &user,
        0,
        0,
        Direction::Short,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_SELL,
        price,
    )
    .unwrap();
    env.assert_invariants();

    let p = env.position_state(&user, 0, 0);
    assert_eq!(p.direction, Direction::Short);
    assert!(
        p.entry_price < 1_085_430_000,
        "a short filled at or above the mid: {}",
        p.entry_price
    );
}

/// A long that is right: the pool pays.
#[test]
fn a_winning_long_is_paid_by_the_pool() {
    let (mut env, user) = eur_env();
    let open_price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        open_price,
    )
    .unwrap();

    let free_after_open = env.user_state(&user).free_collateral;
    let aum_before = env.lp_state().aum;

    // +100 pips: 1.08543 -> 1.09543.
    let close_price = env.post_price(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.close(&user, 0, 0, NO_BOUND_SELL, close_price).unwrap();
    env.assert_invariants();

    let free_after_close = env.user_state(&user).free_collateral;
    let gained = free_after_close - free_after_open;

    // 0.1 lot × 100 pips = $100, less spread and the close fee.
    assert!(
        (1_090 * ONE_USDC..1_101 * ONE_USDC).contains(&gained),
        "expected ~$1,100 back (margin + ~$100 profit), got {gained}"
    );

    assert!(
        env.lp_state().aum < aum_before,
        "the pool must have paid: aum {} was {}",
        env.lp_state().aum,
        aum_before
    );
    assert!(!env.position_exists(&user, 0, 0), "position was not closed");
    assert_eq!(env.market_state(0).open_position_count, 0);
}

/// A long that is wrong: the pool receives. Same mechanic, opposite sign.
#[test]
fn a_losing_long_pays_the_pool() {
    let (mut env, user) = eur_env();
    let open_price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        open_price,
    )
    .unwrap();

    let free_after_open = env.user_state(&user).free_collateral;
    let aum_before = env.lp_state().aum;

    // −100 pips.
    let close_price = env.post_price(FEED_EUR_USD, PriceSpec::at(107_543_000).conf(13_893));
    env.close(&user, 0, 0, NO_BOUND_SELL, close_price).unwrap();
    env.assert_invariants();

    let returned = env.user_state(&user).free_collateral - free_after_open;
    assert!(
        (890 * ONE_USDC..910 * ONE_USDC).contains(&returned),
        "expected ~$900 back (margin less ~$100 loss), got {returned}"
    );
    assert!(
        env.lp_state().aum > aum_before,
        "the pool must have received"
    );
}

// --- the test that matters most ----------------------------------------------------------

/// **Open and immediately close at an unchanged oracle price must always lose money.**
///
/// If this ever passes, there is free money in the protocol and bots will extract it until
/// the vault is empty. `solfx-math` property-tests the arithmetic across the parameter
/// space; this proves it holds through the *real instructions*, where the spread, both fees
/// and the vault plumbing all have to line up.
///
/// Run on every market shape, because the conversion path (C-3) is exactly where a sign or a
/// rounding direction could flip.
#[test]
fn a_round_trip_at_an_unchanged_price_always_loses() {
    for (label, spec, feed, size) in [
        (
            "direct USD-quoted",
            MarketSpec::eur_usd(),
            FEED_EUR_USD,
            MINI,
        ),
        ("metals", MarketSpec::xau_usd(), FEED_XAU_USD, GOLD_SIZE),
    ] {
        for direction in [Direction::Long, Direction::Short] {
            let mut env = Env::new();
            env.init_protocol();
            env.list_and_activate(0, &spec);
            env.seed_pool(1_000_000 * ONE_USDC);
            let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
            env.deposit(&user, 50_000 * ONE_USDC).unwrap();

            let start = env.user_state(&user).free_collateral;
            let tick = if feed == FEED_XAU_USD {
                PriceSpec::at(404_695_000_000).conf(57_871_400)
            } else {
                PriceSpec::default()
            };

            let open_price = env.post_price(feed, tick);
            env.open(
                &user,
                0,
                0,
                direction,
                size,
                2_000 * ONE_USDC,
                if direction == Direction::Long {
                    NO_BOUND_BUY
                } else {
                    NO_BOUND_SELL
                },
                open_price,
            )
            .unwrap_or_else(|e| panic!("{label} {direction:?}: open failed: {e}"));

            // Same oracle price, a fresh account so the transaction differs.
            let close_price = env.post_price(feed, tick);
            env.close(
                &user,
                0,
                0,
                if direction == Direction::Long {
                    NO_BOUND_SELL
                } else {
                    NO_BOUND_BUY
                },
                close_price,
            )
            .unwrap_or_else(|e| panic!("{label} {direction:?}: close failed: {e}"));

            let end = env.user_state(&user).free_collateral;
            assert!(
                end < start,
                "FREE MONEY on {label} {direction:?}: started with {start}, \
                 ended with {end} after a round trip at an unchanged price"
            );
            env.assert_invariants();
        }
    }
}

// --- non-USD-quoted: correction C-3 -------------------------------------------------------

/// USD/INR is the shape that makes C-3 mandatory. PnL lands in Rupees; booking it as USDC
/// would mis-price every position by ~88x.
///
/// The check is the *magnitude* of the margin required: an 88x error would be unmissable.
#[test]
fn a_non_usd_quoted_market_sizes_in_usdc_not_in_quote_currency() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::usd_inr());
    env.seed_pool(1_000_000 * ONE_USDC);
    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    // USD/INR at 88.42.
    let tick = PriceSpec::at(8_842_000_000).conf(5_180_000);
    let price = env.post_price(FEED_USD_INR, tick);
    let conv = env.post_price(FEED_USD_INR, tick);

    // Notional in USDC must be ~$1,000, *not* ~₹88,420.
    let size = INR_SIZE;
    let ix = env.open_ix(
        &user,
        0,
        0,
        Direction::Long,
        size,
        500 * ONE_USDC,
        NO_BOUND_BUY,
        price,
        None,
        Some(conv),
    );
    let kp = user.keypair.insecure_clone();
    env.send(ix, &[&kp]).expect("open on USD/INR failed");
    env.track_position(Env::position_pda(&user.account, 0, 0));
    env.assert_invariants();

    let p = env.position_state(&user, 0, 0);
    assert!(
        (900 * ONE_USDC..1_100 * ONE_USDC).contains(&p.entry_notional),
        "notional {} is not ~$1,000 — the quote conversion was skipped, and an 88x \
         mis-pricing is exactly what correction C-3 exists to prevent",
        p.entry_notional
    );
}

/// A round trip on a converted market must lose money too. The conversion divides both the
/// PnL and the costs, so a sign error there would show up as free money here.
#[test]
fn a_round_trip_on_a_converted_market_also_loses() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::usd_inr());
    env.seed_pool(1_000_000 * ONE_USDC);
    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    let start = env.user_state(&user).free_collateral;
    let tick = PriceSpec::at(8_842_000_000).conf(5_180_000);
    let size = INR_SIZE;
    let kp = user.keypair.insecure_clone();

    let price = env.post_price(FEED_USD_INR, tick);
    let conv = env.post_price(FEED_USD_INR, tick);
    let ix = env.open_ix(
        &user,
        0,
        0,
        Direction::Long,
        size,
        500 * ONE_USDC,
        NO_BOUND_BUY,
        price,
        None,
        Some(conv),
    );
    env.send(ix, &[&kp]).unwrap();
    env.track_position(Env::position_pda(&user.account, 0, 0));

    let price2 = env.post_price(FEED_USD_INR, tick);
    let conv2 = env.post_price(FEED_USD_INR, tick);
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: solfx_core::ID,
        accounts: env.close_metas(&user, 0, 0, price2, None, Some(conv2)),
        data: anchor_lang::InstructionData::data(&solfx_core::instruction::ClosePosition {
            price_limit: NO_BOUND_SELL,
        }),
    };
    env.send(ix, &[&kp]).expect("close on USD/INR failed");

    assert!(
        env.user_state(&user).free_collateral < start,
        "FREE MONEY on a non-USD-quoted market"
    );
    env.assert_invariants();
}

// --- synthetic: both legs ------------------------------------------------------------------

/// EUR/GBP composed from EUR/USD ÷ GBP/USD. Off the v1 critical path, but the lifecycle has
/// to work before it is ever needed.
#[test]
fn a_synthetic_cross_completes_a_full_lifecycle() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_gbp_synthetic());
    env.seed_pool(1_000_000 * ONE_USDC);
    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();
    let kp = user.keypair.insecure_clone();
    let start = env.user_state(&user).free_collateral;

    let eur = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(13_893));
    let gbp = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(17_272));
    let ix = env.open_ix(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        eur,
        Some(gbp),
        None,
    );
    env.send(ix, &[&kp]).expect("open on the cross failed");
    env.track_position(Env::position_pda(&user.account, 0, 0));
    env.assert_invariants();

    // EUR/GBP ≈ 0.85433, so the entry sits near there rather than near either leg.
    let p = env.position_state(&user, 0, 0);
    assert!(
        (840_000_000..870_000_000).contains(&p.entry_price),
        "entry {} is not a composed cross rate",
        p.entry_price
    );

    let eur2 = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(13_893));
    let gbp2 = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(17_272));
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: solfx_core::ID,
        accounts: env.close_metas(&user, 0, 0, eur2, Some(gbp2), None),
        data: anchor_lang::InstructionData::data(&solfx_core::instruction::ClosePosition {
            price_limit: NO_BOUND_SELL,
        }),
    };
    env.send(ix, &[&kp]).expect("close on the cross failed");
    env.assert_invariants();

    assert!(
        env.user_state(&user).free_collateral < start,
        "FREE MONEY on a synthetic cross"
    );
}

// --- modification --------------------------------------------------------------------------

#[test]
fn increasing_a_position_moves_the_entry_price_against_the_trader() {
    let (mut env, user) = eur_env();
    let p1 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        p1,
    )
    .unwrap();
    let entry_before = env.position_state(&user, 0, 0).entry_price;

    // Add at a higher price: the average must rise.
    let p2 = env.post_price(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.increase(&user, 0, 0, MINI, 1_000 * ONE_USDC, NO_BOUND_BUY, p2)
        .unwrap();
    env.assert_invariants();

    let p = env.position_state(&user, 0, 0);
    assert_eq!(p.size_base, 2 * MINI);
    assert_eq!(p.collateral, 2_000 * ONE_USDC);
    assert!(
        p.entry_price > entry_before,
        "adding at a higher price must raise a long's average entry"
    );
    assert!(
        p.entry_price < 109_543_000 * 10,
        "average entry overshot the second fill"
    );
}

#[test]
fn a_partial_close_realises_a_proportional_share() {
    let (mut env, user) = eur_env();
    let p1 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        p1,
    )
    .unwrap();

    let free_before = env.user_state(&user).free_collateral;

    // Close half at +100 pips: ~$50 of the ~$100 profit.
    let p2 = env.post_price(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.decrease(&user, 0, 0, MINI / 2, NO_BOUND_SELL, p2)
        .unwrap();
    env.assert_invariants();

    let p = env.position_state(&user, 0, 0);
    assert_eq!(p.size_base, MINI / 2);
    assert_eq!(p.collateral, 500 * ONE_USDC, "half the margin was released");

    let returned = env.user_state(&user).free_collateral - free_before;
    assert!(
        (540 * ONE_USDC..555 * ONE_USDC).contains(&returned),
        "expected ~$550 (half the margin plus half the profit), got {returned}"
    );

    // The position survives with collateral — invariant I5.
    assert!(env.position_exists(&user, 0, 0));
    assert!(p.collateral > 0);
}

#[test]
fn margin_can_be_added_and_removed() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        2_000 * ONE_USDC,
        NO_BOUND_BUY,
        price,
    )
    .unwrap();

    let price2 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.add_margin(&user, 0, 0, 500 * ONE_USDC, price2).unwrap();
    env.assert_invariants();
    assert_eq!(env.position_state(&user, 0, 0).collateral, 2_500 * ONE_USDC);

    let price3 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.remove_margin(&user, 0, 0, 500 * ONE_USDC, price3)
        .unwrap();
    env.assert_invariants();
    assert_eq!(env.position_state(&user, 0, 0).collateral, 2_000 * ONE_USDC);
}

/// Removing margin must leave the position meeting the *initial* margin requirement, not
/// merely the maintenance one — otherwise a trader could walk a position to the edge of
/// liquidation and leave it there.
#[test]
fn margin_cannot_be_removed_below_the_initial_requirement() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        price,
    )
    .unwrap();

    let price2 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.remove_margin(&user, 0, 0, 950 * ONE_USDC, price2),
        "Removing this collateral would breach",
    );
    env.assert_invariants();
}

// --- guards ---------------------------------------------------------------------------------

#[test]
fn a_slippage_bound_rejects_a_worse_fill() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());

    // Demand a fill at or below the oracle mid. A long is a buy and fills above it, always.
    assert_err_contains(
        env.open(
            &user,
            0,
            0,
            Direction::Long,
            MINI,
            1_000 * ONE_USDC,
            1_085_430_000,
            price,
        ),
        "Fill price is worse than the slippage bound",
    );
    env.assert_invariants();
}

#[test]
fn leverage_above_the_market_cap_is_rejected() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());

    // ~$10,854 notional on $100 is ~108x, well past the market's 50x.
    assert_err_contains(
        env.open(
            &user,
            0,
            0,
            Direction::Long,
            MINI,
            100 * ONE_USDC,
            NO_BOUND_BUY,
            price,
        ),
        "Leverage exceeds this market's maximum",
    );
    env.assert_invariants();
}

#[test]
fn opening_is_blocked_while_the_protocol_is_paused() {
    let (mut env, user) = eur_env();
    let guardian = env.guardian.insecure_clone();
    let ix = env.pause_ix();
    env.send(ix, &[&guardian]).unwrap();

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.open(
            &user,
            0,
            0,
            Direction::Long,
            MINI,
            1_000 * ONE_USDC,
            NO_BOUND_BUY,
            price,
        ),
        "Protocol is paused",
    );
}

/// A halted market must still let a trader **out**. Freezing an exit is how a protocol turns
/// an oracle incident into a solvency incident.
#[test]
fn a_halted_market_still_permits_closing() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        price,
    )
    .unwrap();

    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::ReduceOnly);
    env.send(ix, &[&admin]).unwrap();

    let price2 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.increase(&user, 0, 0, MINI, 0, NO_BOUND_BUY, price2),
        "Market does not permit opening positions",
    );

    let price3 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.close(&user, 0, 0, NO_BOUND_SELL, price3)
        .expect("a ReduceOnly market must still permit closing");
    env.assert_invariants();
}

/// § 6.6's minimum hold. A trader who opens and closes in the same slot pays two fees but
/// takes no risk; if the spread is ever tighter than the true market spread that is riskless
/// profit, repeated by bots.
#[test]
fn a_minimum_hold_time_blocks_a_same_slot_round_trip() {
    let mut env = Env::new();
    env.init_protocol();
    let mut spec = MarketSpec::eur_usd();
    spec.params.symbol = "EURUSD".into();
    env.list_and_activate(0, &spec);
    env.seed_pool(1_000_000 * ONE_USDC);

    // Turn the guard on: markets ship with it at zero, so this is the retune path too.
    env.patch_market(0, |m| m.min_hold_slots = 5);

    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        price,
    )
    .unwrap();

    let price2 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.close(&user, 0, 0, NO_BOUND_SELL, price2),
        "minimum number of slots",
    );

    env.svm.warp_to_slot(50);
    let price3 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.close(&user, 0, 0, NO_BOUND_SELL, price3)
        .expect("closing after the hold window must succeed");
    env.assert_invariants();
}

#[test]
fn a_position_cannot_be_opened_on_a_stale_price() {
    let (mut env, user) = eur_env();
    let stale = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(T0 - 77_040));

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
        "Oracle price is stale",
    );
    env.assert_invariants();
}

/// Correction C-4 on the trading path. A band the protocol cannot price is a band it must
/// not quote against — at 50x leverage a 2-pip uncertainty is ~4% of a trader's margin, and
/// executing at the mid hands free optionality to anyone with a faster feed.
///
/// The *crank* records such a price and halts the market instead; see
/// `oracle::a_band_wider_than_the_market_ceiling_halts_the_market` for why the two paths
/// differ.
#[test]
fn a_wide_confidence_band_blocks_opening_a_position() {
    let (mut env, user) = eur_env();

    // 30.11 bps against the market's 15 bps ceiling — USD/IDR's measured p95.
    let wide = env.post_price(FEED_EUR_USD, PriceSpec::default().conf(326_823));
    assert_err_contains(
        env.open(
            &user,
            0,
            0,
            Direction::Long,
            MINI,
            1_000 * ONE_USDC,
            NO_BOUND_BUY,
            wide,
        ),
        "Oracle confidence exceeds this market's ceiling",
    );
    env.assert_invariants();
}

#[test]
fn another_trader_cannot_close_your_position() {
    let (mut env, user) = eur_env();
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &user,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        price,
    )
    .unwrap();

    let attacker = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    let price2 = env.post_price(FEED_EUR_USD, PriceSpec::default());

    // The attacker signs, but the position PDA is derived from the victim's user account.
    let mut ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: solfx_core::ID,
        accounts: env.close_metas(&attacker, 0, 0, price2, None, None),
        data: anchor_lang::InstructionData::data(&solfx_core::instruction::ClosePosition {
            price_limit: NO_BOUND_SELL,
        }),
    };
    // Point at the victim's actual position account.
    let victim_position = Env::position_pda(&user.account, 0, 0);
    ix.accounts[4].pubkey = victim_position;

    let kp = attacker.keypair.insecure_clone();
    assert!(
        env.send(ix, &[&kp]).is_err(),
        "a position must only be closeable by the account that owns it"
    );
    env.assert_invariants();
}

// --- open interest (invariant I4) -----------------------------------------------------------

#[test]
fn open_interest_tracks_positions_on_both_sides() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);

    let alice = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    let bob = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&alice, 50_000 * ONE_USDC).unwrap();
    env.deposit(&bob, 50_000 * ONE_USDC).unwrap();

    let p1 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &alice,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        p1,
    )
    .unwrap();
    env.assert_invariants();

    let p2 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &bob,
        0,
        0,
        Direction::Short,
        MINI * 2,
        2_000 * ONE_USDC,
        NO_BOUND_SELL,
        p2,
    )
    .unwrap();
    env.assert_invariants();

    let m = env.market_state(0);
    assert_eq!(m.base_oi_long, i128::from(MINI));
    assert_eq!(m.base_oi_short, i128::from(MINI * 2));
    assert!(m.oi_long > 0 && m.oi_short > 0);
    assert_eq!(m.open_position_count, 2);

    // Closing Alice's long empties that side exactly.
    let p3 = env.post_price(FEED_EUR_USD, PriceSpec::default());
    env.close(&alice, 0, 0, NO_BOUND_SELL, p3).unwrap();
    env.assert_invariants();

    let m = env.market_state(0);
    assert_eq!(
        m.base_oi_long, 0,
        "closing must return base OI to zero exactly"
    );
    assert_eq!(m.base_oi_short, i128::from(MINI * 2));
    assert_eq!(m.open_position_count, 1);
}

#[test]
fn the_open_interest_cap_rejects_an_oversized_book() {
    let mut env = Env::new();
    env.init_protocol();
    let mut spec = MarketSpec::eur_usd();
    spec.params.max_oi_long = 5_000 * ONE_USDC; // ~half a mini lot
    env.list_and_activate(0, &spec);
    env.seed_pool(1_000_000 * ONE_USDC);

    let user = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 50_000 * ONE_USDC).unwrap();

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.open(
            &user,
            0,
            0,
            Direction::Long,
            MINI,
            1_000 * ONE_USDC,
            NO_BOUND_BUY,
            price,
        ),
        "Open interest cap reached",
    );
    env.assert_invariants();
}

// --- several positions, several markets -------------------------------------------------------

/// The invariant sweep across a busy protocol: two traders, two markets, both directions,
/// partial and full closes, with everything re-checked after each step.
#[test]
fn invariants_hold_across_a_busy_multi_market_session() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.list_and_activate(1, &MarketSpec::xau_usd());
    env.seed_pool(2_000_000 * ONE_USDC);

    let alice = env.new_user(200_000 * ONE_USDC, Pubkey::default());
    let bob = env.new_user(200_000 * ONE_USDC, Pubkey::default());
    env.deposit(&alice, 100_000 * ONE_USDC).unwrap();
    env.deposit(&bob, 100_000 * ONE_USDC).unwrap();
    env.assert_invariants();

    let eur = || PriceSpec::default();
    let gold = || PriceSpec::at(404_695_000_000).conf(57_871_400);

    let p = env.post_price(FEED_EUR_USD, eur());
    env.open(
        &alice,
        0,
        0,
        Direction::Long,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();
    env.assert_invariants();

    let p = env.post_price(FEED_XAU_USD, gold());
    env.open(
        &alice,
        1,
        0,
        Direction::Short,
        GOLD_SIZE,
        1_000 * ONE_USDC,
        NO_BOUND_SELL,
        p,
    )
    .unwrap();
    env.assert_invariants();

    let p = env.post_price(FEED_EUR_USD, eur());
    env.open(
        &bob,
        0,
        0,
        Direction::Short,
        MINI,
        1_000 * ONE_USDC,
        NO_BOUND_SELL,
        p,
    )
    .unwrap();
    env.assert_invariants();

    // Bob adds to his short at a better price.
    let p = env.post_price(FEED_EUR_USD, PriceSpec::at(109_000_000).conf(13_893));
    env.increase(&bob, 0, 0, MINI, 1_000 * ONE_USDC, NO_BOUND_SELL, p)
        .unwrap();
    env.assert_invariants();

    // Alice halves her long into a gain.
    let p = env.post_price(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.decrease(&alice, 0, 0, MINI / 2, NO_BOUND_SELL, p)
        .unwrap();
    env.assert_invariants();

    // Alice tops up her gold short.
    let p = env.post_price(FEED_XAU_USD, gold());
    env.add_margin(&alice, 1, 0, 200 * ONE_USDC, p).unwrap();
    env.assert_invariants();

    // Everyone out.
    let p = env.post_price(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.close(&alice, 0, 0, NO_BOUND_SELL, p).unwrap();
    env.assert_invariants();

    let p = env.post_price(FEED_XAU_USD, gold());
    env.close(&alice, 1, 0, NO_BOUND_BUY, p).unwrap();
    env.assert_invariants();

    let p = env.post_price(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.close(&bob, 0, 0, NO_BOUND_BUY, p).unwrap();
    env.assert_invariants();

    // The book is empty and every counter returned to zero.
    for i in 0..2u16 {
        let m = env.market_state(i);
        assert_eq!(m.base_oi_long, 0, "market {i} base_oi_long");
        assert_eq!(m.base_oi_short, 0, "market {i} base_oi_short");
        assert_eq!(m.open_position_count, 0, "market {i} open_position_count");
    }
    assert_eq!(env.user_state(&alice).open_positions, 0);
    assert_eq!(env.user_state(&bob).open_positions, 0);

    // Fees were collected, split and are sitting where they belong.
    assert!(
        env.token_balance(&env.fee_vault) > 0,
        "treasury took nothing"
    );
    assert!(env.insurance_state().balance > 0, "insurance took nothing");
    assert!(
        env.protocol_state().total_referral_accrued > 0,
        "no referral accrual was recorded"
    );
}

/// A trader can hold several positions in the same market, and each is independently
/// collateralised (ADR-004).
#[test]
fn one_trader_can_hold_several_positions_in_one_market() {
    let (mut env, user) = eur_env();

    for nonce in 0..3u8 {
        let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
        let direction = if nonce % 2 == 0 {
            Direction::Long
        } else {
            Direction::Short
        };
        let bound = if direction == Direction::Long {
            NO_BOUND_BUY
        } else {
            NO_BOUND_SELL
        };
        env.open(
            &user,
            0,
            nonce,
            direction,
            MINI,
            1_000 * ONE_USDC,
            bound,
            price,
        )
        .unwrap();
        env.assert_invariants();
    }

    assert_eq!(env.user_state(&user).open_positions, 3);
    assert_eq!(env.market_state(0).open_position_count, 3);

    // Each is its own account with its own margin.
    for nonce in 0..3u8 {
        assert_eq!(
            env.position_state(&user, 0, nonce).collateral,
            1_000 * ONE_USDC
        );
    }
}

// --- liquidity ------------------------------------------------------------------------------

#[test]
fn the_first_liquidity_deposit_mints_one_to_one() {
    let mut env = Env::new();
    env.init_protocol();
    env.seed_pool(500_000 * ONE_USDC);

    let pool = env.lp_state();
    assert_eq!(pool.aum, 500_000 * ONE_USDC);
    assert_eq!(
        pool.lp_token_supply,
        500_000 * ONE_USDC,
        "the first deposit fixes NAV per share at one dollar"
    );
    env.assert_i2();
}

/// **I8**: LP supply and AUM are non-zero together. A pool with shares but no assets, or
/// assets but no shares, is a pool whose NAV is undefined.
#[test]
fn lp_supply_and_aum_are_non_zero_together() {
    let mut env = Env::new();
    env.init_protocol();

    let pool = env.lp_state();
    assert_eq!(pool.aum, 0);
    assert_eq!(pool.lp_token_supply, 0);

    env.seed_pool(1_000 * ONE_USDC);
    let pool = env.lp_state();
    assert!(pool.aum > 0 && pool.lp_token_supply > 0);
    env.assert_i2();
}
