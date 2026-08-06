//! The LP vault (`ARCHITECTURE.md` § 5.4 LP, § 8.1 streams 4–5, threat T6).
//!
//! Phase 5's exit criterion is *"LP accounting exact under adversarial sequences; I2, I8
//! hold."* The adversary is threat T6 — the just-in-time liquidity attack — and the sequences
//! below are that attack, attempted from several directions.
//!
//! # What the attack is
//!
//! An LP watches for a trade the pool is about to win, deposits in front of it, and withdraws
//! as soon as it settles. They capture the pool's edge without ever carrying its risk, and
//! every LP who *did* carry it is diluted. The mirror works too: withdraw ahead of a trade
//! the pool is about to lose.
//!
//! # What stops it
//!
//! Not the exit fee. 0.05% is trivially outrun by a large enough known move. What stops it is
//! that **redemption is priced when it settles, not when it is requested** — so the attacker
//! has to hold through the cooldown, which is exactly the risk they were avoiding.

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
use solfx_core::state::Direction;

const MINI: u64 = ONE_LOT / 10;
const NO_BOUND_BUY: i64 = i64::MAX;
const NO_BOUND_SELL: i64 = 1;
const DAY: i64 = 86_400;

fn env_with_pool() -> Env {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_insurance(50_000 * ONE_USDC);
    env
}

// --- the round trip ---------------------------------------------------------------------------

#[test]
fn a_provider_can_deposit_wait_and_redeem() {
    let mut env = env_with_pool();
    let lp = env.new_lp(100_000 * ONE_USDC);

    env.lp_deposit(&lp, 50_000 * ONE_USDC).unwrap();
    env.assert_invariants();

    let shares = env.lp_shares(&lp);
    assert_eq!(shares, 50_000 * ONE_USDC, "the first deposit mints 1:1");
    assert_eq!(env.nav_per_share(), ONE_USDC, "NAV starts at $1.00");

    env.request_remove(&lp, shares).unwrap();
    env.advance_clock(DAY + 1);
    env.remove_liquidity(&lp, 0).unwrap();
    env.assert_invariants();

    assert_eq!(env.lp_shares(&lp), 0);
    assert_eq!(env.lp_state().lp_token_supply, 0);
    assert_eq!(env.lp_state().aum, 0);

    // Whole. The exit fee is waived when the redemption empties the pool: it exists to
    // compensate the LPs who stayed, and there are none. Leaving it behind would strand USDC
    // against zero shares — which is what invariant I8 forbids.
    let returned = env.token_balance(&lp.token_account);
    assert_eq!(
        returned,
        100_000 * ONE_USDC,
        "the last provider out pays no exit fee, because there is nobody to pay it to"
    );
}

/// **The cooldown is the defence.** Redemption before it elapses must fail.
#[test]
fn redemption_before_the_cooldown_elapses_is_refused() {
    let mut env = env_with_pool();
    let lp = env.new_lp(100_000 * ONE_USDC);
    env.lp_deposit(&lp, 50_000 * ONE_USDC).unwrap();

    let shares = env.lp_shares(&lp);
    env.request_remove(&lp, shares).unwrap();

    assert_err_contains(
        env.remove_liquidity(&lp, 0),
        "Withdrawal cooldown has not elapsed",
    );

    // One second short still fails. The boundary is not approximate.
    env.advance_clock(DAY - 1);
    assert_err_contains(
        env.remove_liquidity(&lp, 0),
        "Withdrawal cooldown has not elapsed",
    );

    env.advance_clock(2);
    env.remove_liquidity(&lp, 0).unwrap();
    env.assert_invariants();
}

#[test]
fn redemption_without_a_request_is_refused() {
    let mut env = env_with_pool();
    let lp = env.new_lp(100_000 * ONE_USDC);
    env.lp_deposit(&lp, 50_000 * ONE_USDC).unwrap();

    assert!(
        env.remove_liquidity(&lp, 0).is_err(),
        "there is no request account to settle against"
    );
    env.assert_invariants();
}

#[test]
fn a_cancelled_request_cannot_be_settled() {
    let mut env = env_with_pool();
    let lp = env.new_lp(100_000 * ONE_USDC);
    env.lp_deposit(&lp, 50_000 * ONE_USDC).unwrap();

    let shares = env.lp_shares(&lp);
    env.request_remove(&lp, shares).unwrap();
    assert_eq!(env.lp_state().pending_withdrawal_shares, shares);

    env.cancel_remove(&lp).unwrap();
    assert_eq!(env.lp_state().pending_withdrawal_shares, 0);

    env.advance_clock(DAY + 1);
    assert!(
        env.remove_liquidity(&lp, 0).is_err(),
        "a cancelled request must not settle"
    );
    env.assert_invariants();
}

/// Cancelling costs nothing. Staying in the pool is the outcome the cooldown wants; charging
/// for it would push LPs to follow through on exits they had changed their mind about.
#[test]
fn cancelling_is_free_and_the_provider_keeps_their_shares() {
    let mut env = env_with_pool();
    let lp = env.new_lp(100_000 * ONE_USDC);
    env.lp_deposit(&lp, 50_000 * ONE_USDC).unwrap();

    let shares_before = env.lp_shares(&lp);
    let aum_before = env.lp_state().aum;

    env.request_remove(&lp, shares_before).unwrap();
    env.cancel_remove(&lp).unwrap();

    assert_eq!(env.lp_shares(&lp), shares_before);
    assert_eq!(env.lp_state().aum, aum_before);
    env.assert_invariants();
}

#[test]
fn two_outstanding_requests_are_refused() {
    let mut env = env_with_pool();
    let lp = env.new_lp(100_000 * ONE_USDC);
    env.lp_deposit(&lp, 50_000 * ONE_USDC).unwrap();

    let shares = env.lp_shares(&lp);
    env.request_remove(&lp, shares / 2).unwrap();
    assert!(
        env.request_remove(&lp, shares / 2).is_err(),
        "a second request would need a second unlock clock"
    );
}

#[test]
fn requesting_more_shares_than_held_is_refused() {
    let mut env = env_with_pool();
    let lp = env.new_lp(100_000 * ONE_USDC);
    env.lp_deposit(&lp, 10_000 * ONE_USDC).unwrap();

    assert_err_contains(
        env.request_remove(&lp, 20_000 * ONE_USDC),
        "Insufficient LP shares",
    );
}

// --- threat T6: the just-in-time attack -------------------------------------------------------

/// **The attack, attempted directly.**
///
/// An LP sees a trade the pool is about to win. They deposit in front of it and try to
/// withdraw the moment it settles — capturing the pool's edge without carrying its risk.
///
/// The cooldown makes that impossible: the redemption cannot settle until a day has passed,
/// by which time the attacker has been exposed to everything that happened in it.
#[test]
fn the_jit_attack_cannot_capture_a_gain_without_carrying_risk() {
    let mut env = env_with_pool();

    // An honest LP who has been in the pool all along.
    let honest = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&honest, 100_000 * ONE_USDC).unwrap();

    // A trader opens a position the pool is about to win on.
    let trader = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();

    // The attacker front-runs the settlement.
    let attacker = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&attacker, 100_000 * ONE_USDC).unwrap();
    let attacker_shares = env.lp_shares(&attacker);
    env.assert_invariants();

    // The trade settles against the trader; the pool gains.
    let loss_price = env.post_price_now(FEED_EUR_USD, PriceSpec::at(107_543_000).conf(13_893));
    env.close(&trader, 0, 0, NO_BOUND_SELL, loss_price).unwrap();
    env.assert_invariants();

    // The attacker tries to leave immediately. Refused.
    env.request_remove(&attacker, attacker_shares).unwrap();
    assert_err_contains(
        env.remove_liquidity(&attacker, 0),
        "Withdrawal cooldown has not elapsed",
    );

    // They must carry the pool's risk for a full day before they can settle — which is
    // precisely the risk the attack was designed to avoid.
    env.advance_clock(DAY + 1);
    env.remove_liquidity(&attacker, 0).unwrap();
    env.assert_invariants();

    // They did capture a share of the gain, because they genuinely were an LP while it
    // settled. The defence is not that they earn nothing; it is that they were exposed for a
    // day to earn it, which makes the strategy a real position rather than a free option.
    let out = env.token_balance(&attacker.token_account);
    assert!(
        out > 100_000 * ONE_USDC,
        "an LP present for a winning settlement should share in it: {out}"
    );
}

/// The mirror attack: withdraw ahead of a loss the pool is about to take.
///
/// Also blocked, and by the same mechanism — the request cannot settle until the cooldown
/// elapses, so the LP is still in the pool when the loss lands.
#[test]
fn an_lp_cannot_exit_ahead_of_a_loss_the_pool_is_about_to_take() {
    let mut env = env_with_pool();
    let lp = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&lp, 100_000 * ONE_USDC).unwrap();
    let shares = env.lp_shares(&lp);

    // A trader opens a position that is about to win.
    let trader = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();

    // The LP sees it coming and requests an exit.
    env.request_remove(&lp, shares).unwrap();
    let nav_at_request = env.nav_per_share();

    // The trade settles in the trader's favour while the request is still locked.
    env.advance_clock(DAY / 2);
    let win_price = env.post_price_now(FEED_EUR_USD, PriceSpec::at(109_543_000).conf(13_893));
    env.close(&trader, 0, 0, NO_BOUND_SELL, win_price).unwrap();
    env.assert_invariants();

    let nav_after_loss = env.nav_per_share();
    assert!(
        nav_after_loss < nav_at_request,
        "the pool should have paid out: NAV {nav_at_request} -> {nav_after_loss}"
    );

    env.advance_clock(DAY);
    env.remove_liquidity(&lp, 0).unwrap();
    env.assert_invariants();

    // They are paid at the *settled* NAV, not the one they requested at. The loss was theirs.
    let out = env.token_balance(&lp.token_account);
    assert!(
        out < 200_000 * ONE_USDC,
        "the LP escaped a loss they were present for: {out}"
    );
}

/// Slippage protection on the exit. An LP who requested at one NAV and settles at a much
/// lower one can refuse the trade rather than being forced through it.
#[test]
fn min_usdc_out_protects_a_provider_from_settling_into_a_collapse() {
    let mut env = env_with_pool();
    let lp = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&lp, 100_000 * ONE_USDC).unwrap();
    let shares = env.lp_shares(&lp);

    env.request_remove(&lp, shares).unwrap();
    env.advance_clock(DAY + 1);

    assert_err_contains(
        env.remove_liquidity(&lp, 200_000 * ONE_USDC),
        "Fill price is worse than the slippage bound",
    );

    // The request survives the refusal and can still be settled at an acceptable bound.
    env.remove_liquidity(&lp, 90_000 * ONE_USDC).unwrap();
    env.assert_invariants();
}

// --- fees ---------------------------------------------------------------------------------------

/// The exit fee stays in the pool. § 8.1 lists it as treasury revenue; this protocol pays it
/// to the LPs who stayed instead, because they are the party the defence exists to protect.
#[test]
fn the_exit_fee_accrues_to_the_providers_who_stayed() {
    let mut env = env_with_pool();
    let stayer = env.new_lp(200_000 * ONE_USDC);
    let leaver = env.new_lp(200_000 * ONE_USDC);

    env.lp_deposit(&stayer, 100_000 * ONE_USDC).unwrap();
    env.lp_deposit(&leaver, 100_000 * ONE_USDC).unwrap();

    let nav_before = env.nav_per_share();
    let treasury_before = env.token_balance(&env.fee_vault);

    let shares = env.lp_shares(&leaver);
    env.request_remove(&leaver, shares).unwrap();
    env.advance_clock(DAY + 1);
    env.remove_liquidity(&leaver, 0).unwrap();
    env.assert_invariants();

    assert!(
        env.nav_per_share() > nav_before,
        "the exit fee must raise NAV/share for the remaining LPs: {} -> {}",
        nav_before,
        env.nav_per_share()
    );
    assert_eq!(
        env.token_balance(&env.fee_vault),
        treasury_before,
        "the exit fee must not reach the treasury"
    );
    assert!(env.lp_state().total_exit_fees > 0);
}

/// No performance fee while the pool is at or below its high-water mark.
#[test]
fn no_performance_fee_is_charged_without_a_gain() {
    let mut env = env_with_pool();
    let lp = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&lp, 100_000 * ONE_USDC).unwrap();

    let shares = env.lp_shares(&lp);
    env.request_remove(&lp, shares).unwrap();
    env.advance_clock(DAY + 1);
    env.remove_liquidity(&lp, 0).unwrap();

    assert_eq!(
        env.lp_state().total_performance_fees,
        0,
        "a pool that never rose above par owes no performance fee"
    );
    env.assert_invariants();
}

/// A performance fee is charged once the pool has genuinely made money, and it goes to the
/// treasury rather than staying in the pool.
#[test]
fn a_performance_fee_is_charged_on_a_real_gain() {
    let mut env = env_with_pool();
    let lp = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&lp, 100_000 * ONE_USDC).unwrap();

    // Traders lose; the pool gains well above par.
    for i in 0..3u8 {
        let trader = env.new_user(100_000 * ONE_USDC, Pubkey::default());
        env.deposit(&trader, 50_000 * ONE_USDC).unwrap();
        let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        env.open(
            &trader,
            0,
            i,
            Direction::Long,
            MINI * 5,
            25_000 * ONE_USDC,
            NO_BOUND_BUY,
            p,
        )
        .unwrap();
        let loss = env.post_price_now(FEED_EUR_USD, PriceSpec::at(105_543_000).conf(13_893));
        env.close(&trader, 0, i, NO_BOUND_SELL, loss).unwrap();
        env.assert_invariants();
    }

    let nav = env.nav_per_share();
    assert!(nav > ONE_USDC, "the pool should be above par: {nav}");

    let treasury_before = env.token_balance(&env.fee_vault);
    let shares = env.lp_shares(&lp);
    env.request_remove(&lp, shares).unwrap();
    env.advance_clock(DAY + 1);
    env.remove_liquidity(&lp, 0).unwrap();
    env.assert_invariants();

    assert!(
        env.lp_state().total_performance_fees > 0,
        "a pool well above its high-water mark must charge the performance fee"
    );
    assert!(
        env.token_balance(&env.fee_vault) > treasury_before,
        "the performance fee goes to the treasury"
    );
}

// --- adversarial sequences ------------------------------------------------------------------------

/// Many providers, interleaved deposits and redemptions, with the invariants re-checked after
/// every step. This is the "adversarial sequences" the exit criterion asks for: the shapes
/// where an accounting error would hide are the ones with several parties in flight at once.
#[test]
fn lp_accounting_survives_interleaved_providers() {
    let mut env = env_with_pool();
    let lps: Vec<LiquidityProvider> = (0..4).map(|_| env.new_lp(500_000 * ONE_USDC)).collect();

    // Everyone in.
    for (i, lp) in lps.iter().enumerate() {
        env.lp_deposit(lp, (10_000 + 5_000 * i as u64) * ONE_USDC)
            .unwrap();
        env.assert_invariants();
    }

    // A trade moves NAV between the deposits and the exits.
    let trader = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();
    let loss = env.post_price_now(FEED_EUR_USD, PriceSpec::at(107_543_000).conf(13_893));
    env.close(&trader, 0, 0, NO_BOUND_SELL, loss).unwrap();
    env.assert_invariants();

    // Two request, one cancels, a third joins late.
    env.request_remove(&lps[0], env.lp_shares(&lps[0])).unwrap();
    env.request_remove(&lps[1], env.lp_shares(&lps[1]) / 2)
        .unwrap();
    env.cancel_remove(&lps[1]).unwrap();
    env.assert_invariants();

    let latecomer = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&latecomer, 50_000 * ONE_USDC).unwrap();
    env.assert_invariants();

    // Settle everything.
    env.advance_clock(DAY + 1);
    env.remove_liquidity(&lps[0], 0).unwrap();
    env.assert_invariants();

    for lp in &lps[1..] {
        env.request_remove(lp, env.lp_shares(lp)).unwrap();
        env.advance_clock(DAY + 1);
        env.remove_liquidity(lp, 0).unwrap();
        env.assert_invariants();
    }

    env.request_remove(&latecomer, env.lp_shares(&latecomer))
        .unwrap();
    env.advance_clock(DAY + 1);
    env.remove_liquidity(&latecomer, 0).unwrap();
    env.assert_invariants();

    // The pool is empty and nothing is stranded.
    let pool = env.lp_state();
    assert_eq!(pool.lp_token_supply, 0);
    assert_eq!(pool.aum, 0, "no dust may be left where nobody can claim it");
}

/// A provider who joins after the pool has made money buys fewer shares for the same money.
/// The whole point of NAV-based pricing.
#[test]
fn a_late_provider_does_not_dilute_the_early_one() {
    let mut env = env_with_pool();
    let early = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&early, 100_000 * ONE_USDC).unwrap();
    let early_shares = env.lp_shares(&early);

    // The pool makes money.
    let trader = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        0,
        Direction::Long,
        MINI * 5,
        25_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();
    let loss = env.post_price_now(FEED_EUR_USD, PriceSpec::at(105_543_000).conf(13_893));
    env.close(&trader, 0, 0, NO_BOUND_SELL, loss).unwrap();

    let nav = env.nav_per_share();
    assert!(nav > ONE_USDC);

    // The same deposit now buys fewer shares.
    let late = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&late, 100_000 * ONE_USDC).unwrap();
    let late_shares = env.lp_shares(&late);

    assert!(
        late_shares < early_shares,
        "a late provider paying the same money must receive fewer shares: \
         {late_shares} vs {early_shares}"
    );
    env.assert_invariants();
}

// --- the vault circuit breakers (§ 7.2, § 7.4) ---------------------------------------------------

/// § 7.2: an insurance fund below its floor means the protocol has no reserve behind new
/// risk, so it stops taking any.
#[test]
fn a_depleted_insurance_fund_blocks_new_positions() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    // Fund it far below the 25% floor of its $100,000 target.
    env.seed_insurance(1_000 * ONE_USDC);
    env.patch_protocol(|p| p.min_insurance_ratio_bps = 2_500);

    let trader = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());

    assert_err_contains(
        env.open(
            &trader,
            0,
            0,
            Direction::Long,
            MINI,
            5_000 * ONE_USDC,
            NO_BOUND_BUY,
            p,
        ),
        "Insurance fund is below its minimum",
    );
    env.assert_invariants();
}

/// § 7.4: past the skew cap the heavy side closes to new opens. Funding alone is too slow a
/// lever at extremes.
///
/// The test also exercises the **imbalance floor**. § 7.4's ratio alone would block the very
/// first position in a market — one position is 100% one-sided by definition — so the cap
/// only binds once the imbalance is worth at least 10% of pool AUM. Below that a lopsided
/// book carries no real risk to the vault.
#[test]
fn the_skew_cap_closes_the_heavy_side_but_not_the_light_one() {
    let mut env = env_with_pool();
    env.seed_pool(1_000_000 * ONE_USDC); // floor: $100,000 of imbalance
    env.patch_protocol(|p| p.max_skew_bps = 6_000); // § 7.4's 0.6

    let trader = env.new_user(1_000_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 500_000 * ONE_USDC).unwrap();

    // A first, small long. 100% one-sided, but only ~$10,850 of imbalance against a
    // $100,000 floor — so the ratio does not bind and the market can actually be opened.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .expect("the first position in a market must not be blocked by a ratio it cannot avoid");

    // Balance the book.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        1,
        Direction::Short,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_SELL,
        p,
    )
    .unwrap();
    env.assert_invariants();

    // Now a long large enough to push the imbalance past both gates: ~$325,000 against the
    // $100,000 floor, and a 93% ratio against the 60% cap.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.open(
            &trader,
            0,
            2,
            Direction::Long,
            MINI * 30,
            150_000 * ONE_USDC,
            NO_BOUND_BUY,
            p,
        ),
        "Open interest is too one-sided",
    );

    // The correcting trade is always allowed — a cap that blocked it would entrench the
    // imbalance it exists to fix.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        3,
        Direction::Short,
        MINI * 5,
        25_000 * ONE_USDC,
        NO_BOUND_SELL,
        p,
    )
    .expect("a trade that reduces skew must never be blocked");
    env.assert_invariants();
}

/// § 7.2: a pool cannot credibly stand behind unlimited open interest.
#[test]
fn the_utilisation_ceiling_blocks_opens_the_pool_cannot_back() {
    let mut env = env_with_pool();
    let lp = env.new_lp(200_000 * ONE_USDC);
    env.lp_deposit(&lp, 10_000 * ONE_USDC).unwrap();
    env.patch_protocol(|p| p.max_utilisation_bps = 10_000); // 100% of AUM

    let trader = env.new_user(500_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 200_000 * ONE_USDC).unwrap();

    // ~$10,854 of notional against $10,000 of AUM is already over the ceiling.
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(
        env.open(
            &trader,
            0,
            0,
            Direction::Long,
            MINI,
            5_000 * ONE_USDC,
            NO_BOUND_BUY,
            p,
        ),
        "Vault utilisation is at its ceiling",
    );

    // More liquidity, and the same trade fits.
    env.lp_deposit(&lp, 100_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .expect("a deeper pool can back the same trade");
    env.assert_invariants();
}

// --- treasury -------------------------------------------------------------------------------------

/// The admin can sweep the treasury's own fees — and nothing else. There is no instruction
/// anywhere that lets them sign a transfer from the collateral, LP or insurance vaults, which
/// is the structural form of the non-custodial claim.
#[test]
fn the_admin_can_sweep_the_fee_vault_and_only_the_fee_vault() {
    let mut env = env_with_pool();
    env.seed_pool(1_000_000 * ONE_USDC);

    // Generate some treasury fees.
    let trader = env.new_user(100_000 * ONE_USDC, Pubkey::default());
    env.deposit(&trader, 50_000 * ONE_USDC).unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        &trader,
        0,
        0,
        Direction::Long,
        MINI,
        5_000 * ONE_USDC,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();

    let fees = env.token_balance(&env.fee_vault);
    assert!(fees > 0, "the open fee should have reached the treasury");

    let destination = Pubkey::new_unique();
    let mint = env.usdc_mint;
    let admin_key = env.admin.pubkey();
    env.write_token_account(destination, mint, admin_key, 0);

    let admin = env.admin.insecure_clone();
    let ix = env.withdraw_treasury_ix(destination, fees);
    env.send(ix, &[&admin]).unwrap();

    assert_eq!(env.token_balance(&destination), fees);
    assert_eq!(env.token_balance(&env.fee_vault), 0);
    env.assert_invariants();
}

#[test]
fn a_stranger_cannot_sweep_the_treasury() {
    let mut env = env_with_pool();
    let attacker = solana_keypair::Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    let destination = Pubkey::new_unique();
    let mint = env.usdc_mint;
    env.write_token_account(destination, mint, attacker.pubkey(), 0);

    let mut ix = env.withdraw_treasury_ix(destination, 1);
    ix.accounts[0].pubkey = attacker.pubkey();
    assert_err_contains(
        env.send(ix, &[&attacker]),
        "Signer is not the protocol admin",
    );
}
