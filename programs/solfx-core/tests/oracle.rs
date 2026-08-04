//! The validated read path, end to end (`ARCHITECTURE.md` ADR-006, § 7.1, § 7.2).
//!
//! This suite is where Phase 2's exit criterion is met: **direct, synthetic, EM and
//! non-USD-quoted feeds all price correctly on chain**, and every gate rejects what it is
//! supposed to reject.
//!
//! Threats T2 (latency arbitrage) and T3 (stale price) are the two that kill a pool-backed
//! venue. Everything else in the threat table is recoverable. Both are stopped here or
//! nowhere.

// Test code asserts against known values and unwraps expected-Ok results. The workspace
// denies `unwrap`, `expect` and `panic` because a panic in a *program* is a failed
// transaction with no named error; in a test a panic is the reporting mechanism, and
// routing every assertion through a Result would make the suite unreadable without making
// anything safer.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

mod common;

use common::*;
use pyth_solana_receiver_sdk::price_update::VerificationLevel;
use solana_signer::Signer;
use solfx_core::state::MarketStatus;

fn env_with_market(spec: &MarketSpec) -> Env {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, spec);
    env
}

// --- the happy path ----------------------------------------------------------------------

/// Exponent normalisation, end to end. Pyth publishes EUR/USD 1.08543 as mantissa
/// `108_543_000` at exponent -8; the engine stores `1_085_430_000` at `PRICE_PRECISION`.
#[test]
fn a_direct_market_prices_correctly() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());

    env.crank(0, price).unwrap();

    let m = env.market_state(0);
    assert_eq!(m.last_price, 1_085_430_000, "1.08543 at PRICE_PRECISION");
    assert_eq!(m.ema_price, 1_085_430_000);
    assert_eq!(m.last_price_update_ts, T0);
    assert_eq!(m.status, MarketStatus::Active);
}

/// Gold at 4046.95 — the Friday close recorded in `oracle-feasibility.md`, and the tightest
/// feed measured in the whole set at 1.43 bps p50.
#[test]
fn gold_prices_correctly_at_its_own_scale() {
    let mut env = env_with_market(&MarketSpec::xau_usd());
    let price = env.post_price(
        FEED_XAU_USD,
        PriceSpec::at(404_695_000_000).conf(57_871_400),
    );

    env.crank(0, price).unwrap();
    assert_eq!(env.market_state(0).last_price, 4_046_950_000_000);
}

/// USD/JPY at 157.20 must not overflow `i64` at `PRICE_PRECISION` — eleven orders of
/// magnitude of headroom, per § 6.1.
#[test]
fn a_high_valued_quote_pair_prices_without_overflow() {
    let mut env = Env::new();
    env.init_protocol();
    let mut spec = MarketSpec::eur_usd();
    spec.params.symbol = "USDJPY".into();
    spec.params.pyth_feed_id = FEED_USD_JPY;
    env.list_and_activate(0, &spec);

    let price = env.post_price(FEED_USD_JPY, PriceSpec::at(15_720_000_000).conf(133_620));
    env.crank(0, price).unwrap();
    assert_eq!(env.market_state(0).last_price, 157_200_000_000);
}

/// The generic-engine proof (§ 5.6): crypto needs a regime, not new maths.
#[test]
fn a_crypto_market_prices_through_the_same_path() {
    let mut env = env_with_market(&MarketSpec::btc_usd());
    let price = env.post_price(
        FEED_BTC_USD,
        PriceSpec::at(9_500_000_000_000).conf(2_000_000_000),
    );

    env.crank(0, price).unwrap();
    assert_eq!(env.market_state(0).last_price, 95_000_000_000_000);
}

// --- gate 4: freshness, both ends --------------------------------------------------------

/// The measurement that ended the weekend product, as a regression test.
///
/// On Saturday 2026-08-01 every Pyth metals feed was frozen at the Friday 21:00 close —
/// 21.4 hours stale. Trading against that price is free money for anyone who read the news,
/// and the C-1 exploit that drains the vault.
#[test]
fn a_friday_close_read_on_saturday_is_rejected() {
    let mut env = env_with_market(&MarketSpec::xau_usd());

    let friday_close = T0 - 77_040; // 21.4 hours
    let price = env.post_price(
        FEED_XAU_USD,
        PriceSpec::at(404_695_000_000)
            .conf(57_871_400)
            .published_at(friday_close),
    );

    assert_err_contains(env.crank(0, price), "Oracle price is stale");
    assert_eq!(
        env.market_state(0).last_price,
        0,
        "a rejected read must leave no trace on the market"
    );
}

#[test]
fn the_staleness_window_is_exact_at_its_boundary() {
    let mut env = env_with_market(&MarketSpec::eur_usd()); // max_staleness_seconds = 10

    let ok = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(T0 - 10));
    env.crank(0, ok).unwrap();

    let too_old = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(T0 - 11));
    assert_err_contains(env.crank(0, too_old), "Oracle price is stale");
}

/// The gap the SDK helper leaves open. `get_price_no_older_than` only asks
/// `publish_time + max_age >= now`; a price stamped an hour ahead satisfies that trivially
/// and keeps satisfying it for an hour regardless of what the market does.
#[test]
fn a_future_dated_price_is_rejected() {
    let mut env = env_with_market(&MarketSpec::eur_usd());

    // Inside the clock-drift allowance.
    let ok = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(T0 + 5));
    env.crank(0, ok).unwrap();

    let ahead = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(T0 + 6));
    assert_err_contains(env.crank(0, ahead), "Oracle price is dated in the future");

    let far_ahead = env.post_price(FEED_EUR_USD, PriceSpec::default().published_at(T0 + 3_600));
    assert_err_contains(
        env.crank(0, far_ahead),
        "Oracle price is dated in the future",
    );
}

/// A price that was fresh when posted goes stale as the clock advances. The gate reads the
/// cluster clock, not the moment of posting.
#[test]
fn a_price_goes_stale_as_the_clock_advances() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());

    env.crank(0, price).unwrap();

    env.advance_clock(60);
    assert_err_contains(env.crank(0, price), "Oracle price is stale");
}

// --- gate 5: confidence (correction C-4) -------------------------------------------------

/// USD/IDR measured 30.11 bps p95 and was excluded from the listable set on exactly this
/// test. At 50x leverage a band that wide is worth several percent of a trader's margin, and
/// quoting the mid against it hands free optionality to anyone with a faster feed.
///
/// # The crank *observes*; it does not gate
///
/// A wide band **halts the market** rather than failing the crank. That distinction is
/// load-bearing: the crank is what trips the breaker, so a crank that refused an uncertain
/// price could never trip it — the market would stay `Active` holding a last-known price
/// from before the blow-out, which is the failure mode looking exactly like the exploit.
///
/// The *rejection* happens on the trading path. See
/// `positions::a_wide_confidence_band_blocks_opening_a_position`.
#[test]
fn a_band_wider_than_the_market_ceiling_halts_the_market() {
    let mut env = env_with_market(&MarketSpec::eur_usd()); // max_conf_bps = 15

    // 30.11 bps of 1.08543 — USD/IDR's measured p95.
    let wide = env.post_price(FEED_EUR_USD, PriceSpec::default().conf(326_823));
    env.crank(0, wide)
        .expect("the crank must record a wide price, not refuse it");
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "a band twice the ceiling must trip the confidence breaker"
    );
    assert_eq!(
        env.market_state(0).last_price,
        1_085_430_000,
        "the observation is still recorded — halting is a response to it"
    );
}

#[test]
fn a_band_inside_the_ceiling_does_not_halt() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let ok = env.post_price(FEED_EUR_USD, PriceSpec::default().conf(151_960)); // 14 bps
    env.crank(0, ok).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
}

/// Gold's ceiling is tighter than EUR/USD's, because its measured band is. Per-market
/// ceilings are data, and the same price is acceptable on one market and not on another.
#[test]
fn the_confidence_ceiling_is_per_market() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd()); // 15 bps
    env.list_and_activate(1, &MarketSpec::xau_usd()); // 10 bps

    // ~12 bps on each feed.
    let eur = env.post_price(FEED_EUR_USD, PriceSpec::default().conf(130_252));
    env.crank(0, eur).unwrap();

    let gold = env.post_price(
        FEED_XAU_USD,
        PriceSpec::at(404_695_000_000).conf(485_634_000),
    );
    let keeper = env.admin.insecure_clone();
    let ix = env.crank_ix(1, gold, None, None, keeper.pubkey());
    env.send(ix, &[&keeper]).unwrap();

    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Active,
        "12 bps is inside EUR/USD's 15 bps ceiling"
    );
    assert_eq!(
        env.market_state(1).status,
        MarketStatus::Halted,
        "the same 12 bps is outside gold's tighter 10 bps ceiling — per-market ceilings are \
         data, and the same price is acceptable on one market and not on another"
    );
}

// --- gate 2 and 3: identity and verification ---------------------------------------------

/// A real, fresh, fully verified update for the wrong instrument must not price this market.
#[test]
fn a_valid_update_for_another_feed_is_rejected() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let wrong_feed = env.post_price(FEED_GBP_USD, PriceSpec::default());

    assert_err_contains(
        env.crank(0, wrong_feed),
        "Price update does not match this market's configured feed",
    );
}

/// `Partial` verification lowers the number of Wormhole guardians that must collude to forge
/// a price. That is not a trade worth making on a leveraged venue.
#[test]
fn a_partially_verified_update_is_rejected() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let partial = env.post_price(
        FEED_EUR_USD,
        PriceSpec::default().verification(VerificationLevel::Partial { num_signatures: 5 }),
    );

    assert_err_contains(
        env.crank(0, partial),
        "Price update does not match this market's configured feed",
    );
}

// --- gate 6: deviation and the circuit breaker (§ 7.2) -----------------------------------

/// EUR/CHF fell ~30% in minutes on 15 January 2015 and bankrupted brokers including Alpari
/// UK. The breaker must fire long before that, and the market must **stay** halted for a
/// human to look at.
#[test]
fn a_large_dislocation_trips_the_breaker_and_halts_the_market() {
    let mut env = env_with_market(&MarketSpec::eur_usd()); // max_deviation_bps = 300

    // Spot 8.5% away from Pyth's EMA.
    let dislocated = env.post_price(
        FEED_EUR_USD,
        PriceSpec::at(108_543_000).ema(100_000_000).conf(13_893),
    );
    env.crank(0, dislocated).unwrap();

    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "a deviating market must halt for review, not merely reject one trade"
    );
    assert_eq!(
        env.market_state(0).last_price,
        1_085_430_000,
        "the observation is still recorded — halting is a response to it, not a refusal to \
         look at it"
    );
}

/// The reason the crank *observes* rather than *gates*: if refreshing the reference required
/// passing the deviation check, a market could never recover from a genuine large move. The
/// reference would stay stale, every later price would deviate from it, and the market would
/// be wedged shut precisely when it most needed attention.
#[test]
fn a_halted_market_can_still_be_repriced() {
    let mut env = env_with_market(&MarketSpec::eur_usd());

    let dislocated = env.post_price(
        FEED_EUR_USD,
        PriceSpec::at(108_543_000).ema(100_000_000).conf(13_893),
    );
    env.crank(0, dislocated).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);

    // Once the EMA catches up, the crank still works and records the new level.
    let settled = env.post_price(FEED_EUR_USD, PriceSpec::at(108_000_000).conf(13_893));
    env.crank(0, settled).unwrap();
    assert_eq!(env.market_state(0).last_price, 1_080_000_000);

    // Reopening is a human decision, never automatic.
    assert_eq!(env.market_state(0).status, MarketStatus::Halted);
    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Active);
    env.send(ix, &[&admin]).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
}

#[test]
fn a_move_inside_the_deviation_limit_does_not_halt() {
    let mut env = env_with_market(&MarketSpec::eur_usd()); // 300 bps

    // 2% away — inside the limit.
    let moved = env.post_price(
        FEED_EUR_USD,
        PriceSpec::at(102_000_000).ema(100_000_000).conf(13_000),
    );
    env.crank(0, moved).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
}

/// A feed with no EMA yet must not be permanently untradeable — § 7.1 skips the check when
/// the reference is not established.
#[test]
fn a_feed_without_an_ema_still_prices() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let no_ema = env.post_price(FEED_EUR_USD, PriceSpec::default().ema(0));

    env.crank(0, no_ema).unwrap();
    assert_eq!(env.market_state(0).last_price, 1_085_430_000);
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
}

// --- synthetic composition (§ 3.5) -------------------------------------------------------

/// EUR/GBP = EUR/USD ÷ GBP/USD. 1.0850 ÷ 1.2700 = 0.854330708…
#[test]
fn a_synthetic_market_composes_both_legs() {
    let mut env = env_with_market(&MarketSpec::eur_gbp_synthetic());

    let eur = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(13_893));
    let gbp = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(17_272));

    let keeper = env.admin.insecure_clone();
    let ix = env.crank_ix(0, eur, Some(gbp), None, keeper.pubkey());
    env.send(ix, &[&keeper]).unwrap();

    assert_eq!(env.market_state(0).last_price, 854_330_708);
}

#[test]
fn a_synthetic_market_without_its_second_leg_is_rejected() {
    let mut env = env_with_market(&MarketSpec::eur_gbp_synthetic());
    let eur = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(13_893));

    assert_err_contains(
        env.crank(0, eur),
        "A synthetic market requires a second price update account",
    );
}

/// § 7.2: a synthetic market rejects if **either** leg fails validation.
#[test]
fn a_synthetic_market_rejects_when_either_leg_is_stale() {
    let mut env = env_with_market(&MarketSpec::eur_gbp_synthetic());
    let keeper = env.admin.insecure_clone();

    let fresh_eur = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(13_893));
    let stale_gbp = env.post_price(
        FEED_GBP_USD,
        PriceSpec::at(127_000_000)
            .conf(17_272)
            .published_at(T0 - 600),
    );
    let ix = env.crank_ix(0, fresh_eur, Some(stale_gbp), None, keeper.pubkey());
    assert_err_contains(env.send(ix, &[&keeper]), "Oracle price is stale");

    let stale_eur = env.post_price(
        FEED_EUR_USD,
        PriceSpec::at(108_500_000)
            .conf(13_893)
            .published_at(T0 - 600),
    );
    let fresh_gbp = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(17_272));
    let ix = env.crank_ix(0, stale_eur, Some(fresh_gbp), None, keeper.pubkey());
    assert_err_contains(env.send(ix, &[&keeper]), "Oracle price is stale");
}

/// Confidence compounds. Two legs at 12 bps each compose to ~24 bps, which clears the
/// synthetic market's 30 bps ceiling; push them to 20 bps each and the composed 40 bps does
/// not. This is the linear-sum rule doing its job — quadrature would have reported ~28 bps
/// and let it through.
#[test]
fn composed_confidence_is_gated_on_the_sum_not_the_legs() {
    let mut env = env_with_market(&MarketSpec::eur_gbp_synthetic()); // 30 bps
    let keeper = env.admin.insecure_clone();

    let eur12 = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(130_200));
    let gbp12 = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(152_400));
    let ix = env.crank_ix(0, eur12, Some(gbp12), None, keeper.pubkey());
    env.send(ix, &[&keeper]).unwrap();

    assert_eq!(env.market_state(0).status, MarketStatus::Active);

    let eur20 = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(217_000));
    let gbp20 = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(254_000));
    let ix = env.crank_ix(0, eur20, Some(gbp20), None, keeper.pubkey());
    env.send(ix, &[&keeper]).unwrap();
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "40 bps composed exceeds the 30 bps ceiling — quadrature would have reported ~28 bps \
         and let it through"
    );
}

// --- quote conversion (correction C-3) ---------------------------------------------------

/// USD/INR is the shape that makes C-3 mandatory rather than optional: PnL lands in Rupees,
/// and booking it as USDC would mis-price every position by a factor of ~88.
#[test]
fn a_non_usd_quoted_market_requires_its_conversion_feed() {
    let mut env = env_with_market(&MarketSpec::usd_inr());

    // 88.42, at expo -8.
    let inr = env.post_price(FEED_USD_INR, PriceSpec::at(8_842_000_000).conf(5_180_000));

    assert_err_contains(
        env.crank(0, inr),
        "This market requires a quote-conversion price update account",
    );

    let conv = env.post_price(FEED_USD_INR, PriceSpec::at(8_842_000_000).conf(5_180_000));
    let keeper = env.admin.insecure_clone();
    let ix = env.crank_ix(0, inr, None, Some(conv), keeper.pubkey());
    env.send(ix, &[&keeper]).unwrap();

    assert_eq!(env.market_state(0).last_price, 88_420_000_000);
}

/// An account the market does not use is an account nobody audits.
#[test]
fn supplying_an_unused_price_account_is_rejected() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let extra = env.post_price(FEED_GBP_USD, PriceSpec::default());
    let keeper = env.admin.insecure_clone();

    let ix = env.crank_ix(0, price, Some(extra), None, keeper.pubkey());
    assert_err_contains(
        env.send(ix, &[&keeper]),
        "A price update account was supplied that this market does not use",
    );

    let ix = env.crank_ix(0, price, None, Some(extra), keeper.pubkey());
    assert_err_contains(
        env.send(ix, &[&keeper]),
        "A price update account was supplied that this market does not use",
    );
}

// --- ownership (gate 1) ------------------------------------------------------------------

/// An attacker-owned account holding perfectly-shaped `PriceUpdateV2` bytes must not be
/// readable as a price. Anchor enforces this by type, but the whole security model rests on
/// it, so it is asserted rather than assumed.
#[test]
fn a_price_account_not_owned_by_the_pyth_receiver_is_rejected() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let forged = env.post_price_owned_by(FEED_EUR_USD, PriceSpec::default(), solfx_core::ID);

    assert!(
        env.crank(0, forged).is_err(),
        "only the Pyth receiver program may own an account the engine reads a price from"
    );
}

// --- lifecycle ---------------------------------------------------------------------------

#[test]
fn a_delisted_market_cannot_be_cranked() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Delisted);
    env.send(ix, &[&admin]).unwrap();

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    assert_err_contains(env.crank(0, price), "Market is not active");
}

/// Cranking is permissionless: a market whose price only its operator can record has a
/// single point of failure, and § 9.2 requires a resilient keeper market.
#[test]
fn anyone_can_crank() {
    let mut env = env_with_market(&MarketSpec::eur_usd());
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());

    let stranger = solana_keypair::Keypair::new();
    env.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();

    let ix = env.crank_ix(0, price, None, None, stranger.pubkey());
    env.send(ix, &[&stranger]).unwrap();
    assert_eq!(env.market_state(0).last_price, 1_085_430_000);
}
