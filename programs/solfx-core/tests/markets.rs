//! Market listing (`ARCHITECTURE.md` § 5.6) and parameter validation (§ 6.3, § 7.1).
//!
//! The claim under test: **a new market needs no redeploy.** The program is the formula and
//! markets are rows, so listing an instrument is one transaction against an unchanged binary.
//! § 12.5 makes this a hard exit criterion for Phase 9; these tests establish it now, while
//! it is still cheap to fix if it turns out not to hold.

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
use solana_keypair::Keypair;
use solana_signer::Signer;
use solfx_core::state::{FeedKind, MarketStatus, PriceSource, QuoteConversionKind};

#[test]
fn a_listed_market_starts_untradeable() {
    let mut env = Env::new();
    env.init_protocol();
    env.init_market(0, &MarketSpec::eur_usd());

    let m = env.market_state(0);
    assert_eq!(
        m.status,
        MarketStatus::Initialized,
        "a freshly listed market must not be tradeable — activation is a separate reviewed \
         step, so a mistyped feed id cannot be traded against in the meantime"
    );
    assert_eq!(m.symbol_str(), "EURUSD");
    assert_eq!(m.market_index, 0);
    assert_eq!(m.max_leverage, 50);
    assert_eq!(m.pyth_feed_id, FEED_EUR_USD);
    assert!(!m.is_synthetic());
    assert!(!m.needs_quote_conversion());
    assert_eq!(env.protocol_state().num_markets, 1);
}

/// The § 5.6 property, demonstrated: twelve instruments across four price/quote shapes, all
/// against one deployment of one binary. No upgrade, no migration, no downtime.
#[test]
fn many_markets_list_against_one_unchanged_binary() {
    let mut env = Env::new();
    env.init_protocol();

    let specs: Vec<MarketSpec> = vec![
        MarketSpec::eur_usd(),
        MarketSpec::xau_usd(),
        MarketSpec::usd_inr(),
        MarketSpec::eur_gbp_synthetic(),
        MarketSpec::btc_usd(),
    ];

    for (i, spec) in specs.iter().enumerate() {
        env.list_and_activate(u16::try_from(i).unwrap(), spec);
    }

    // Seven more, to show the count is not the constraint.
    for i in 5..12u16 {
        let mut s = MarketSpec::eur_usd();
        s.params.symbol = format!("PAIR{i:02}");
        s.params.pyth_feed_id = [u8::try_from(i).unwrap() + 0x80; 32];
        env.list_and_activate(i, &s);
    }

    assert_eq!(env.protocol_state().num_markets, 12);

    // Every market still readable and independently configured — listing #12 did not
    // disturb #0, which is the property a redeploy would have broken.
    assert_eq!(env.market_state(0).symbol_str(), "EURUSD");
    assert_eq!(env.market_state(0).max_leverage, 50);
    assert_eq!(env.market_state(2).symbol_str(), "USDINR");
    assert_eq!(
        env.market_state(2).max_leverage,
        20,
        "EM leverage must stay a fraction of majors: the risk shape is a policy gap, not a \
         trading range, and no liquidation engine outruns a gap"
    );
    assert!(env.market_state(3).is_synthetic());
    assert_eq!(env.market_state(4).feed_kind, FeedKind::Crypto);
}

#[test]
fn market_indices_must_be_sequential() {
    let mut env = Env::new();
    env.init_protocol();

    // Skipping an index would leave a hole that an indexer walking `0..num_markets` misses.
    let ix = env.init_market_ix(3, &MarketSpec::eur_usd());
    let admin = env.admin.insecure_clone();
    assert_err_contains(
        env.send(ix, &[&admin]),
        "Market index does not match the protocol's next index",
    );
}

#[test]
fn a_stranger_cannot_list_a_market() {
    let mut env = Env::new();
    env.init_protocol();

    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    let mut ix = env.init_market_ix(0, &MarketSpec::eur_usd());
    ix.accounts[0].pubkey = attacker.pubkey();
    assert_err_contains(
        env.send(ix, &[&attacker]),
        "Signer is not the protocol admin",
    );
}

// --- feed configuration ------------------------------------------------------------------

/// Phase 0b found eight EM symbols listed in Pyth's catalogue with `publish_time == 0` —
/// registered names with no data behind them. A market pointed at one could be created but
/// never priced, opened or liquidated.
#[test]
fn rejects_an_all_zero_feed_id() {
    let mut env = Env::new();
    env.init_protocol();

    let mut spec = MarketSpec::eur_usd();
    spec.params.pyth_feed_id = [0u8; 32];

    let ix = env.init_market_ix(0, &spec);
    let admin = env.admin.insecure_clone();
    assert_err_contains(env.send(ix, &[&admin]), "Feed id must not be all zeroes");
}

/// The measurement that killed the weekend product, encoded as a constraint.
///
/// `ContinuousIndex` is retained so the regime can be switched by config the day Q1b
/// resolves — but until a 24/7 feed is actually reachable, listing against one would be
/// promising something the oracle does not deliver.
#[test]
fn continuous_index_markets_are_blocked_pending_q1b() {
    let mut env = Env::new();
    env.init_protocol();

    let mut spec = MarketSpec::xau_usd();
    spec.params.feed_kind = FeedKind::ContinuousIndex;

    let ix = env.init_market_ix(0, &spec);
    let admin = env.admin.insecure_clone();
    assert_err_contains(
        env.send(ix, &[&admin]),
        "FeedKind::ContinuousIndex has no reachable feed",
    );
}

#[test]
fn a_synthetic_market_requires_both_legs() {
    let mut env = Env::new();
    env.init_protocol();

    let mut spec = MarketSpec::eur_gbp_synthetic();
    spec.params.secondary_feed_id = [0u8; 32];

    let ix = env.init_market_ix(0, &spec);
    let admin = env.admin.insecure_clone();
    assert_err_contains(env.send(ix, &[&admin]), "Feed id must not be all zeroes");
}

/// An account a market does not use is an account nobody audits. Reject rather than ignore.
#[test]
fn a_direct_market_rejects_a_secondary_feed() {
    let mut env = Env::new();
    env.init_protocol();

    let mut spec = MarketSpec::eur_usd();
    spec.params.secondary_feed_id = FEED_GBP_USD;

    let ix = env.init_market_ix(0, &spec);
    let admin = env.admin.insecure_clone();
    assert_err_contains(env.send(ix, &[&admin]), "Feed id must not be all zeroes");
}

/// Correction C-3: for USD/INR the PnL formula produces Rupees. Booking those as USDC
/// mis-prices the position by the FX rate — at ~88, by a factor of 88. The conversion feed
/// is what prevents it, so a market that declares a conversion must carry one.
#[test]
fn a_non_usd_quoted_market_requires_a_conversion_feed() {
    let mut env = Env::new();
    env.init_protocol();

    let mut spec = MarketSpec::usd_inr();
    spec.params.quote_conversion_feed = [0u8; 32];

    let ix = env.init_market_ix(0, &spec);
    let admin = env.admin.insecure_clone();
    assert_err_contains(env.send(ix, &[&admin]), "Feed id must not be all zeroes");
}

#[test]
fn a_usd_quoted_market_rejects_a_conversion_feed() {
    let mut env = Env::new();
    env.init_protocol();

    let mut spec = MarketSpec::eur_usd();
    spec.params.quote_conversion_feed = FEED_USD_JPY;
    spec.params.quote_conversion_kind = QuoteConversionKind::None;

    let ix = env.init_market_ix(0, &spec);
    let admin = env.admin.insecure_clone();
    assert_err_contains(env.send(ix, &[&admin]), "Feed id must not be all zeroes");
}

#[test]
fn the_conversion_direction_is_recorded_alongside_the_feed() {
    let mut env = Env::new();
    env.init_protocol();
    env.init_market(0, &MarketSpec::usd_inr());

    let m = env.market_state(0);
    assert!(m.needs_quote_conversion());
    assert_eq!(
        m.quote_conversion_kind,
        QuoteConversionKind::QuotePerUsd,
        "the feed id alone does not say whether to multiply or divide — storing only the id \
         would leave the direction to be guessed at the call site"
    );
}

// --- risk parameter validation -----------------------------------------------------------

fn expect_rejected(spec: MarketSpec, needle: &str) {
    let mut env = Env::new();
    env.init_protocol();
    let ix = env.init_market_ix(0, &spec);
    let admin = env.admin.insecure_clone();
    assert_err_contains(env.send(ix, &[&admin]), needle);
}

#[test]
fn rejects_zero_leverage() {
    let mut s = MarketSpec::eur_usd();
    s.params.max_leverage = 0;
    expect_rejected(s, "Leverage must be between 1 and the protocol maximum");
}

/// 500x is what GMTrade offers. The cap makes it impossible to set here by accident: every
/// leverage increase is easy PR and every decrease reads as distress, so the ceiling is a
/// one-way door that belongs behind a deliberate change.
#[test]
fn rejects_leverage_above_the_protocol_ceiling() {
    let mut s = MarketSpec::eur_usd();
    s.params.max_leverage = 500;
    expect_rejected(s, "Leverage must be between 1 and the protocol maximum");
}

/// A position that is liquidatable the instant it opens is not a position.
#[test]
fn rejects_maintenance_margin_at_or_above_initial_margin() {
    let mut s = MarketSpec::eur_usd();
    s.params.mmr_bps = s.params.imr_bps;
    expect_rejected(
        s,
        "Maintenance margin must be positive and below initial margin",
    );
}

/// `imr_bps` and `max_leverage` state the same constraint twice. If IMR sat below
/// `1 / max_leverage` the leverage cap would be unreachable, and the effective ceiling would
/// be whichever check happened to run first.
#[test]
fn rejects_an_initial_margin_that_contradicts_the_leverage_cap() {
    let mut s = MarketSpec::eur_usd();
    s.params.max_leverage = 50; // implies IMR >= 200 bps
    s.params.imr_bps = 150;
    s.params.mmr_bps = 100;
    expect_rejected(
        s,
        "Maintenance margin must be positive and below initial margin",
    );
}

/// The penalty must leave something behind. A liquidation fee at or above the maintenance
/// margin guarantees the position closes with negative equity, manufacturing bad debt out of
/// an otherwise recoverable liquidation.
#[test]
fn rejects_a_liquidation_fee_at_or_above_the_maintenance_margin() {
    let mut s = MarketSpec::eur_usd();
    s.params.liquidation_fee_bps = s.params.mmr_bps;
    expect_rejected(s, "Liquidation fee must be positive");
}

/// Every listable feed publishes at a 1 s cadence. A window beyond a minute is not tolerance
/// for a slow feed, it is tolerance for a closed one — which is the C-1 exploit.
#[test]
fn rejects_a_staleness_window_beyond_the_ceiling() {
    let mut s = MarketSpec::eur_usd();
    s.params.max_staleness_seconds = 3_600;
    expect_rejected(s, "Staleness window must be positive");
}

#[test]
fn rejects_a_zero_staleness_window() {
    let mut s = MarketSpec::eur_usd();
    s.params.max_staleness_seconds = 0;
    expect_rejected(s, "Staleness window must be positive");
}

#[test]
fn rejects_a_confidence_ceiling_beyond_the_protocol_limit() {
    let mut s = MarketSpec::eur_usd();
    s.params.max_conf_bps = 5_000;
    expect_rejected(s, "Confidence limit must be positive");
}

#[test]
fn rejects_inconsistent_position_size_bounds() {
    let mut s = MarketSpec::eur_usd();
    s.params.min_position_size = 10 * ONE_USDC;
    s.params.max_position_size = ONE_USDC;
    expect_rejected(s, "Position size bounds are inconsistent");
}

#[test]
fn rejects_an_invalid_session_calendar() {
    let mut s = MarketSpec::eur_usd();
    s.params.session_open_dow = 9;
    expect_rejected(s, "Session calendar is invalid");
}

#[test]
fn rejects_an_empty_symbol() {
    let mut s = MarketSpec::eur_usd();
    s.params.symbol = String::new();
    expect_rejected(s, "Market symbol is empty or too long");
}

#[test]
fn rejects_an_oversized_symbol() {
    let mut s = MarketSpec::eur_usd();
    s.params.symbol = "THIS_SYMBOL_IS_FAR_TOO_LONG".to_string();
    expect_rejected(s, "Market symbol is empty or too long");
}

// --- retuning without an upgrade ---------------------------------------------------------

/// Risk parameters are per-market data, never constants. That is what lets EM sit at 20x and
/// majors at 50x on the same binary, and what lets either be retuned from a measurement
/// without shipping code.
#[test]
fn risk_parameters_can_be_retuned_in_place() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.update_risk_ix(
        0,
        solfx_core::instructions::UpdateRiskParams {
            max_leverage: 30,
            imr_bps: 334,
            mmr_bps: 167,
            liquidation_fee_bps: 60,
            max_oi_long: 500_000 * ONE_USDC,
            max_oi_short: 500_000 * ONE_USDC,
            max_position_size: 50_000 * ONE_USDC,
            min_position_size: ONE_USDC,
            max_staleness_seconds: 5,
            max_conf_bps: 10,
            liquidation_max_conf_bps: 40,
            max_deviation_bps: 200,
            weekend_max_leverage: 0,
            weekend_oi_cap_bps: 0,
            weekend_max_conf_bps: 0,
        },
    );
    env.send(ix, &[&admin]).unwrap();

    let m = env.market_state(0);
    assert_eq!(m.max_leverage, 30);
    assert_eq!(m.mmr_bps, 167);
    assert_eq!(m.max_conf_bps, 10);
    assert_eq!(m.max_staleness_seconds, 5);
}

/// A partial update must not be able to leave a market in a state nobody validated.
#[test]
fn a_retune_into_an_invalid_envelope_is_rejected_whole() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    let before = env.market_state(0);

    let admin = env.admin.insecure_clone();
    let ix = env.update_risk_ix(
        0,
        solfx_core::instructions::UpdateRiskParams {
            max_leverage: 50,
            imr_bps: 200,
            mmr_bps: 300, // above IMR
            liquidation_fee_bps: 50,
            max_oi_long: ONE_USDC,
            max_oi_short: ONE_USDC,
            max_position_size: ONE_USDC,
            min_position_size: ONE_USDC,
            max_staleness_seconds: 10,
            max_conf_bps: 15,
            liquidation_max_conf_bps: 60,
            max_deviation_bps: 300,
            weekend_max_leverage: 0,
            weekend_oi_cap_bps: 0,
            weekend_max_conf_bps: 0,
        },
    );
    assert_err_contains(
        env.send(ix, &[&admin]),
        "Maintenance margin must be positive and below initial margin",
    );

    let after = env.market_state(0);
    assert_eq!(after.mmr_bps, before.mmr_bps);
    assert_eq!(after.max_leverage, before.max_leverage);
}

/// The carry rate is the interest-rate differential plus the protocol markup. Publishing it
/// on chain is the concrete improvement over XM and Exness (§ 6.7) — a trader can read the
/// exact swap rate instead of discovering it after rollover.
#[test]
fn the_carry_rate_is_stored_on_chain_and_readable() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.update_fees_ix(
        0,
        solfx_core::instructions::UpdateFeeParams {
            open_fee_rate: 80_000, // 0.8 bps — not expressible in whole bps
            close_fee_rate: 80_000,
            base_spread_bps: 2,
            conf_spread_multiplier_bps: 10_000,
            skew_impact_bps_per_unit: 1,
            weekend_spread_bps: 0,
            funding_rate_cap_per_hour: 500_000,
            carry_rate_per_hour: -228_310, // long EUR/USD pays
        },
    );
    env.send(ix, &[&admin]).unwrap();

    let m = env.market_state(0);
    assert_eq!(
        m.open_fee_rate, 80_000,
        "0.8 bps must be expressible — it is a tier in the § 8.2 schedule and would round to \
         1 bp if fees were stored in whole basis points"
    );
    assert_eq!(m.carry_rate_per_hour, -228_310);
}

// --- status transitions ------------------------------------------------------------------

#[test]
fn admin_can_move_a_market_through_the_tradeable_states() {
    let mut env = Env::new();
    env.init_protocol();
    env.init_market(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    for status in [
        MarketStatus::Active,
        MarketStatus::ReduceOnly,
        MarketStatus::GapWindow,
        MarketStatus::Halted,
        MarketStatus::Active,
    ] {
        let ix = env.set_status_ix(0, status);
        env.send(ix, &[&admin]).unwrap();
        assert_eq!(env.market_state(0).status, status);
    }
}

/// Both belong to the continuous regime, which no market can currently occupy. Setting one
/// by hand would drop a session-bound market into a state whose transitions Phase 4 never
/// wrote — which is how a market gets stuck.
#[test]
fn continuous_regime_states_cannot_be_set_by_hand() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    for status in [MarketStatus::WeekendMode, MarketStatus::PreOpenWindow] {
        let ix = env.set_status_ix(0, status);
        assert_err_contains(env.send(ix, &[&admin]), "Invalid market status transition");
    }
}

#[test]
fn delisting_is_terminal() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Delisted);
    env.send(ix, &[&admin]).unwrap();

    let ix = env.set_status_ix(0, MarketStatus::Active);
    assert_err_contains(env.send(ix, &[&admin]), "Invalid market status transition");
}

// --- repointing the oracle ---------------------------------------------------------------
//
// `pyth_feed_id` was write-once until `set_market_oracle`: `initialize_market` set it and
// nothing could correct it. Pyth retires feeds — `InterestRate.10YZ6` and
// `Commodities.CAN6/USD` are both gone from the catalogue — and a market whose feed is
// retired is dead in the way that matters: it cannot be priced, so open positions on it
// cannot be closed or liquidated either. These six cases fix the shape of the repair path.

/// The happy path, and the only shape the instruction accepts: halt, repoint, re-activate.
#[test]
fn a_halted_market_can_be_repointed_at_a_different_feed() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Halted);
    env.send(ix, &[&admin]).unwrap();

    let ix = env.set_oracle_ix(0, FEED_EUR_USD, FEED_GBP_USD);
    env.send(ix, &[&admin]).unwrap();

    assert_eq!(
        env.market_state(0).pyth_feed_id,
        FEED_GBP_USD,
        "the stored feed must be the new one — this is the whole point of the instruction"
    );
    assert_eq!(
        env.market_state(0).status,
        MarketStatus::Halted,
        "repointing must not activate the market as a side effect; re-activation stays a \
         separate reviewed step, exactly as at listing"
    );

    // And the market is usable again afterwards.
    let ix = env.set_status_ix(0, MarketStatus::Active);
    env.send(ix, &[&admin]).unwrap();
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
    env.assert_invariants();
}

/// Drift's `handle_update_spot_market_oracle` guard: the caller passes the feed it believes
/// is stored, and a mismatch aborts. A deployment script working from a stale picture of the
/// chain therefore cannot repoint a market — which is precisely how the wrong feeds got onto
/// devnet in the first place.
#[test]
fn repointing_requires_knowing_the_current_feed() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Halted);
    env.send(ix, &[&admin]).unwrap();

    let ix = env.set_oracle_ix(0, FEED_USD_JPY, FEED_GBP_USD);
    assert_err_contains(
        env.send(ix, &[&admin]),
        "Price update does not match this market's configured feed",
    );

    assert_eq!(
        env.market_state(0).pyth_feed_id,
        FEED_EUR_USD,
        "a rejected repoint must leave the feed untouched"
    );
    env.assert_invariants();
}

/// The same check `initialize_market` applies, reused rather than reimplemented: a market
/// pointed at an all-zero id could be created but never priced, opened or liquidated.
#[test]
fn repointing_rejects_an_all_zero_feed() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Halted);
    env.send(ix, &[&admin]).unwrap();

    let ix = env.set_oracle_ix(0, FEED_EUR_USD, [0u8; 32]);
    assert_err_contains(env.send(ix, &[&admin]), "Feed id must not be all zeroes");

    assert_eq!(env.market_state(0).pyth_feed_id, FEED_EUR_USD);
    env.assert_invariants();
}

/// A no-op write fails loudly. Emitting `MarketOracleChanged` with two identical ids would
/// tell the Phase 8 indexer a repoint happened when nothing did.
#[test]
fn repointing_a_market_at_its_own_feed_is_rejected() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Halted);
    env.send(ix, &[&admin]).unwrap();

    let ix = env.set_oracle_ix(0, FEED_EUR_USD, FEED_EUR_USD);
    assert_err_contains(env.send(ix, &[&admin]), "Parameter out of range");

    assert_eq!(env.market_state(0).pyth_feed_id, FEED_EUR_USD);
    env.assert_invariants();
}

/// The load-bearing case.
///
/// Every live position's entry price, unrealised PnL and liquidation threshold was computed
/// against the old feed. Repointing under an open book would re-price all of them against an
/// instrument they were never opened on — the next oracle read would compare a EUR/USD entry
/// to a GBP/USD quote and liquidate on the difference. Forcing a halt first is what makes
/// this instruction defensible at all.
///
/// Note the guard is `status != Active`, so `ReduceOnly` and `GapWindow` still permit a
/// repoint even though positions can be closed in both. That is the plan's specified shape;
/// it is a deliberate narrowing to the state where new positions can be opened, not an
/// oversight, but it is the line worth revisiting if this instruction ever grows a caller
/// beyond the operator repair path.
#[test]
fn an_active_market_cannot_be_repointed() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.set_oracle_ix(0, FEED_EUR_USD, FEED_GBP_USD);
    assert_err_contains(env.send(ix, &[&admin]), "Market is not active");

    assert_eq!(
        env.market_state(0).pyth_feed_id,
        FEED_EUR_USD,
        "an active market's feed must survive a repoint attempt intact — a position opened \
         against this feed is still open"
    );
    assert_eq!(env.market_state(0).status, MarketStatus::Active);
    env.assert_invariants();
}

#[test]
fn a_stranger_cannot_repoint_a_market() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let admin = env.admin.insecure_clone();
    let ix = env.set_status_ix(0, MarketStatus::Halted);
    env.send(ix, &[&admin]).unwrap();

    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    // The attacker knows the current feed — the compare-and-swap is not an access control,
    // and must not be mistaken for one. `has_one = admin` is what stops this.
    let mut ix = env.set_oracle_ix(0, FEED_EUR_USD, FEED_GBP_USD);
    ix.accounts[0].pubkey = attacker.pubkey();
    assert_err_contains(
        env.send(ix, &[&attacker]),
        "Signer is not the protocol admin",
    );

    assert_eq!(env.market_state(0).pyth_feed_id, FEED_EUR_USD);
    env.assert_invariants();
}

/// The allow-list in `MarketStatus::allows_open` is deliberately narrow: a status added
/// later is closed to new positions until someone writes it in on purpose.
#[test]
fn only_active_and_weekend_mode_permit_opening() {
    use MarketStatus::*;
    for s in [Active, WeekendMode] {
        assert!(s.allows_open(), "{s:?} should permit opens");
    }
    for s in [
        Initialized,
        ReduceOnly,
        Halted,
        GapWindow,
        PreOpenWindow,
        Delisted,
    ] {
        assert!(!s.allows_open(), "{s:?} must not permit opens");
    }
}

/// An underwater position must stay liquidatable even while the market is halted. A halt
/// that also stopped liquidations would convert a bad position into bad debt.
#[test]
fn liquidation_remains_possible_in_every_state_except_initialized() {
    use MarketStatus::*;
    assert!(!Initialized.allows_liquidation());
    for s in [
        Active,
        ReduceOnly,
        Halted,
        GapWindow,
        WeekendMode,
        PreOpenWindow,
        Delisted,
    ] {
        assert!(s.allows_liquidation(), "{s:?} must permit liquidation");
    }
}

#[test]
fn a_direct_market_reports_no_synthetic_source() {
    assert!(!matches!(
        MarketSpec::eur_usd().params.price_source,
        PriceSource::Synthetic { .. }
    ));
    assert!(matches!(
        MarketSpec::eur_gbp_synthetic().params.price_source,
        PriceSource::Synthetic { invert_quote: true }
    ));
}
