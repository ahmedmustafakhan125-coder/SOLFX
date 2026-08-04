//! Oracle price normalisation, freshness and sanity checks — the arithmetic half of
//! `ARCHITECTURE.md` § 7.1.
//!
//! # Why this is split across two crates
//!
//! § 7.1 puts `load_validated_price` inside the program. It is split here:
//!
//! - **This module** holds everything that is pure arithmetic: exponent normalisation,
//!   staleness comparison, deviation measurement, synthetic composition. No Pyth types,
//!   no Anchor types, no I/O — so it is exhaustively property-testable in milliseconds.
//! - **`solfx_core::oracle`** holds the ten lines that touch `PriceUpdateV2` and calls
//!   straight into this module.
//!
//! The single-choke-point guarantee that ADR-006 asks for is preserved: there is still
//! exactly one path from a Pyth account to a price the engine will trade on, and the part
//! of it that can be wrong in an interesting way lives here, under test.
//!
//! # Fail closed
//!
//! Every function in this module returns an error rather than a fallback value. There is no
//! "use the last known price" path, because that is the C-1 exploit written as a helper.

use crate::constants::{BPS_PRECISION, PRICE_PRECISION, RATE_PRECISION};
use crate::error::{MathError, MathResult};
use crate::fixed::{
    div_ceil, div_floor, mul_div_ceil, mul_div_floor, positive_to_u128, to_i64, to_u64,
};

/// A price that has passed every gate in [`ARCHITECTURE.md` § 7.1], normalised to
/// [`PRICE_PRECISION`].
///
/// Constructing one of these is the *only* way the engine obtains a tradeable price. It has
/// no public constructor that skips validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedPrice {
    /// Strictly positive, at [`PRICE_PRECISION`].
    pub price: i64,
    /// Confidence half-width, same scale as `price`.
    pub conf: u64,
    /// `conf / price` in basis points. Precomputed because both the spread and the
    /// hard-reject gate need it.
    pub conf_bps: u64,
    /// Oracle publish time (seconds). Not local time — see [`validate_publish_time`].
    pub publish_time: i64,
}

impl ValidatedPrice {
    /// Assemble from already-normalised parts, enforcing the confidence ceiling.
    ///
    /// `max_conf_bps` is the per-market ceiling from `Market`. Correction C-4: at 50x
    /// leverage a 2 pip confidence band is ~4% of a trader's margin, so a price whose
    /// uncertainty exceeds the ceiling is not a price we can quote against.
    pub fn new(price: i64, conf: u64, publish_time: i64, max_conf_bps: u16) -> MathResult<Self> {
        let p = positive_to_u128(price)?;
        // Ceil: a confidence that rounds up widens the spread and makes the reject more
        // likely. Both are protocol-favourable.
        let conf_bps = to_u64(mul_div_ceil(u128::from(conf), BPS_PRECISION, p)?)?;
        if conf_bps > u64::from(max_conf_bps) {
            return Err(MathError::ConfidenceTooWide);
        }
        Ok(Self {
            price,
            conf,
            conf_bps,
            publish_time,
        })
    }
}

/// `10^n` as `u128`, or [`MathError::Overflow`] past 10^38.
///
/// Exists so exponent normalisation cannot silently wrap on a hostile or corrupt exponent.
pub fn pow10(n: u32) -> MathResult<u128> {
    let mut acc: u128 = 1;
    for _ in 0..n {
        acc = acc.checked_mul(10).ok_or(MathError::Overflow)?;
    }
    Ok(acc)
}

/// How far a Pyth exponent must shift to reach [`PRICE_PRECISION`].
///
/// Pyth publishes `real_value = mantissa × 10^exponent`. We store
/// `real_value × PRICE_PRECISION`, so the mantissa shifts by `9 + exponent` decimal places.
/// FX and metals feeds publish at `-8`, giving a shift of `+1`.
fn price_shift(exponent: i32) -> MathResult<i32> {
    // PRICE_PRECISION is 1e9 -> 9 decimal places.
    9_i32.checked_add(exponent).ok_or(MathError::Overflow)
}

/// Rescale a Pyth mantissa/exponent pair into [`PRICE_PRECISION`], rounding **down**.
///
/// Rejects non-positive input, and rejects any value that would normalise to zero — a price
/// of zero is not a cheap asset, it is a broken feed.
///
/// # Rounding
///
/// Truncation here is bounded by one unit of [`PRICE_PRECISION`], i.e. 1e-9 of the quote
/// currency, or one hundred-thousandth of a pip. It cannot be farmed: the adverse-side
/// spread applied in [`crate::pricing::execution_price`] is orders of magnitude larger and
/// is what actually protects the vault. The direction is fixed rather than
/// caller-selectable so two call sites cannot disagree about the mid price.
pub fn normalize_price(price: i64, exponent: i32) -> MathResult<i64> {
    let mantissa = positive_to_u128(price)?;
    let shift = price_shift(exponent)?;
    let scaled = if shift >= 0 {
        let factor = pow10(u32::try_from(shift).map_err(|_| MathError::Overflow)?)?;
        mantissa.checked_mul(factor).ok_or(MathError::Overflow)?
    } else {
        let divisor = pow10(shift.unsigned_abs())?;
        div_floor(mantissa, divisor)?
    };
    if scaled == 0 {
        return Err(MathError::InvalidPrice);
    }
    to_i64(i128::try_from(scaled).map_err(|_| MathError::Overflow)?)
}

/// Rescale a Pyth confidence into [`PRICE_PRECISION`], rounding **up**.
///
/// Opposite direction to [`normalize_price`] on purpose: a wider confidence widens the
/// spread and brings the reject gate closer, so rounding up is protocol-favourable.
/// A zero confidence normalises to zero and is allowed through — the caller decides whether
/// to trust it (`oracle-feasibility.md` flags XAUM/USD as a feed reporting exactly 0.00 bps,
/// which is a redemption rate rather than a traded market and must not back a liquidation).
pub fn normalize_conf(conf: u64, exponent: i32) -> MathResult<u64> {
    if conf == 0 {
        return Ok(0);
    }
    let shift = price_shift(exponent)?;
    let scaled = if shift >= 0 {
        let factor = pow10(u32::try_from(shift).map_err(|_| MathError::Overflow)?)?;
        u128::from(conf)
            .checked_mul(factor)
            .ok_or(MathError::Overflow)?
    } else {
        let divisor = pow10(shift.unsigned_abs())?;
        div_ceil(u128::from(conf), divisor)?
    };
    to_u64(scaled)
}

/// Reject a price that is too old **or dated in the future**.
///
/// # Why the future check exists
///
/// `PriceUpdateV2::get_price_no_older_than` checks only one side —
/// `publish_time + max_age >= now`. A price stamped an hour ahead of the cluster clock
/// satisfies that trivially and would then satisfy it for the next hour regardless of what
/// the market did. That is the stale-price exploit (C-1, T3) reached through a different
/// door, so the freshness window is closed at both ends here.
///
/// `max_future_drift_seconds` absorbs ordinary disagreement between the oracle publisher's
/// clock and the cluster's; it should be small (a few seconds), never zero.
pub fn validate_publish_time(
    publish_time: i64,
    now: i64,
    max_staleness_seconds: u32,
    max_future_drift_seconds: u32,
) -> MathResult<()> {
    let age = i128::from(now)
        .checked_sub(i128::from(publish_time))
        .ok_or(MathError::Overflow)?;
    if age > i128::from(max_staleness_seconds) {
        return Err(MathError::PriceTooStale);
    }
    let drift = age.checked_neg().ok_or(MathError::Overflow)?;
    if drift > i128::from(max_future_drift_seconds) {
        return Err(MathError::PriceFromFuture);
    }
    Ok(())
}

/// `|price − reference| / reference` in basis points, rounded **up**.
///
/// `reference` must be strictly positive; a zero reference means "no reference yet" and the
/// caller must skip the check rather than pass it here (§ 7.1 guards with `ema_price > 0`).
pub fn deviation_bps(price: i64, reference: i64) -> MathResult<u64> {
    let p = positive_to_u128(price)?;
    let r = positive_to_u128(reference)?;
    let diff = p.abs_diff(r);
    to_u64(mul_div_ceil(diff, BPS_PRECISION, r)?)
}

/// Gate on [`deviation_bps`]. Split from the measurement so a caller can *observe* a
/// deviation without being forced to reject on it.
///
/// That split matters: § 7.2 sends a deviating market to `Halted` with a guardian alert,
/// which is a different response from rejecting one trade. A cranker that had to pass this
/// gate in order to refresh the stored reference could never recover from a large genuine
/// move — the reference would stay stale, every subsequent price would deviate from it, and
/// the market would be wedged shut. Measure first, then decide.
pub fn validate_deviation(deviation_bps: u64, max_deviation_bps: u16) -> MathResult<()> {
    if deviation_bps > u64::from(max_deviation_bps) {
        return Err(MathError::DeviationTooLarge);
    }
    Ok(())
}

/// Compose a cross rate from two legs (`ARCHITECTURE.md` § 3.5).
///
/// - `invert_quote == false`: `price = base × quote`, e.g. GBP/JPY = (GBP/USD) × (USD/JPY)
/// - `invert_quote == true`:  `price = base ÷ quote`, e.g. EUR/GBP = (EUR/USD) ÷ (GBP/USD)
///
/// # Confidence compounds linearly, not in quadrature
///
/// § 3.5 gives `√(conf₁² + conf₂²)`. **This implements `conf₁ + conf₂` instead**, and the
/// difference is deliberate.
///
/// Quadrature is the variance-addition result for **independent** errors. The two legs of
/// every cross we would ever compose share a currency — that is what makes them composable —
/// so their errors are correlated and independence does not hold. The linear sum is the
/// triangle-inequality upper bound and is valid under *any* correlation, including the
/// adversarial case. Quadrature understates the band by up to 29% at equal leg confidences,
/// and the band is what stands between the vault and latency arbitrage (C-4).
///
/// The cost of being conservative here is close to zero: `oracle-feasibility.md` § 4 measured
/// Pyth's ~180 native crosses as *tighter* than the majors they would be composed from
/// (EUR/JPY 0.59 bps vs EUR/USD 1.28 bps), so v1 lists direct feeds only and this path is
/// reserved for pairs Pyth does not carry.
///
/// `max_conf_bps` is applied to the **composed** price, so a cross inherits one ceiling
/// rather than two.
pub fn compose_synthetic(
    base: ValidatedPrice,
    quote: ValidatedPrice,
    invert_quote: bool,
    max_conf_bps: u16,
) -> MathResult<ValidatedPrice> {
    let b = positive_to_u128(base.price)?;
    let q = positive_to_u128(quote.price)?;

    // Floor, matching normalize_price: the composed value is a mid, and the adverse spread
    // is applied downstream.
    let composed = if invert_quote {
        mul_div_floor(b, PRICE_PRECISION, q)?
    } else {
        mul_div_floor(b, q, PRICE_PRECISION)?
    };
    if composed == 0 {
        return Err(MathError::InvalidPrice);
    }
    let composed_price = to_i64(i128::try_from(composed).map_err(|_| MathError::Overflow)?)?;

    // Relative confidences at RATE_PRECISION rather than bps: a leg confidence below
    // 1 bp is common (EUR/JPY sits at 0.59 bps) and would truncate to zero in bps,
    // silently erasing the uncertainty this whole function exists to propagate.
    let rel_base = mul_div_ceil(u128::from(base.conf), RATE_PRECISION, b)?;
    let rel_quote = mul_div_ceil(u128::from(quote.conf), RATE_PRECISION, q)?;
    let rel_total = rel_base.checked_add(rel_quote).ok_or(MathError::Overflow)?;
    let conf = to_u64(mul_div_ceil(composed, rel_total, RATE_PRECISION)?)?;

    // The composed price is only as fresh as its staler leg.
    let publish_time = base.publish_time.min(quote.publish_time);

    ValidatedPrice::new(composed_price, conf, publish_time, max_conf_bps)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_CONF_LIMIT: u16 = u16::MAX;

    // --- exponent normalisation ---------------------------------------------------------

    /// The canonical case: every FX and metals feed in the measured set publishes at -8.
    #[test]
    fn normalizes_a_real_eur_usd_quote() {
        // Pyth publishes EUR/USD 1.08543 as mantissa 108_543_000 with exponent -8.
        assert_eq!(normalize_price(108_543_000, -8).unwrap(), 1_085_430_000);
    }

    #[test]
    fn normalizes_usd_jpy_without_overflowing_i64() {
        // USD/JPY 157.20 -> 15_720_000_000 at expo -8 -> 157_200_000_000 at 1e9.
        assert_eq!(
            normalize_price(15_720_000_000, -8).unwrap(),
            157_200_000_000
        );
    }

    #[test]
    fn normalizes_gold_near_four_thousand() {
        // XAU/USD 4046.95, the Friday close recorded in oracle-feasibility.md.
        assert_eq!(
            normalize_price(404_695_000_000, -8).unwrap(),
            4_046_950_000_000
        );
    }

    #[test]
    fn exponent_of_negative_nine_is_the_identity() {
        assert_eq!(normalize_price(1_085_430_000, -9).unwrap(), 1_085_430_000);
    }

    #[test]
    fn exponent_below_negative_nine_divides() {
        // expo -10 means the mantissa carries one more digit than we store.
        assert_eq!(normalize_price(10_854_300_005, -10).unwrap(), 1_085_430_000);
    }

    #[test]
    fn positive_exponent_multiplies() {
        assert_eq!(normalize_price(2, 1).unwrap(), 20 * 1_000_000_000);
    }

    #[test]
    fn non_positive_price_is_rejected() {
        assert_eq!(normalize_price(0, -8), Err(MathError::InvalidPrice));
        assert_eq!(normalize_price(-1, -8), Err(MathError::InvalidPrice));
    }

    /// A price so small it normalises away is a broken feed, not a cheap asset.
    #[test]
    fn price_that_underflows_to_zero_is_rejected() {
        assert_eq!(normalize_price(5, -30), Err(MathError::InvalidPrice));
    }

    #[test]
    fn absurd_exponents_error_rather_than_wrap() {
        assert_eq!(normalize_price(1, i32::MAX), Err(MathError::Overflow));
        assert_eq!(normalize_price(1, 40), Err(MathError::Overflow));
    }

    /// Confidence rounds up where price rounds down. Same input, opposite direction.
    #[test]
    fn conf_rounds_up_while_price_rounds_down() {
        assert_eq!(normalize_price(10_854_300_005, -10).unwrap(), 1_085_430_000);
        assert_eq!(normalize_conf(10_854_300_005, -10).unwrap(), 1_085_430_001);
    }

    #[test]
    fn zero_confidence_survives_normalisation() {
        // XAUM/USD reports exactly 0.00 bps. It must not error here — the decision to
        // distrust it belongs to the market's max_conf_bps, not to the rescaler.
        assert_eq!(normalize_conf(0, -8).unwrap(), 0);
    }

    #[test]
    fn conf_normalises_at_the_same_scale_as_price() {
        // 1.43 bps on gold at 4046.95 -> conf ~0.5787.
        let conf = normalize_conf(57_871_400, -8).unwrap();
        assert_eq!(conf, 578_714_000);
    }

    // --- ValidatedPrice ----------------------------------------------------------------

    #[test]
    fn conf_bps_matches_the_measured_gold_figure() {
        // XAU/USD at 4046.95 with a 1.43 bps band.
        let price = 4_046_950_000_000_i64;
        let conf = 578_714_000_u64; // 0.5787 -> 1.43 bps of 4046.95
        let v = ValidatedPrice::new(price, conf, 0, NO_CONF_LIMIT).unwrap();
        assert_eq!(v.conf_bps, 2); // 1.43 rounded up
    }

    #[test]
    fn confidence_ceiling_rejects_a_wide_band() {
        // USD/IDR measured 30.11 bps p95 and was excluded on exactly this test.
        let price = 1_000_000_000_i64;
        let conf = 3_011_000_u64; // 30.11 bps
        assert_eq!(
            ValidatedPrice::new(price, conf, 0, 25),
            Err(MathError::ConfidenceTooWide)
        );
        assert!(ValidatedPrice::new(price, conf, 0, 31).is_ok());
    }

    #[test]
    fn conf_bps_rounds_up_so_a_sub_bp_band_is_never_free() {
        let v = ValidatedPrice::new(1_000_000_000, 1, 0, NO_CONF_LIMIT).unwrap();
        assert_eq!(v.conf_bps, 1, "a non-zero band must not round to zero bps");
    }

    #[test]
    fn non_positive_price_cannot_be_validated() {
        assert_eq!(
            ValidatedPrice::new(0, 0, 0, NO_CONF_LIMIT),
            Err(MathError::InvalidPrice)
        );
    }

    // --- freshness ----------------------------------------------------------------------

    #[test]
    fn fresh_price_passes() {
        assert!(validate_publish_time(1_000, 1_005, 10, 5).is_ok());
    }

    #[test]
    fn stale_price_is_rejected_at_the_boundary() {
        assert!(validate_publish_time(1_000, 1_010, 10, 5).is_ok());
        assert_eq!(
            validate_publish_time(1_000, 1_011, 10, 5),
            Err(MathError::PriceTooStale)
        );
    }

    /// The gap `get_price_no_older_than` leaves open. A Friday close read on Saturday is
    /// 21.4 hours stale — the exact measurement in oracle-feasibility.md.
    #[test]
    fn a_friday_close_read_on_saturday_is_rejected() {
        let friday_close = 1_000_000;
        let saturday = friday_close + 77_040; // 21.4 h
        assert_eq!(
            validate_publish_time(friday_close, saturday, 10, 5),
            Err(MathError::PriceTooStale)
        );
    }

    #[test]
    fn future_dated_price_is_rejected() {
        assert!(validate_publish_time(1_005, 1_000, 10, 5).is_ok());
        assert_eq!(
            validate_publish_time(1_006, 1_000, 10, 5),
            Err(MathError::PriceFromFuture)
        );
    }

    #[test]
    fn freshness_does_not_overflow_at_the_extremes() {
        assert_eq!(
            validate_publish_time(i64::MIN, i64::MAX, u32::MAX, u32::MAX),
            Err(MathError::PriceTooStale)
        );
        assert_eq!(
            validate_publish_time(i64::MAX, i64::MIN, u32::MAX, u32::MAX),
            Err(MathError::PriceFromFuture)
        );
    }

    // --- deviation ----------------------------------------------------------------------

    #[test]
    fn deviation_is_symmetric_in_direction() {
        let up = deviation_bps(1_010_000_000, 1_000_000_000).unwrap();
        let down = deviation_bps(990_000_000, 1_000_000_000).unwrap();
        assert_eq!(up, 100);
        assert_eq!(down, 100);
    }

    #[test]
    fn deviation_gate_trips_above_the_ceiling() {
        assert!(validate_deviation(300, 300).is_ok());
        assert_eq!(
            validate_deviation(301, 300),
            Err(MathError::DeviationTooLarge)
        );
    }

    /// EUR/CHF fell ~30% on 15 Jan 2015. The breaker must fire well before that.
    #[test]
    fn chf_depeg_scale_move_trips_the_breaker() {
        let dev = deviation_bps(700_000_000, 1_000_000_000).unwrap();
        assert_eq!(dev, 3_000);
        assert_eq!(
            validate_deviation(dev, 300),
            Err(MathError::DeviationTooLarge)
        );
    }

    #[test]
    fn deviation_rejects_a_non_positive_reference() {
        assert_eq!(deviation_bps(1, 0), Err(MathError::InvalidPrice));
    }

    // --- synthetic composition ----------------------------------------------------------

    #[test]
    fn composes_eur_gbp_by_division() {
        // EUR/USD 1.0850 ÷ GBP/USD 1.2700 = 0.854330...
        let eur_usd = ValidatedPrice::new(1_085_000_000, 0, 0, NO_CONF_LIMIT).unwrap();
        let gbp_usd = ValidatedPrice::new(1_270_000_000, 0, 0, NO_CONF_LIMIT).unwrap();
        let cross = compose_synthetic(eur_usd, gbp_usd, true, NO_CONF_LIMIT).unwrap();
        assert_eq!(cross.price, 854_330_708);
    }

    #[test]
    fn composes_gbp_jpy_by_multiplication() {
        // GBP/USD 1.2700 × USD/JPY 157.20 = 199.644
        let gbp_usd = ValidatedPrice::new(1_270_000_000, 0, 0, NO_CONF_LIMIT).unwrap();
        let usd_jpy = ValidatedPrice::new(157_200_000_000, 0, 0, NO_CONF_LIMIT).unwrap();
        let cross = compose_synthetic(gbp_usd, usd_jpy, false, NO_CONF_LIMIT).unwrap();
        assert_eq!(cross.price, 199_644_000_000);
    }

    /// The documented deviation from § 3.5: linear, not quadrature.
    #[test]
    fn composed_confidence_is_the_linear_sum_not_the_quadrature_sum() {
        // Both legs at 100 bps. Quadrature would give √2 × 100 ≈ 141 bps; we require 200.
        let a = ValidatedPrice::new(1_000_000_000, 10_000_000, 0, NO_CONF_LIMIT).unwrap();
        let b = ValidatedPrice::new(1_000_000_000, 10_000_000, 0, NO_CONF_LIMIT).unwrap();
        let cross = compose_synthetic(a, b, false, NO_CONF_LIMIT).unwrap();
        assert_eq!(cross.conf_bps, 200);
        assert!(cross.conf_bps > 142, "must exceed the quadrature result");
    }

    /// A sub-basis-point leg confidence must survive composition. Measured crosses sit at
    /// 0.59 bps, which is zero in integer bps — the reason relative confidence is carried
    /// at RATE_PRECISION rather than in bps.
    #[test]
    fn sub_basis_point_leg_confidence_is_not_erased() {
        // 0.59 bps of 1.0 = 59_000 units at 1e9.
        let a = ValidatedPrice::new(1_000_000_000, 59_000, 0, NO_CONF_LIMIT).unwrap();
        let b = ValidatedPrice::new(1_000_000_000, 59_000, 0, NO_CONF_LIMIT).unwrap();
        let cross = compose_synthetic(a, b, false, NO_CONF_LIMIT).unwrap();
        assert!(cross.conf > 0, "composed confidence collapsed to zero");
        assert_eq!(cross.conf, 118_000);
    }

    #[test]
    fn composed_price_inherits_the_staler_leg_timestamp() {
        let fresh = ValidatedPrice::new(1_000_000_000, 0, 5_000, NO_CONF_LIMIT).unwrap();
        let stale = ValidatedPrice::new(1_000_000_000, 0, 4_000, NO_CONF_LIMIT).unwrap();
        let cross = compose_synthetic(fresh, stale, false, NO_CONF_LIMIT).unwrap();
        assert_eq!(cross.publish_time, 4_000);
    }

    #[test]
    fn composed_price_is_subject_to_the_confidence_ceiling() {
        let a = ValidatedPrice::new(1_000_000_000, 2_000_000, 0, NO_CONF_LIMIT).unwrap();
        let b = ValidatedPrice::new(1_000_000_000, 2_000_000, 0, NO_CONF_LIMIT).unwrap();
        // 20 bps each, 40 bps composed.
        assert_eq!(
            compose_synthetic(a, b, true, 30),
            Err(MathError::ConfidenceTooWide)
        );
        assert!(compose_synthetic(a, b, true, 40).is_ok());
    }

    #[test]
    fn composition_that_underflows_to_zero_is_rejected() {
        let tiny = ValidatedPrice::new(1, 0, 0, NO_CONF_LIMIT).unwrap();
        let huge = ValidatedPrice::new(i64::MAX, 0, 0, NO_CONF_LIMIT).unwrap();
        assert_eq!(
            compose_synthetic(tiny, huge, true, NO_CONF_LIMIT),
            Err(MathError::InvalidPrice)
        );
    }

    // --- pow10 --------------------------------------------------------------------------

    #[test]
    fn pow10_is_exact_and_bounded() {
        assert_eq!(pow10(0).unwrap(), 1);
        assert_eq!(pow10(9).unwrap(), 1_000_000_000);
        assert_eq!(pow10(38).unwrap(), 10_u128.pow(38));
        assert_eq!(pow10(39), Err(MathError::Overflow));
    }
}
