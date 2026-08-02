//! Execution pricing: confidence spread, skew impact, and adverse-side selection.
//!
//! `ARCHITECTURE.md` § 6.6 calls this the most important function in the protocol. Get it
//! wrong and the vault leaks continuously to anyone with a faster feed than Pyth (threat T2,
//! and correction C-4).
//!
//! Three mechanisms, all required:
//!
//! 1. **Confidence spread** — charge more precisely when the oracle knows less.
//! 2. **Skew impact** — make it progressively expensive to push the book further one-sided.
//! 3. **Adverse-side selection** — the trader always crosses the side that hurts them,
//!    which removes the free half-spread that latency arbitrageurs harvest.

use crate::constants::{BPS_PRECISION, LOT_SIZE_BASE, MIN_NOTIONAL_QUOTE, RATE_PRECISION};
use crate::error::{MathError, MathResult};
use crate::fixed::{
    abs_i128, i128_to_u128, mul_div_ceil, mul_div_floor, positive_to_u128, to_i64, to_u64,
};
use crate::types::Side;

/// A spread this wide would drive the sell price to zero or below. Rejected outright —
/// no legitimate market condition produces it, so it means a misconfigured market.
pub const MAX_TOTAL_SPREAD_BPS: u64 = 5_000; // 50%

/// `conf / price` in basis points, rounded **up** so a borderline feed is treated as the
/// wider of the two readings.
pub fn confidence_bps(price: i64, conf: u64) -> MathResult<u64> {
    let p = positive_to_u128(price)?;
    let c = mul_div_ceil(u128::from(conf), BPS_PRECISION, p)?;
    to_u64(c)
}

/// Reject a price whose confidence exceeds the market's ceiling.
///
/// This is a hard gate, not a spread adjustment: past this point the oracle is not saying
/// anything precise enough to liquidate against.
pub fn validate_confidence(confidence_bps: u64, max_conf_bps: u16) -> MathResult<()> {
    if confidence_bps > u64::from(max_conf_bps) {
        return Err(MathError::ConfidenceTooWide);
    }
    Ok(())
}

/// The portion of the spread driven by oracle uncertainty.
///
/// `confidence_bps × multiplier / BPS_PRECISION`, rounded **up**. A multiplier of
/// `10_000` charges the full confidence band; higher values charge a premium on top.
pub fn confidence_spread_bps(confidence_bps: u64, multiplier_bps: u16) -> MathResult<u64> {
    let s = mul_div_ceil(
        u128::from(confidence_bps),
        u128::from(multiplier_bps),
        BPS_PRECISION,
    )?;
    to_u64(s)
}

/// How much this trade worsens the book's imbalance, in base units.
///
/// Positive means the trade pushes the book further one-sided; negative means it helps
/// balance it. `signed_size_delta` is `+size` for a long open or short close, `−size`
/// for the reverse.
pub fn skew_delta(
    base_oi_long: i128,
    base_oi_short: i128,
    signed_size_delta: i128,
) -> MathResult<i128> {
    let before = base_oi_long
        .checked_sub(base_oi_short)
        .ok_or(MathError::Overflow)?;
    let after = before
        .checked_add(signed_size_delta)
        .ok_or(MathError::Overflow)?;
    abs_i128(after)?
        .checked_sub(abs_i128(before)?)
        .ok_or(MathError::Overflow)
}

/// The portion of the spread driven by inventory risk.
///
/// `rate` is in [`RATE_PRECISION`] units and means *basis points of spread added per
/// standard lot by which this trade worsens the imbalance*. So `1_000_000` (0.001) adds
/// 0.001 bps per lot: a 100-lot imbalance increase costs 0.1 bps.
///
/// Trades that **improve** the balance return `0` — v1 charges nothing rather than paying
/// a rebate, so this can never widen a spread in the trader's favour
/// (`ARCHITECTURE.md` § 6.6 specifies `max(skew_impact, 0)`).
pub fn skew_impact_bps(skew_delta: i128, rate: u32) -> MathResult<u64> {
    if skew_delta <= 0 || rate == 0 {
        return Ok(0);
    }
    let denominator = LOT_SIZE_BASE
        .checked_mul(RATE_PRECISION)
        .ok_or(MathError::Overflow)?;
    let impact = mul_div_ceil(i128_to_u128(skew_delta)?, u128::from(rate), denominator)?;
    to_u64(impact)
}

/// Sum the three spread components, with an upper bound.
///
/// Every component rounds up individually, so the total is always at least as wide as the
/// true figure — never narrower.
pub fn total_spread_bps(
    base_spread_bps: u16,
    confidence_spread_bps: u64,
    skew_impact_bps: u64,
) -> MathResult<u64> {
    let total = u64::from(base_spread_bps)
        .checked_add(confidence_spread_bps)
        .ok_or(MathError::Overflow)?
        .checked_add(skew_impact_bps)
        .ok_or(MathError::Overflow)?;
    if total > MAX_TOTAL_SPREAD_BPS {
        return Err(MathError::InvalidParameter);
    }
    Ok(total)
}

/// The price the trader actually fills at.
///
/// The adjustment is rounded **up** and then applied against them in both directions:
/// a buyer pays oracle + adjustment, a seller receives oracle − adjustment.
///
/// Because the adjustment is ceiling-rounded, any non-zero spread moves the price by at
/// least one unit. That is what makes a same-price round trip strictly loss-making, which
/// [`crate::pricing`]'s round-trip property test asserts across the parameter space.
pub fn execution_price(oracle_price: i64, total_spread_bps: u64, side: Side) -> MathResult<i64> {
    if total_spread_bps > MAX_TOTAL_SPREAD_BPS {
        return Err(MathError::InvalidParameter);
    }
    let p = positive_to_u128(oracle_price)?;
    let adjustment = mul_div_ceil(p, u128::from(total_spread_bps), BPS_PRECISION)?;

    let exec = match side {
        Side::Buy => p.checked_add(adjustment).ok_or(MathError::Overflow)?,
        Side::Sell => p.checked_sub(adjustment).ok_or(MathError::Overflow)?,
    };
    if exec == 0 {
        return Err(MathError::InvalidPrice);
    }
    to_i64(crate::fixed::u128_to_i128(exec)?)
}

/// Reject a trade whose notional is too small for fees and spread to round to anything.
///
/// Below [`MIN_NOTIONAL_QUOTE`] a 1 bp fee floors to zero and the round-trip cost of a
/// trade can reach zero — the rounding-loop drain in `ARCHITECTURE.md` § 13.1 T14.
pub fn validate_notional(notional: u64) -> MathResult<()> {
    if notional < MIN_NOTIONAL_QUOTE {
        return Err(MathError::NotionalTooSmall);
    }
    Ok(())
}

/// Check a fill against the trader's slippage bound.
///
/// `limit` is a maximum for a buy and a minimum for a sell.
pub fn validate_slippage(exec_price: i64, limit: i64, side: Side) -> MathResult<()> {
    let acceptable = match side {
        Side::Buy => exec_price <= limit,
        Side::Sell => exec_price >= limit,
    };
    if acceptable {
        Ok(())
    } else {
        Err(MathError::InvalidPrice)
    }
}

/// Convert a price difference into pips for display (`ARCHITECTURE.md` § 10.2).
///
/// `pip_size` is the price-precision value of one pip: `100_000` for a 4-decimal pair,
/// `10_000_000` for a JPY pair. Floors, because this is presentation only.
pub fn price_delta_to_pips(delta: i64, pip_size: u64) -> MathResult<i64> {
    if pip_size == 0 {
        return Err(MathError::DivideByZero);
    }
    let magnitude = mul_div_floor(
        i128_to_u128(abs_i128(i128::from(delta))?)?,
        1,
        u128::from(pip_size),
    )?;
    let pips = to_i64(crate::fixed::u128_to_i128(magnitude)?)?;
    if delta < 0 {
        pips.checked_neg().ok_or(MathError::Overflow)
    } else {
        Ok(pips)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EURUSD: i64 = 1_085_430_000; // 1.08543
    const PIP: u64 = 100_000;

    #[test]
    fn confidence_bps_matches_the_measured_eurusd_figure() {
        // oracle-feasibility.md § 2: EUR/USD confidence p50 is 1.28 bps.
        // 1.28 bps of 1.08543 is ~0.000139, i.e. ~139_000 price units.
        let conf = 139_000_u64;
        assert_eq!(confidence_bps(EURUSD, conf).unwrap(), 2); // ceil(1.28) = 2
    }

    /// Confidence rounds up so a borderline feed reads as the wider of the two.
    #[test]
    fn confidence_bps_rounds_up() {
        // Exactly 1 bp.
        assert_eq!(confidence_bps(10_000, 1).unwrap(), 1);
        // A hair over 1 bp must not read as 1.
        assert_eq!(confidence_bps(10_000, 2).unwrap(), 2);
    }

    #[test]
    fn confidence_gate_rejects_only_above_the_ceiling() {
        assert!(validate_confidence(15, 15).is_ok());
        assert_eq!(
            validate_confidence(16, 15),
            Err(MathError::ConfidenceTooWide)
        );
    }

    /// oracle-feasibility.md: USD/IDR failed at 30.11 bps p95 against a 25 bps limit.
    #[test]
    fn measured_feed_verdicts_reproduce() {
        assert!(validate_confidence(3, 15).is_ok()); // XAU/USD 2.84 p95
        assert!(validate_confidence(5, 15).is_ok()); // EUR/USD 4.08 p95
        assert_eq!(
            validate_confidence(31, 25),
            Err(MathError::ConfidenceTooWide)
        ); // USD/IDR
    }

    #[test]
    fn confidence_spread_scales_with_the_multiplier() {
        assert_eq!(confidence_spread_bps(10, 10_000).unwrap(), 10); // 1.0x
        assert_eq!(confidence_spread_bps(10, 15_000).unwrap(), 15); // 1.5x
        assert_eq!(confidence_spread_bps(10, 5_000).unwrap(), 5); // 0.5x
    }

    #[test]
    fn skew_delta_is_positive_when_worsening_the_imbalance() {
        // Book already long 100; adding another 50 long worsens it.
        assert_eq!(skew_delta(100, 0, 50).unwrap(), 50);
        // Adding 50 short improves it.
        assert_eq!(skew_delta(100, 0, -50).unwrap(), -50);
        // Balanced book: any trade worsens it.
        assert_eq!(skew_delta(100, 100, 50).unwrap(), 50);
    }

    /// Crossing through balance: a 150 short against a 100 long leaves net −50,
    /// so the imbalance shrinks by 50.
    #[test]
    fn skew_delta_handles_crossing_through_zero() {
        assert_eq!(skew_delta(100, 0, -150).unwrap(), -50);
    }

    #[test]
    fn skew_impact_is_zero_when_improving_the_book() {
        assert_eq!(skew_impact_bps(-1_000_000_000_000, 1_000_000).unwrap(), 0);
        assert_eq!(skew_impact_bps(0, 1_000_000).unwrap(), 0);
    }

    /// Rate 1e6 (0.001 in RATE_PRECISION) = 0.001 bps per lot; 100 lots -> 0.1 bps,
    /// which ceils to 1.
    #[test]
    fn skew_impact_scales_with_lots_of_imbalance() {
        let one_lot = LOT_SIZE_BASE;
        let hundred_lots = i128::try_from(one_lot).unwrap() * 100;
        assert_eq!(skew_impact_bps(hundred_lots, 1_000_000).unwrap(), 1);
        let ten_thousand_lots = i128::try_from(one_lot).unwrap() * 10_000;
        assert_eq!(skew_impact_bps(ten_thousand_lots, 1_000_000).unwrap(), 10);
    }

    #[test]
    fn skew_impact_is_disabled_by_a_zero_rate() {
        let huge = i128::try_from(LOT_SIZE_BASE).unwrap() * 1_000_000;
        assert_eq!(skew_impact_bps(huge, 0).unwrap(), 0);
    }

    #[test]
    fn total_spread_sums_components_and_bounds_them() {
        assert_eq!(total_spread_bps(1, 2, 3).unwrap(), 6);
        assert_eq!(
            total_spread_bps(0, MAX_TOTAL_SPREAD_BPS + 1, 0),
            Err(MathError::InvalidParameter)
        );
    }

    /// The core anti-arbitrage property: a buyer pays more than mid, a seller receives less.
    #[test]
    fn execution_price_is_always_adverse() {
        let buy = execution_price(EURUSD, 10, Side::Buy).unwrap();
        let sell = execution_price(EURUSD, 10, Side::Sell).unwrap();
        assert!(buy > EURUSD, "buyer must pay above mid");
        assert!(sell < EURUSD, "seller must receive below mid");
        assert!(buy > sell, "the book must never be crossed");
    }

    /// 10 bps of 1.08543 is 0.00108543, so buy 1.08651543 and sell 1.08434457.
    #[test]
    fn execution_price_applies_the_exact_spread() {
        assert_eq!(
            execution_price(EURUSD, 10, Side::Buy).unwrap(),
            1_086_515_430
        );
        assert_eq!(
            execution_price(EURUSD, 10, Side::Sell).unwrap(),
            1_084_344_570
        );
    }

    /// Zero spread is the only case where a fill happens at mid — and the fee layer
    /// still makes the round trip loss-making.
    #[test]
    fn zero_spread_fills_at_mid() {
        assert_eq!(execution_price(EURUSD, 0, Side::Buy).unwrap(), EURUSD);
        assert_eq!(execution_price(EURUSD, 0, Side::Sell).unwrap(), EURUSD);
    }

    /// Ceiling rounding guarantees at least one unit of adverse movement, however small
    /// the spread. Without this a sub-unit spread would round away and hand out free fills.
    ///
    /// Prices span the listable set: XAU/USD near 4,000 down to a deeply devalued EM rate.
    #[test]
    fn any_nonzero_spread_moves_the_price_at_least_one_unit() {
        let prices = [
            1_000_000_i64,     // 0.001 — far below any listed market
            85_430_000,        // 0.0854
            1_085_430_000,     // EUR/USD 1.08543
            157_200_000_000,   // USD/JPY 157.20
            4_046_950_000_000, // XAU/USD 4,046.95
            88_500_000_000,    // USD/INR 88.50
        ];
        for price in prices {
            let buy = execution_price(price, 1, Side::Buy).unwrap();
            let sell = execution_price(price, 1, Side::Sell).unwrap();
            assert!(buy > price, "buy did not move at price {price}");
            assert!(sell < price, "sell did not move at price {price}");
        }
    }

    /// The guarantee above has a floor: a price small enough that the spread consumes it
    /// entirely cannot produce a bid, and is rejected rather than filled at zero. No real
    /// market reaches this — PRICE_PRECISION is 1e9, so it needs a price under 1e-5.
    #[test]
    fn a_price_too_small_to_carry_the_spread_is_rejected() {
        assert_eq!(
            execution_price(1, 1, Side::Sell),
            Err(MathError::InvalidPrice)
        );
        // 10_000 units at 1 bp: adjustment is exactly 1, leaving a valid bid.
        assert!(execution_price(10_000, 1, Side::Sell).is_ok());
    }

    #[test]
    fn execution_price_rejects_a_spread_that_would_zero_the_bid() {
        assert_eq!(
            execution_price(EURUSD, MAX_TOTAL_SPREAD_BPS + 1, Side::Sell),
            Err(MathError::InvalidParameter)
        );
    }

    /// At the maximum permitted spread a price of 1 unit would floor the bid to zero.
    #[test]
    fn execution_price_rejects_a_zero_result() {
        assert_eq!(
            execution_price(1, MAX_TOTAL_SPREAD_BPS, Side::Sell),
            Err(MathError::InvalidPrice)
        );
    }

    #[test]
    fn execution_price_rejects_nonpositive_oracle_input() {
        assert_eq!(
            execution_price(0, 10, Side::Buy),
            Err(MathError::InvalidPrice)
        );
        assert_eq!(
            execution_price(-1, 10, Side::Buy),
            Err(MathError::InvalidPrice)
        );
    }

    #[test]
    fn notional_floor_is_enforced() {
        assert!(validate_notional(MIN_NOTIONAL_QUOTE).is_ok());
        assert_eq!(
            validate_notional(MIN_NOTIONAL_QUOTE - 1),
            Err(MathError::NotionalTooSmall)
        );
    }

    #[test]
    fn slippage_bound_is_directional() {
        // Buyer accepts anything at or below their limit.
        assert!(validate_slippage(100, 100, Side::Buy).is_ok());
        assert!(validate_slippage(99, 100, Side::Buy).is_ok());
        assert_eq!(
            validate_slippage(101, 100, Side::Buy),
            Err(MathError::InvalidPrice)
        );
        // Seller accepts anything at or above theirs.
        assert!(validate_slippage(100, 100, Side::Sell).is_ok());
        assert!(validate_slippage(101, 100, Side::Sell).is_ok());
        assert_eq!(
            validate_slippage(99, 100, Side::Sell),
            Err(MathError::InvalidPrice)
        );
    }

    #[test]
    fn pip_conversion_matches_the_explainer() {
        // FOREX-EXPLAINED.md § 2: 1.0850 -> 1.0900 is 50 pips.
        let delta = 1_090_000_000 - 1_085_000_000;
        assert_eq!(price_delta_to_pips(delta, PIP).unwrap(), 50);
        assert_eq!(price_delta_to_pips(-delta, PIP).unwrap(), -50);
    }

    /// A JPY pair's pip is the 2nd decimal, not the 4th.
    #[test]
    fn pip_conversion_handles_jpy_scale() {
        let jpy_pip = 10_000_000_u64; // 0.01 at PRICE_PRECISION
        let delta = 157_300_000_000_i64 - 157_200_000_000;
        assert_eq!(price_delta_to_pips(delta, jpy_pip).unwrap(), 10);
    }

    #[test]
    fn pip_conversion_rejects_zero_pip_size() {
        assert_eq!(price_delta_to_pips(1, 0), Err(MathError::DivideByZero));
    }
}
