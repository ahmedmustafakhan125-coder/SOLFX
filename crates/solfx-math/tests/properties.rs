//! Property tests for the SolFX financial engine (`ARCHITECTURE.md` § 12.2).
//!
//! Unit tests check known values. These check *invariants* across the whole parameter
//! space, which is where the expensive bugs actually live — a rounding direction that only
//! flips at one size, an overflow that only appears at gold's price scale.
//!
//! The one that matters most is [`round_trip_at_the_same_price_always_loses`]. If it ever
//! passes, there is free money in the protocol and bots will extract it until the vault is
//! empty.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::integer_division,
    clippy::unwrap_used
)]

use proptest::prelude::*;

use solfx_math::constants::{MIN_NOTIONAL_QUOTE, RATE_PRECISION};
use solfx_math::fees::{self, FeeSplitBps, ONE_BPS_RATE};
use solfx_math::fixed;
use solfx_math::funding;
use solfx_math::margin::{self, AccruedCosts};
use solfx_math::oracle;
use solfx_math::pnl;
use solfx_math::pricing;
use solfx_math::{Direction, QuoteConversion, Side, TradeAction};

/// Case count for a property, overridable by `PROPTEST_CASES`.
///
/// `ProptestConfig::with_cases` would pin the count and ignore the environment, so CI's deep
/// run could not turn the dial up. This reads the variable first and falls back to the
/// per-property default.
fn config(default_cases: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_cases);
    ProptestConfig {
        cases,
        // The reject budget is a fixed 1,024 by default and does not scale with `cases`, so a
        // deep run aborts on rejects long before it exhausts its case budget. A property that
        // filters on a *computed* result (e.g. "only positions whose liquidation price did not
        // clamp") cannot avoid some rejection, so the budget has to grow with the run.
        max_global_rejects: cases,
        ..ProptestConfig::default()
    }
}

// --- strategies ---------------------------------------------------------------------------

/// Position sizes from a micro-lot up to 1,000 standard lots.
fn size_strategy() -> impl Strategy<Value = u64> {
    1_000_000_000_u64..100_000_000_000_000_000
}

/// Prices spanning the listable set: from a heavily devalued EM rate up to gold.
fn price_strategy() -> impl Strategy<Value = i64> {
    100_000_000_i64..10_000_000_000_000
}

/// Fee rates spanning the published schedule, 0.6 to 1.5 bps.
fn fee_rate_strategy() -> impl Strategy<Value = u64> {
    60_000_u64..=150_000
}

/// Total spread in bps, from zero up to a wide-news figure.
fn spread_strategy() -> impl Strategy<Value = u64> {
    0_u64..=200
}

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop_oneof![Just(Direction::Long), Just(Direction::Short)]
}

/// Order a generated pair instead of filtering with `prop_assume!`.
///
/// Filtering rejects about half of all cases, which trips proptest's global reject limit
/// long before the case budget is spent. Sorting keeps every sample.
fn ordered<T: PartialOrd>(pair: (T, T)) -> (T, T) {
    if pair.0 <= pair.1 {
        pair
    } else {
        (pair.1, pair.0)
    }
}

// --- the test that matters ----------------------------------------------------------------

proptest! {
    #![proptest_config(config(4096))]

    /// **The single most valuable test in the suite** (`ARCHITECTURE.md` § 12.2).
    ///
    /// Open and immediately close at an unchanged oracle price. The trader must always end
    /// with less collateral than they started with — they paid the spread twice and the fee
    /// twice, and took no risk. If this ever fails, a bot can extract value indefinitely.
    #[test]
    fn round_trip_at_the_same_price_always_loses(
        size in size_strategy(),
        oracle in price_strategy(),
        spread in spread_strategy(),
        rate in fee_rate_strategy(),
        direction in direction_strategy(),
    ) {
        let entry_side = Side::resolve(direction, TradeAction::Open);
        let exit_side = Side::resolve(direction, TradeAction::Close);

        let entry_price = pricing::execution_price(oracle, spread, entry_side).unwrap();
        let exit_price = pricing::execution_price(oracle, spread, exit_side).unwrap();

        let entry_notional = pnl::notional_in_quote(size, entry_price).unwrap();
        prop_assume!(entry_notional >= MIN_NOTIONAL_QUOTE);
        let exit_notional = pnl::notional_in_quote(size, exit_price).unwrap();

        let open_fee = fees::fee_amount(entry_notional, rate).unwrap();
        let close_fee = fees::fee_amount(exit_notional, rate).unwrap();

        let gross = pnl::upnl_in_quote(size, entry_price, exit_price, direction).unwrap();
        let net = i128::from(gross) - i128::from(open_fee) - i128::from(close_fee);

        prop_assert!(
            net < 0,
            "free money: net {net} (gross {gross}, fees {open_fee}+{close_fee}) \
             at size {size} oracle {oracle} spread {spread} rate {rate} {direction:?}"
        );
    }

    /// The same round trip on a non-USD-quoted market. C-3's conversion must not open a
    /// hole that the USD-quoted path does not have.
    #[test]
    fn round_trip_never_profits_on_a_non_usd_quoted_market(
        size in size_strategy(),
        oracle in 10_000_000_000_i64..200_000_000_000, // USD/JPY and USD/INR territory
        spread in spread_strategy(),
        rate in fee_rate_strategy(),
        direction in direction_strategy(),
    ) {
        let entry_side = Side::resolve(direction, TradeAction::Open);
        let exit_side = Side::resolve(direction, TradeAction::Close);

        let entry_price = pricing::execution_price(oracle, spread, entry_side).unwrap();
        let exit_price = pricing::execution_price(oracle, spread, exit_side).unwrap();

        let conv = QuoteConversion::QuotePerUsd;

        let entry_notional =
            pnl::notional_in_collateral(size, entry_price, conv, oracle).unwrap();
        prop_assume!(entry_notional >= MIN_NOTIONAL_QUOTE);
        let exit_notional = pnl::notional_in_collateral(size, exit_price, conv, oracle).unwrap();

        let open_fee = fees::fee_amount(entry_notional, rate).unwrap();
        let close_fee = fees::fee_amount(exit_notional, rate).unwrap();

        let gross = pnl::upnl_in_collateral(
            size, entry_price, exit_price, direction, conv, oracle,
        ).unwrap();
        let net = i128::from(gross) - i128::from(open_fee) - i128::from(close_fee);

        prop_assert!(net < 0, "free money on a converted market: net {net}");
    }
}

// --- PnL ----------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(2048))]

    /// A long and a short of the same size, entry and exit must be equal and opposite,
    /// to within one unit of protocol-favourable rounding.
    #[test]
    fn pnl_is_antisymmetric(
        size in size_strategy(),
        entry in price_strategy(),
        exit in price_strategy(),
    ) {
        let long = pnl::upnl_in_quote(size, entry, exit, Direction::Long).unwrap();
        let short = pnl::upnl_in_quote(size, entry, exit, Direction::Short).unwrap();
        let sum = i128::from(long) + i128::from(short);

        // Never positive: the pair must not create value out of rounding.
        prop_assert!(sum <= 0, "rounding created {sum} from nothing");
        prop_assert!(sum >= -1, "rounding lost more than one unit: {sum}");
    }

    /// Closing at the entry price yields exactly zero, in both directions.
    #[test]
    fn pnl_is_zero_at_the_entry_price(size in size_strategy(), entry in price_strategy()) {
        for d in [Direction::Long, Direction::Short] {
            prop_assert_eq!(pnl::upnl_in_quote(size, entry, entry, d).unwrap(), 0);
        }
    }

    /// PnL must move the right way: a long gains as price rises, a short loses.
    #[test]
    fn pnl_is_monotonic_in_price(
        size in size_strategy(),
        entry in price_strategy(),
        exits in (price_strategy(), price_strategy()),
    ) {
        let (lower, higher) = ordered(exits);
        let long_low = pnl::upnl_in_quote(size, entry, lower, Direction::Long).unwrap();
        let long_high = pnl::upnl_in_quote(size, entry, higher, Direction::Long).unwrap();
        prop_assert!(long_high >= long_low);

        let short_low = pnl::upnl_in_quote(size, entry, lower, Direction::Short).unwrap();
        let short_high = pnl::upnl_in_quote(size, entry, higher, Direction::Short).unwrap();
        prop_assert!(short_high <= short_low);
    }

    /// Splitting a close into parts must never realise more than closing all at once.
    #[test]
    fn partial_closes_never_realise_more_than_the_whole(
        (total, split) in (2_u64..1_000_000).prop_flat_map(|t| (Just(t), 1_u64..t)),
        gross in -1_000_000_000_i64..1_000_000_000,
    ) {
        let first = pnl::proportional_pnl(gross, split, total).unwrap();
        let second = pnl::proportional_pnl(gross, total - split, total).unwrap();
        prop_assert!(i128::from(first) + i128::from(second) <= i128::from(gross));
    }

    /// A weighted entry always lands between the two prices it averages.
    #[test]
    fn weighted_entry_stays_within_bounds(
        size_a in 1_u64..1_000_000_000_000,
        size_b in 1_u64..1_000_000_000_000,
        price_a in price_strategy(),
        price_b in price_strategy(),
        direction in direction_strategy(),
    ) {
        let avg = pnl::weighted_entry_price(size_a, price_a, size_b, price_b, direction)
            .unwrap();
        let lo = price_a.min(price_b);
        let hi = price_a.max(price_b);
        prop_assert!(avg >= lo && avg <= hi, "{avg} outside [{lo}, {hi}]");
    }

    /// Converting a cost and converting a payout must never favour the trader: for the same
    /// magnitude, the cost conversion is always at least the payout conversion.
    #[test]
    fn cost_conversion_never_undercuts_payout_conversion(
        amount in 1_u64..1_000_000_000_000,
        rate in price_strategy(),
    ) {
        for conv in [QuoteConversion::QuotePerUsd, QuoteConversion::UsdPerQuote] {
            let cost = pnl::convert_cost_to_collateral(amount, conv, rate).unwrap();
            let payout =
                pnl::convert_pnl_to_collateral(amount as i64, conv, rate).unwrap();
            prop_assert!(
                i128::from(cost) >= i128::from(payout),
                "cost {cost} below payout {payout}"
            );
        }
    }
}

// --- pricing ------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(2048))]

    /// The book must never be crossed: the price a buyer pays always exceeds the price a
    /// seller receives, so there is no risk-free spread to capture.
    #[test]
    fn the_book_is_never_crossed(price in price_strategy(), spread in spread_strategy()) {
        let buy = pricing::execution_price(price, spread, Side::Buy).unwrap();
        let sell = pricing::execution_price(price, spread, Side::Sell).unwrap();
        prop_assert!(buy >= price);
        prop_assert!(sell <= price);
        prop_assert!(buy >= sell);
    }

    /// A wider spread is always at least as adverse — never accidentally better.
    #[test]
    fn a_wider_spread_is_never_better_for_the_trader(
        price in price_strategy(),
        spreads in (spread_strategy(), spread_strategy()),
    ) {
        let (narrow, wide) = ordered(spreads);
        let buy_narrow = pricing::execution_price(price, narrow, Side::Buy).unwrap();
        let buy_wide = pricing::execution_price(price, wide, Side::Buy).unwrap();
        prop_assert!(buy_wide >= buy_narrow);

        let sell_narrow = pricing::execution_price(price, narrow, Side::Sell).unwrap();
        let sell_wide = pricing::execution_price(price, wide, Side::Sell).unwrap();
        prop_assert!(sell_wide <= sell_narrow);
    }

    /// Any non-zero spread must move the price by at least one unit, or a sub-unit spread
    /// rounds away and hands out free fills.
    #[test]
    fn a_nonzero_spread_always_bites(price in price_strategy(), spread in 1_u64..=200) {
        let buy = pricing::execution_price(price, spread, Side::Buy).unwrap();
        let sell = pricing::execution_price(price, spread, Side::Sell).unwrap();
        prop_assert!(buy > price);
        prop_assert!(sell < price);
    }

    /// Trading toward a more balanced book is never charged skew impact.
    #[test]
    fn improving_the_book_is_never_penalised(
        long_oi in 0_i128..1_000_000_000_000_000,
        short_oi in 0_i128..1_000_000_000_000_000,
        size in 1_i128..1_000_000_000_000_000,
        rate in 0_u32..1_000_000,
    ) {
        let toward_balance = if long_oi > short_oi { -size } else { size };
        let delta = pricing::skew_delta(long_oi, short_oi, toward_balance).unwrap();
        if delta <= 0 {
            prop_assert_eq!(pricing::skew_impact_bps(delta, rate).unwrap(), 0);
        }
    }

    /// Confidence is the oracle's own uncertainty; the spread it produces must scale with it.
    #[test]
    fn confidence_spread_is_monotonic(
        confidences in (0_u64..1_000, 0_u64..1_000),
        multiplier in 1_u16..30_000,
    ) {
        let (low, high) = ordered(confidences);
        let a = pricing::confidence_spread_bps(low, multiplier).unwrap();
        let b = pricing::confidence_spread_bps(high, multiplier).unwrap();
        prop_assert!(b >= a);
    }
}

// --- fees ---------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(2048))]

    /// Fees round toward the protocol: the charge is never below the exact value, and
    /// never overshoots it by a whole unit.
    #[test]
    fn fees_always_round_toward_the_protocol(
        notional in 1_u64..1_000_000_000_000_000,
        rate in fee_rate_strategy(),
    ) {
        let fee = fees::fee_amount(notional, rate).unwrap();
        let exact = (u128::from(notional) * u128::from(rate)) / RATE_PRECISION;
        prop_assert!(u128::from(fee) >= exact, "fee {fee} below exact {exact}");
        prop_assert!(u128::from(fee) <= exact + 1, "fee {fee} overshot exact {exact}");
    }

    /// Threat T14: no trade with positive notional may ever be free.
    #[test]
    fn no_positive_notional_trade_is_ever_free(
        notional in 1_u64..1_000_000_000_000_000,
        rate in fee_rate_strategy(),
    ) {
        prop_assert!(fees::fee_amount(notional, rate).unwrap() >= 1);
    }

    /// A higher rate never charges less.
    #[test]
    fn fees_are_monotonic_in_rate(
        notional in 1_u64..1_000_000_000_000_000,
        rates in (fee_rate_strategy(), fee_rate_strategy()),
    ) {
        let (low, high) = ordered(rates);
        prop_assert!(
            fees::fee_amount(notional, low).unwrap()
                <= fees::fee_amount(notional, high).unwrap()
        );
    }

    /// Invariant I7: a split must conserve every unit collected.
    #[test]
    fn fee_splits_conserve_every_unit(fee in 0_u64..1_000_000_000_000_000) {
        let a = fees::split_fee(fee, FeeSplitBps::LAUNCH).unwrap();
        prop_assert_eq!(a.total().unwrap(), fee);
    }

    /// Rounding dust from a split must accrue to the protocol, so the LP share is never
    /// more than its exact entitlement.
    #[test]
    fn split_never_overpays_the_lp_share(fee in 0_u64..1_000_000_000_000_000) {
        let a = fees::split_fee(fee, FeeSplitBps::LAUNCH).unwrap();
        let exact_lp = (u128::from(fee) * u128::from(FeeSplitBps::LAUNCH.lp)) / 10_000;
        prop_assert!(u128::from(a.lp) <= exact_lp);
    }

    /// A liquidation penalty split must also conserve, and never pay the liquidator more
    /// than their 40% entitlement.
    #[test]
    fn penalty_splits_conserve_and_never_overpay(penalty in 0_u64..1_000_000_000_000_000) {
        let p = fees::split_liquidation_penalty(penalty).unwrap();
        prop_assert_eq!(p.liquidator + p.insurance + p.treasury, penalty);
        prop_assert!(u128::from(p.liquidator) <= (u128::from(penalty) * 4_000) / 10_000);
    }

    /// The published schedule must never charge a higher rate to a higher-volume trader.
    #[test]
    fn fee_rate_never_rises_with_volume(volumes in (0_u64..u64::MAX, 0_u64..u64::MAX)) {
        let (low, high) = ordered(volumes);
        prop_assert!(
            fees::fee_rate_for_volume(low) >= fees::fee_rate_for_volume(high)
        );
    }
}

// --- margin -------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(2048))]

    /// Margin requirements round up, so the protocol always holds at least the exact figure.
    #[test]
    fn margin_requirements_round_up(
        notional in 1_u64..1_000_000_000_000_000,
        bps in 1_u16..5_000,
    ) {
        let im = margin::initial_margin(notional, bps).unwrap();
        let exact = (u128::from(notional) * u128::from(bps)) / 10_000;
        prop_assert!(u128::from(im) >= exact);
        prop_assert!(u128::from(im) <= exact + 1);
    }

    /// A bigger position never has a smaller MMR multiplier.
    #[test]
    fn mmr_multiplier_never_falls_with_size(
        sizes in (0_u64..10_000_000_000_000, 0_u64..10_000_000_000_000),
    ) {
        let (small, large) = ordered(sizes);
        prop_assert!(
            margin::mmr_multiplier_bps(small) <= margin::mmr_multiplier_bps(large)
        );
    }

    /// The displayed health factor and the liquidation decision must never disagree —
    /// otherwise the UI shows "safe" on a position a keeper is about to close.
    #[test]
    fn health_factor_agrees_with_the_liquidation_test(
        equity in -1_000_000_000_000_i64..1_000_000_000_000,
        mm in 1_u64..1_000_000_000_000,
    ) {
        let hf = margin::health_factor_bps(equity, mm).unwrap();
        prop_assert_eq!(
            hf < margin::HEALTH_FACTOR_ONE,
            margin::is_liquidatable(equity, mm)
        );
    }

    /// Every accrued cost strictly reduces equity. A cost that increased it would be a
    /// direct drain on the vault.
    #[test]
    fn costs_only_ever_reduce_equity(
        collateral in 0_u64..1_000_000_000_000,
        upnl in -1_000_000_000_i64..1_000_000_000,
        carry in 0_u64..1_000_000,
        close_fee in 0_u64..1_000_000,
    ) {
        let clean = margin::equity(collateral, upnl, AccruedCosts::default()).unwrap();
        let dirty = margin::equity(
            collateral,
            upnl,
            AccruedCosts { carry, funding: 0, close_fee },
        ).unwrap();
        prop_assert!(dirty <= clean);
    }

    /// The displayed liquidation price must be **conservative**: at that price the position
    /// is still (just) alive, and real liquidation happens a tick or two beyond it.
    ///
    /// # Why it is not exact
    ///
    /// Solving for the price converts a quote-unit budget into a price move and floors it.
    /// Recomputing PnL from that floored price amplifies the discarded fraction by
    /// `size / NOTIONAL_DIVISOR` — the position's value per price tick. A position holding
    /// 5 quote units per tick cannot have its liquidation price pinned closer than 5 units
    /// of equity; that is a property of fixed-point granularity, not a bug.
    ///
    /// What matters is the **direction**. Flooring the move pulls the liquidation price
    /// toward entry for both longs and shorts, so the number shown to the trader always
    /// arrives at or before the real one. Showing it late would be the dangerous error.
    #[test]
    fn the_displayed_liquidation_price_is_conservative(
        size in 1_000_000_000_000_u64..100_000_000_000_000,
        entry in 500_000_000_i64..5_000_000_000,
        collateral in 1_000_000_u64..100_000_000_000,
        mm in 1_u64..1_000_000_000,
        direction in direction_strategy(),
    ) {
        prop_assume!(collateral > mm);
        let liq = margin::liquidation_price(
            size, entry, collateral, mm, AccruedCosts::default(), direction,
        ).unwrap();
        prop_assume!(liq > 1); // not clamped

        let at_liq = pnl::upnl_in_quote(size, entry, liq, direction).unwrap();
        let equity_at_liq =
            margin::equity(collateral, at_liq, AccruedCosts::default()).unwrap();

        // Never liquidatable *before* the displayed price.
        prop_assert!(
            !margin::is_liquidatable(equity_at_liq, mm),
            "displayed liquidation price fires early: equity {equity_at_liq} vs mm {mm}"
        );

        // And never further out than one tick of granularity plus a rounding unit.
        let tick_value = i128::from(size) / 1_000_000_000_000;
        let gap = i128::from(equity_at_liq) - i128::from(mm);
        prop_assert!(
            gap <= tick_value + 1,
            "gap {gap} exceeds the {tick_value}-per-tick granularity of this size"
        );
    }

    /// One tick beyond the displayed price, the position really is liquidatable — the
    /// display is tight, not arbitrarily early.
    #[test]
    fn liquidation_bites_just_beyond_the_displayed_price(
        size in 1_000_000_000_000_u64..100_000_000_000_000,
        entry in 500_000_000_i64..5_000_000_000,
        collateral in 1_000_000_u64..100_000_000_000,
        mm in 1_u64..1_000_000_000,
        direction in direction_strategy(),
    ) {
        prop_assume!(collateral > mm);
        let liq = margin::liquidation_price(
            size, entry, collateral, mm, AccruedCosts::default(), direction,
        ).unwrap();
        prop_assume!(liq > 2);

        // A long dies below its liquidation price, a short above it. Step two ticks past
        // to clear the granularity described above.
        let beyond = match direction {
            Direction::Long => liq - 2,
            Direction::Short => liq + 2,
        };
        let at_beyond = pnl::upnl_in_quote(size, entry, beyond, direction).unwrap();
        let equity_beyond =
            margin::equity(collateral, at_beyond, AccruedCosts::default()).unwrap();

        prop_assert!(
            margin::is_liquidatable(equity_beyond, mm),
            "position survived past its liquidation price: equity {equity_beyond} vs mm {mm}"
        );
    }
}

// --- funding ------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(2048))]

    /// Invariant I3: funding is a transfer between traders and must never cost the
    /// protocol. Longs' payments always cover shorts' receipts.
    #[test]
    fn funding_never_pays_out_more_than_it_collects(
        long_oi in 1_i128..1_000_000_000_000_000,
        short_oi in 1_i128..1_000_000_000_000_000,
        rate in -1_000_000_000_i64..1_000_000_000,
        elapsed in 1_i64..86_400,
    ) {
        let u = funding::funding_index_update(long_oi, short_oi, rate, elapsed).unwrap();
        let net = long_oi * u.long_delta + short_oi * u.short_delta;
        prop_assert!(net >= 0, "funding paid out {net} more than it collected");
    }

    /// The heavy side always pays and the light side always receives.
    #[test]
    fn funding_always_charges_the_crowded_side(
        long_oi in 1_i128..1_000_000_000_000_000,
        short_oi in 1_i128..1_000_000_000_000_000,
        k in 1_u64..2_000_000_000,
        cap in 1_i64..1_000_000_000,
    ) {
        let rate = funding::funding_rate_per_hour(long_oi, short_oi, k, cap).unwrap();
        if long_oi > short_oi {
            prop_assert!(rate >= 0, "crowded longs were paid");
        } else if short_oi > long_oi {
            prop_assert!(rate <= 0, "crowded shorts were paid");
        } else {
            prop_assert_eq!(rate, 0);
        }
    }

    /// The funding rate never exceeds its per-market cap in either direction.
    #[test]
    fn funding_rate_respects_its_cap(
        long_oi in 1_i128..1_000_000_000_000_000,
        short_oi in 1_i128..1_000_000_000_000_000,
        k in 0_u64..u64::from(u32::MAX),
        cap in 0_i64..1_000_000_000,
    ) {
        let rate = funding::funding_rate_per_hour(long_oi, short_oi, k, cap).unwrap();
        prop_assert!(rate.abs() <= cap);
    }

    /// The carry index is monotonic. A position that snapshotted it must never see it move
    /// backwards, or its accrued carry underflows.
    #[test]
    fn the_carry_index_never_moves_backwards(
        start in 0_u128..1_000_000_000_000,
        rate in -1_000_000_i64..1_000_000,
        elapsed in -100_i64..86_400,
    ) {
        let next = funding::advance_carry_index(start, rate, elapsed).unwrap();
        prop_assert!(next >= start);
    }

    /// A matched long/short pair must never cost the protocol carry.
    #[test]
    fn a_matched_carry_pair_never_costs_the_protocol(
        base in 0_i64..1_000_000_000,
        quote in 0_i64..1_000_000_000,
        markup in 0_u64..1_000_000,
    ) {
        let long =
            funding::carry_cost_rate_per_hour(base, quote, markup, Direction::Long).unwrap();
        let short =
            funding::carry_cost_rate_per_hour(base, quote, markup, Direction::Short).unwrap();
        prop_assert!(i128::from(long) + i128::from(short) >= 0);
    }

    /// Carry is a charge, so it rounds up — never below the exact value.
    #[test]
    fn accrued_carry_rounds_up(
        basis in 1_u64..1_000_000_000_000,
        delta in 0_u128..1_000_000_000,
    ) {
        let carry = funding::accrued_carry(basis, delta, 0).unwrap();
        let exact = (u128::from(basis) * delta) / RATE_PRECISION;
        prop_assert!(u128::from(carry) >= exact);
        prop_assert!(u128::from(carry) <= exact + 1);
    }
}

// --- fixed-point primitives ---------------------------------------------------------------

proptest! {
    #![proptest_config(config(4096))]

    /// Ceil is never below floor, and never more than one unit above it.
    #[test]
    fn floor_and_ceil_bracket_the_exact_quotient(
        a in 0_u128..1_000_000_000_000_000_000,
        b in 0_u128..1_000_000_000,
        d in 1_u128..1_000_000_000,
    ) {
        let f = fixed::mul_div_floor(a, b, d).unwrap();
        let c = fixed::mul_div_ceil(a, b, d).unwrap();
        prop_assert!(c >= f);
        prop_assert!(c - f <= 1);
        prop_assert_eq!(f * d <= a * b, true);
    }

    /// Signed floor rounds toward −∞ for every sign, unlike Rust's `/`.
    #[test]
    fn signed_floor_always_rounds_down(
        a in -1_000_000_000_000_i128..1_000_000_000_000,
        b in -1_000_000_i128..1_000_000,
        d in 1_i128..1_000_000_000,
    ) {
        let q = fixed::mul_div_floor_signed(a, b, d).unwrap();
        let exact = a * b;
        prop_assert!(q * d <= exact, "{q} * {d} exceeded {exact}");
        prop_assert!((q + 1) * d > exact, "{q} was not the tightest floor");
    }

    /// Signed ceil rounds toward +∞ for every sign.
    #[test]
    fn signed_ceil_always_rounds_up(
        a in -1_000_000_000_000_i128..1_000_000_000_000,
        b in -1_000_000_i128..1_000_000,
        d in 1_i128..1_000_000_000,
    ) {
        let q = fixed::mul_div_ceil_signed(a, b, d).unwrap();
        let exact = a * b;
        prop_assert!(q * d >= exact, "{q} * {d} fell below {exact}");
        prop_assert!((q - 1) * d < exact, "{q} was not the tightest ceiling");
    }

    /// Narrowing conversions must reject anything they cannot represent, never truncate.
    #[test]
    fn narrowing_never_silently_truncates(v in 0_u128..u128::MAX) {
        match fixed::to_u64(v) {
            Ok(n) => prop_assert_eq!(u128::from(n), v),
            Err(_) => prop_assert!(v > u128::from(u64::MAX)),
        }
    }

    /// Clamping is idempotent and always lands inside the bound.
    #[test]
    fn clamp_is_idempotent_and_bounded(
        v in i128::MIN / 2..i128::MAX / 2,
        bound in 0_i128..1_000_000_000_000,
    ) {
        let once = fixed::clamp_symmetric(v, bound).unwrap();
        let twice = fixed::clamp_symmetric(once, bound).unwrap();
        prop_assert_eq!(once, twice);
        prop_assert!(once.abs() <= bound);
    }
}

// --- oracle -------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(4096))]

    /// The exponent that matches PRICE_PRECISION must leave the mantissa untouched.
    #[test]
    fn exponent_of_negative_nine_is_the_identity(price in 1_i64..i64::MAX / 2) {
        prop_assert_eq!(oracle::normalize_price(price, -9).unwrap(), price);
    }

    /// Normalisation must preserve ordering. A rescaler that inverted two prices anywhere in
    /// its range would silently reorder the book.
    #[test]
    fn normalisation_is_monotonic_in_price(
        a in 1_i64..1_000_000_000_000,
        b in 1_i64..1_000_000_000_000,
        expo in -12_i32..0,
    ) {
        if let (Ok(na), Ok(nb)) = (
            oracle::normalize_price(a, expo),
            oracle::normalize_price(b, expo),
        ) {
            prop_assert_eq!(a < b, na < nb, "ordering flipped at expo {}", expo);
        }
    }

    /// Price floors and confidence ceils. For the same mantissa the confidence must never
    /// land below the price, or the rounding pair would be leaking the band.
    #[test]
    fn confidence_never_normalises_below_price(
        mantissa in 1_u64..1_000_000_000_000,
        expo in -12_i32..0,
    ) {
        let p = oracle::normalize_price(mantissa as i64, expo);
        let c = oracle::normalize_conf(mantissa, expo);
        if let (Ok(p), Ok(c)) = (p, c) {
            prop_assert!(c >= p as u64, "conf {} < price {}", c, p);
        }
    }

    /// A non-zero confidence must never round away to nothing. If it could, a market could
    /// be quoted as if it were certain when it is not.
    ///
    /// Confidence is generated **relative to price** rather than independently. An
    /// independent range lets the two be paired into a ratio above 65,535 bps, which
    /// `max_conf_bps: u16` cannot express — so the ceiling rejects the input before the
    /// rounding behaviour under test is ever reached. Bounding conf at half the price keeps
    /// every case inside the representable domain and still covers the interesting end,
    /// where conf is a tiny fraction of price and rounding to zero is the real risk.
    ///
    /// The original generator only failed at ~100k cases, which is exactly why the deep run
    /// exists as a separate CI job.
    #[test]
    fn a_non_zero_confidence_never_rounds_to_zero_bps(
        (price, conf) in (1_000_000_i64..100_000_000_000_000)
            .prop_flat_map(|p| (Just(p), 1_u64..=(p as u64 / 2))),
    ) {
        let v = oracle::ValidatedPrice::new(price, conf, 0, u16::MAX).unwrap();
        prop_assert!(v.conf_bps >= 1, "a real band reported 0 bps");
    }

    /// The freshness window is closed at both ends, and open exactly on the boundary.
    #[test]
    fn freshness_accepts_exactly_the_configured_window(
        publish_time in -1_000_000_000_i64..1_000_000_000,
        age in -100_i64..100,
        max_stale in 0_u32..60,
        max_drift in 0_u32..10,
    ) {
        let now = publish_time + age;
        let result = oracle::validate_publish_time(publish_time, now, max_stale, max_drift);
        let expected_ok = age <= max_stale as i64 && -age <= max_drift as i64;
        prop_assert_eq!(result.is_ok(), expected_ok);
    }

    /// Deviation must not depend on which side of the reference the price fell.
    #[test]
    fn deviation_is_symmetric_about_the_reference(
        reference in 1_000_000_i64..1_000_000_000_000,
        delta in 1_i64..500_000,
    ) {
        prop_assume!(reference > delta);
        let up = oracle::deviation_bps(reference + delta, reference).unwrap();
        let down = oracle::deviation_bps(reference - delta, reference).unwrap();
        prop_assert_eq!(up, down);
    }

    /// Composition must never report a tighter band than either leg carried. This is the
    /// property that makes the linear-sum choice safe: uncertainty only accumulates.
    #[test]
    fn composition_never_narrows_the_confidence_band(
        base_price in 1_000_000_000_i64..10_000_000_000,
        quote_price in 1_000_000_000_i64..10_000_000_000,
        base_conf in 1_u64..10_000_000,
        quote_conf in 1_u64..10_000_000,
        invert in any::<bool>(),
    ) {
        let a = oracle::ValidatedPrice::new(base_price, base_conf, 0, u16::MAX).unwrap();
        let b = oracle::ValidatedPrice::new(quote_price, quote_conf, 0, u16::MAX).unwrap();
        let cross = oracle::compose_synthetic(a, b, invert, u16::MAX).unwrap();
        prop_assert!(
            cross.conf_bps >= a.conf_bps.max(b.conf_bps),
            "composed {} bps below legs {} / {}", cross.conf_bps, a.conf_bps, b.conf_bps
        );
    }

    /// Multiplicative composition is order-independent. If it were not, a cross and its
    /// mirror would price differently and the difference would be arbitrageable.
    #[test]
    fn multiplicative_composition_is_commutative(
        a_price in 1_000_000_000_i64..100_000_000_000,
        b_price in 1_000_000_000_i64..100_000_000_000,
        a_conf in 0_u64..1_000_000,
        b_conf in 0_u64..1_000_000,
    ) {
        let a = oracle::ValidatedPrice::new(a_price, a_conf, 0, u16::MAX).unwrap();
        let b = oracle::ValidatedPrice::new(b_price, b_conf, 0, u16::MAX).unwrap();
        let ab = oracle::compose_synthetic(a, b, false, u16::MAX).unwrap();
        let ba = oracle::compose_synthetic(b, a, false, u16::MAX).unwrap();
        prop_assert_eq!(ab.price, ba.price);
        prop_assert_eq!(ab.conf, ba.conf);
    }

    /// Dividing by a leg and multiplying it back must return to the starting price, within
    /// the truncation two floored divisions can introduce.
    #[test]
    fn divide_then_multiply_returns_to_the_original_price(
        base in 1_000_000_000_i64..100_000_000_000,
        leg in 1_000_000_000_i64..100_000_000_000,
    ) {
        let b = oracle::ValidatedPrice::new(base, 0, 0, u16::MAX).unwrap();
        let l = oracle::ValidatedPrice::new(leg, 0, 0, u16::MAX).unwrap();
        let divided = oracle::compose_synthetic(b, l, true, u16::MAX).unwrap();
        let back = oracle::compose_synthetic(divided, l, false, u16::MAX).unwrap();
        // Each compose floors once; the error scales with the leg.
        let tolerance = (leg / 1_000_000_000).max(1) + 1;
        prop_assert!(
            (back.price - base).abs() <= tolerance,
            "round trip {} -> {} exceeded tolerance {}", base, back.price, tolerance
        );
    }

    /// Every oracle entry point returns an error rather than panicking, for any input.
    #[test]
    fn no_oracle_input_can_panic(
        price in any::<i64>(),
        conf in any::<u64>(),
        expo in -40_i32..40,
        publish_time in any::<i64>(),
        now in any::<i64>(),
        max_conf in any::<u16>(),
    ) {
        let _ = oracle::normalize_price(price, expo);
        let _ = oracle::normalize_conf(conf, expo);
        let _ = oracle::validate_publish_time(publish_time, now, 30, 5);
        let _ = oracle::deviation_bps(price, price);
        let _ = oracle::ValidatedPrice::new(price, conf, publish_time, max_conf);
    }
}

// --- cross-module consistency -------------------------------------------------------------

proptest! {
    #![proptest_config(config(1024))]

    /// A trade's cost must always exceed the compute cost of liquidating it, or liquidations
    /// stop being worth running — and unliquidated positions are how vaults die
    /// (`ARCHITECTURE.md` § 6.8).
    #[test]
    fn liquidator_reward_stays_above_transaction_cost(
        notional in 10_000_000_000_u64..1_000_000_000_000_000, // >= $10k
    ) {
        let penalty = fees::fee_amount(notional, 50 * ONE_BPS_RATE).unwrap(); // 0.5%
        let split = fees::split_liquidation_penalty(penalty).unwrap();
        // ~$0.01 covers a 200k-CU transaction plus a generous priority fee.
        prop_assert!(split.liquidator > 10_000, "reward {} too small", split.liquidator);
    }
}
