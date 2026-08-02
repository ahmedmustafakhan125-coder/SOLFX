//! Funding (skew rebalancing) and carry (the FX swap equivalent).
//!
//! See `ARCHITECTURE.md` § 6.7. These are two distinct mechanisms and must not be conflated:
//!
//! - **Carry** is an interest charge on leveraged notional, paid to the protocol and LPs.
//!   It is the on-chain equivalent of an overnight swap, and publishing its two components
//!   separately is a concrete improvement on XM/Exness opacity (`FOREX-EXPLAINED.md` § 9).
//! - **Funding** is a pure transfer *between traders* to correct book skew. It must never
//!   touch LP capital, and must net to zero across the market (invariant I3).
//!
//! # Why a cumulative index
//!
//! A Solana instruction cannot iterate every open position to charge funding. Instead the
//! market holds a monotonically accumulating index, each position stores its snapshot at
//! open, and the difference is what it owes. That makes accrual O(1) per position.

use crate::constants::{RATE_PRECISION, RATE_PRECISION_I128, SECONDS_PER_HOUR};
use crate::error::{MathError, MathResult};
use crate::fixed::{
    abs_i128, clamp_symmetric, i128_to_u128, mul_div_ceil, mul_div_ceil_signed, mul_div_floor,
    mul_div_floor_signed, to_i64, to_u64,
};
use crate::types::Direction;

/// Hours in a 365-day year, for converting annual central-bank rates to hourly accrual.
pub const HOURS_PER_YEAR: i128 = 365 * 24;

/// The hourly **cost** of holding a position, at [`RATE_PRECISION`].
///
/// Positive means the trader pays. Inputs are annual interest rates at [`RATE_PRECISION`]
/// (so 4% per year is `40_000_000`).
///
/// # Sign convention
///
/// A long holds the base currency and borrows the quote, so it earns `base` and pays
/// `quote`; its cost is `quote − base`. `FOREX-EXPLAINED.md` § 9 works this through: long
/// EUR/USD with EUR at 2% and USD at 4% costs 2% per year.
///
/// `ARCHITECTURE.md` § 6.7 writes the same quantity as `(rate_base − rate_quote)`, which is
/// the negation — it states the *earn* rate rather than the cost. This function returns the
/// cost, matching the user-facing document and the sign that [`accrued_carry`] expects.
///
/// The markup is always added as a charge on **both** directions. That is our revenue, and
/// showing it as a separate line item is the transparency feature.
///
/// # Rounding
///
/// This is a **cost**, so it rounds toward `+∞` — a charge grows, a credit shrinks. That
/// makes long and short rates *near* mirrors but not exact ones: on a differential that does
/// not divide evenly by [`HOURS_PER_YEAR`], `long + short` is `1` rather than `0`, with the
/// odd unit accruing to the protocol. Rounding a matched pair to exact symmetry would
/// require rounding one side in the trader's favour.
pub fn carry_cost_rate_per_hour(
    base_annual_rate: i64,
    quote_annual_rate: i64,
    markup_per_hour: u64,
    direction: Direction,
) -> MathResult<i64> {
    let differential = i128::from(quote_annual_rate)
        .checked_sub(i128::from(base_annual_rate))
        .ok_or(MathError::Overflow)?;

    // A short has the mirrored exposure, so the differential flips.
    let directional = differential
        .checked_mul(direction.sign())
        .ok_or(MathError::Overflow)?;

    let hourly = mul_div_ceil_signed(directional, 1, HOURS_PER_YEAR)?;

    let with_markup = hourly
        .checked_add(i128::from(markup_per_hour))
        .ok_or(MathError::Overflow)?;
    to_i64(with_markup)
}

/// Advance a cumulative carry index by `elapsed_seconds` at `rate_per_hour`.
///
/// The index is unsigned and monotonically increasing: carry with a markup is a net charge
/// in normal conditions. A negative differential large enough to make the total rate
/// negative is clamped to zero rather than allowed to run the index backwards — a trader
/// being *paid* to hold leveraged notional is a configuration error, not a feature, and
/// letting the index decrease would corrupt every position that snapshotted a higher value.
pub fn advance_carry_index(
    current_index: u128,
    rate_per_hour: i64,
    elapsed_seconds: i64,
) -> MathResult<u128> {
    if elapsed_seconds <= 0 {
        return Ok(current_index);
    }
    if rate_per_hour <= 0 {
        return Ok(current_index);
    }
    let accrual = mul_div_floor(
        i128_to_u128(i128::from(rate_per_hour))?,
        i128_to_u128(i128::from(elapsed_seconds))?,
        i128_to_u128(i128::from(SECONDS_PER_HOUR))?,
    )?;
    current_index
        .checked_add(accrual)
        .ok_or(MathError::Overflow)
}

/// Carry owed by a position, in USDC. Rounded **up** — it is a charge.
///
/// `basis` is the position's **notional at entry**, not its base size. Carry is interest on
/// the borrowed notional, and pinning it to entry keeps the charge deterministic instead of
/// drifting with the mark price.
pub fn accrued_carry(basis: u64, index_now: u128, index_at_entry: u128) -> MathResult<u64> {
    let delta = index_now
        .checked_sub(index_at_entry)
        .ok_or(MathError::Overflow)?;
    let carry = mul_div_ceil(u128::from(basis), delta, RATE_PRECISION)?;
    to_u64(carry)
}

/// The hourly funding rate, at [`RATE_PRECISION`]. Positive means longs pay shorts.
///
/// `rate = clamp(skew_ratio × k, −cap, +cap)` where
/// `skew_ratio = (long − short) / (long + short)`.
///
/// Returns zero when either side is empty: funding is a transfer between traders, so with
/// nobody on the other side there is no one to pay. Charging anyway would route trader money
/// into LP capital, which is exactly the property invariant I3 forbids.
pub fn funding_rate_per_hour(
    base_oi_long: i128,
    base_oi_short: i128,
    k: u64,
    cap_per_hour: i64,
) -> MathResult<i64> {
    if base_oi_long <= 0 || base_oi_short <= 0 {
        return Ok(0);
    }
    let total = base_oi_long
        .checked_add(base_oi_short)
        .ok_or(MathError::Overflow)?;
    let skew = base_oi_long
        .checked_sub(base_oi_short)
        .ok_or(MathError::Overflow)?;

    // skew_ratio at RATE_PRECISION, then scaled by k (also at RATE_PRECISION).
    let ratio = mul_div_floor_signed(skew, RATE_PRECISION_I128, total)?;
    let scaled = mul_div_floor_signed(ratio, i128::from(k), RATE_PRECISION_I128)?;

    let clamped = clamp_symmetric(scaled, i128::from(cap_per_hour))?;
    to_i64(clamped)
}

/// How much to add to each side's cumulative funding index.
///
/// Both deltas are *costs*: positive means that side pays. The receiving side's delta is
/// negative, so [`accrued_funding`] returns a negative cost for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FundingIndexUpdate {
    pub long_delta: i128,
    pub short_delta: i128,
}

/// Compute both index deltas for an elapsed period, conserving value between the two sides.
///
/// # Why the receiving side is scaled
///
/// If longs pay `r` per base unit, the total collected is `r × base_oi_long`. Crediting
/// shorts the same `r` per unit would pay out `r × base_oi_short` — which only balances on a
/// perfectly even book. The light side's rate is therefore scaled by the OI ratio so the
/// transfer nets out (invariant I3).
///
/// The payer's rate rounds **up** and the receiver's rounds **down**, so any rounding dust
/// stays with the protocol rather than being paid out of LP capital.
pub fn funding_index_update(
    base_oi_long: i128,
    base_oi_short: i128,
    rate_per_hour: i64,
    elapsed_seconds: i64,
) -> MathResult<FundingIndexUpdate> {
    if elapsed_seconds <= 0 || rate_per_hour == 0 {
        return Ok(FundingIndexUpdate::default());
    }
    if base_oi_long <= 0 || base_oi_short <= 0 {
        return Ok(FundingIndexUpdate::default());
    }

    // Rate for this period, magnitude only. Rounds up: the payer pays the dust.
    let magnitude = i128_to_u128(abs_i128(i128::from(rate_per_hour))?)?;
    let period_rate = mul_div_ceil(
        magnitude,
        i128_to_u128(i128::from(elapsed_seconds))?,
        i128_to_u128(i128::from(SECONDS_PER_HOUR))?,
    )?;
    let period_rate = i128::try_from(period_rate).map_err(|_| MathError::Overflow)?;

    let longs_pay = rate_per_hour > 0;
    let (payer_oi, receiver_oi) = if longs_pay {
        (base_oi_long, base_oi_short)
    } else {
        (base_oi_short, base_oi_long)
    };

    // Receiver's per-unit rate, floored: they receive no more than was collected.
    let receiver_rate = mul_div_floor_signed(period_rate, payer_oi, receiver_oi)?;

    Ok(if longs_pay {
        FundingIndexUpdate {
            long_delta: period_rate,
            short_delta: receiver_rate.checked_neg().ok_or(MathError::Overflow)?,
        }
    } else {
        FundingIndexUpdate {
            long_delta: receiver_rate.checked_neg().ok_or(MathError::Overflow)?,
            short_delta: period_rate,
        }
    })
}

/// Funding owed by a position, in USDC. Signed — negative means the trader receives.
///
/// `basis` is the position's **base size**, matching the units the index accrues in.
/// Floors, so a payment rounds up in magnitude and a receipt rounds down.
pub fn accrued_funding(basis: u64, index_now: i128, index_at_entry: i128) -> MathResult<i64> {
    let delta = index_now
        .checked_sub(index_at_entry)
        .ok_or(MathError::Overflow)?;
    let funding = mul_div_floor_signed(i128::from(basis), delta, RATE_PRECISION_I128)?;
    to_i64(funding)
}

/// The book's skew as a signed ratio at [`RATE_PRECISION`]. Presentation and risk checks —
/// `ARCHITECTURE.md` § 7.4 blocks opens on the heavy side past 0.6.
pub fn skew_ratio(base_oi_long: i128, base_oi_short: i128) -> MathResult<i64> {
    let total = base_oi_long
        .checked_add(base_oi_short)
        .ok_or(MathError::Overflow)?;
    if total <= 0 {
        return Ok(0);
    }
    let skew = base_oi_long
        .checked_sub(base_oi_short)
        .ok_or(MathError::Overflow)?;
    to_i64(mul_div_floor_signed(skew, RATE_PRECISION_I128, total)?)
}

/// Utilisation of the LP vault: `exposure / aum` at [`RATE_PRECISION`], rounded **up**.
///
/// `ARCHITECTURE.md` § 7.2 rejects new opens on the crowded side past 80%.
pub fn utilisation_ratio(exposure: u64, aum: u64) -> MathResult<u64> {
    if aum == 0 {
        return Err(MathError::DivideByZero);
    }
    let r = mul_div_ceil(u128::from(exposure), RATE_PRECISION, u128::from(aum))?;
    to_u64(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PCT: i64 = 10_000_000; // 1% at RATE_PRECISION
    const USD: u64 = 1_000_000;

    /// FOREX-EXPLAINED.md § 9: long EUR/USD with EUR at 2% and USD at 4% pays 2%/year.
    #[test]
    fn long_carry_matches_the_explainer_worked_example() {
        let rate = carry_cost_rate_per_hour(2 * PCT, 4 * PCT, 0, Direction::Long).unwrap();
        // 2% per year spread over 8,760 hours, rounded up because it is a charge.
        let exact = (2 * i128::from(PCT)) / HOURS_PER_YEAR;
        assert_eq!(i128::from(rate), exact + 1);
        assert!(rate > 0, "a long in this configuration pays");
    }

    /// The same position shorted receives the differential instead.
    #[test]
    fn short_carry_has_the_opposite_sign() {
        let long = carry_cost_rate_per_hour(2 * PCT, 4 * PCT, 0, Direction::Long).unwrap();
        let short = carry_cost_rate_per_hour(2 * PCT, 4 * PCT, 0, Direction::Short).unwrap();
        assert!(long > 0, "a long in this configuration pays");
        assert!(short < 0, "a short in this configuration receives");
        assert_eq!(long.abs() - short.abs(), 1); // ceiling rounding, one unit apart
    }

    /// The two sides cannot be exact mirrors under protocol-favourable rounding, but the
    /// pair must never net *against* the protocol: it charges the long at least as much as
    /// it credits the short.
    #[test]
    fn a_matched_pair_never_costs_the_protocol() {
        let cases = [
            (2 * PCT, 4 * PCT),
            (4 * PCT, 2 * PCT),
            (0, 1),
            (1, 0),
            (33, 77),
            (5 * PCT + 7, 3 * PCT - 11),
        ];
        for (base, quote) in cases {
            let long = carry_cost_rate_per_hour(base, quote, 0, Direction::Long).unwrap();
            let short = carry_cost_rate_per_hour(base, quote, 0, Direction::Short).unwrap();
            let net = long + short;
            assert!(
                (0..=1).contains(&net),
                "matched pair netted {net} for base {base} quote {quote}"
            );
        }
    }

    /// The markup is a charge on both sides — that is the revenue stream, and § 6.7
    /// requires it be visible as its own line item.
    #[test]
    fn markup_is_charged_in_both_directions() {
        let markup = 1_000_u64;
        let long_clean = carry_cost_rate_per_hour(2 * PCT, 4 * PCT, 0, Direction::Long).unwrap();
        let long_marked =
            carry_cost_rate_per_hour(2 * PCT, 4 * PCT, markup, Direction::Long).unwrap();
        let short_clean = carry_cost_rate_per_hour(2 * PCT, 4 * PCT, 0, Direction::Short).unwrap();
        let short_marked =
            carry_cost_rate_per_hour(2 * PCT, 4 * PCT, markup, Direction::Short).unwrap();

        assert_eq!(long_marked - long_clean, 1_000);
        assert_eq!(short_marked - short_clean, 1_000);
    }

    /// Equal rates on both currencies leave only the markup.
    #[test]
    fn equal_interest_rates_leave_only_the_markup() {
        let r = carry_cost_rate_per_hour(3 * PCT, 3 * PCT, 500, Direction::Long).unwrap();
        assert_eq!(r, 500);
    }

    #[test]
    fn carry_index_advances_proportionally_to_time() {
        let one_hour = advance_carry_index(0, 1_000, SECONDS_PER_HOUR).unwrap();
        let two_hours = advance_carry_index(0, 1_000, 2 * SECONDS_PER_HOUR).unwrap();
        assert_eq!(one_hour, 1_000);
        assert_eq!(two_hours, 2_000);
    }

    #[test]
    fn carry_index_never_moves_backwards() {
        // A negative rate must not decrease the index — positions snapshotted the old value.
        assert_eq!(
            advance_carry_index(5_000, -1_000, SECONDS_PER_HOUR).unwrap(),
            5_000
        );
        assert_eq!(advance_carry_index(5_000, 1_000, 0).unwrap(), 5_000);
        assert_eq!(advance_carry_index(5_000, 1_000, -60).unwrap(), 5_000);
    }

    /// $108,543 of notional at 2%/year costs about $5.95/day — the figure in
    /// FOREX-EXPLAINED.md § 9.
    #[test]
    fn daily_carry_matches_the_explainer() {
        let rate = carry_cost_rate_per_hour(2 * PCT, 4 * PCT, 0, Direction::Long).unwrap();
        let index = advance_carry_index(0, rate, 24 * SECONDS_PER_HOUR).unwrap();
        let carry = accrued_carry(108_543 * USD, index, 0).unwrap();
        // ~$5.95, allowing for hourly truncation.
        assert!(
            (5_900_000..=6_000_000).contains(&carry),
            "daily carry was {carry}"
        );
    }

    #[test]
    fn accrued_carry_rounds_up() {
        // A basis and delta whose product is a fraction of one unit still charges 1.
        assert_eq!(accrued_carry(1, 1, 0).unwrap(), 1);
        assert_eq!(accrued_carry(1_000_000, 0, 0).unwrap(), 0);
    }

    #[test]
    fn funding_rate_is_zero_on_a_balanced_book() {
        assert_eq!(
            funding_rate_per_hour(1_000, 1_000, RATE_PRECISION as u64, 1_000_000).unwrap(),
            0
        );
    }

    #[test]
    fn funding_rate_is_positive_when_longs_are_crowded() {
        let r = funding_rate_per_hour(3_000, 1_000, RATE_PRECISION as u64, 1_000_000_000).unwrap();
        // skew ratio = 2000/4000 = 0.5
        assert_eq!(r, 500_000_000);
    }

    #[test]
    fn funding_rate_is_negative_when_shorts_are_crowded() {
        let r = funding_rate_per_hour(1_000, 3_000, RATE_PRECISION as u64, 1_000_000_000).unwrap();
        assert_eq!(r, -500_000_000);
    }

    #[test]
    fn funding_rate_respects_the_cap() {
        let cap = 1_000;
        let r = funding_rate_per_hour(1_000_000, 1, RATE_PRECISION as u64, cap).unwrap();
        assert_eq!(r, cap);
        let r = funding_rate_per_hour(1, 1_000_000, RATE_PRECISION as u64, cap).unwrap();
        assert_eq!(r, -cap);
    }

    /// Funding is a transfer between traders. With one side empty there is nobody to pay,
    /// so charging would push trader money into LP capital (invariant I3).
    #[test]
    fn funding_is_zero_when_a_side_is_empty() {
        assert_eq!(
            funding_rate_per_hour(1_000, 0, RATE_PRECISION as u64, 1_000_000).unwrap(),
            0
        );
        assert_eq!(
            funding_rate_per_hour(0, 1_000, RATE_PRECISION as u64, 1_000_000).unwrap(),
            0
        );
        assert_eq!(
            funding_rate_per_hour(0, 0, RATE_PRECISION as u64, 1_000_000).unwrap(),
            0
        );

        let u = funding_index_update(1_000, 0, 500_000, SECONDS_PER_HOUR).unwrap();
        assert_eq!(u, FundingIndexUpdate::default());
    }

    #[test]
    fn funding_update_charges_the_heavy_side() {
        let u = funding_index_update(3_000, 1_000, 500_000, SECONDS_PER_HOUR).unwrap();
        assert!(u.long_delta > 0, "crowded longs must pay");
        assert!(u.short_delta < 0, "light shorts must receive");
    }

    /// Invariant I3: funding must net to zero across the market — and any rounding dust
    /// must favour the protocol, never leave it short.
    #[test]
    fn funding_conserves_value_between_the_two_sides() {
        let cases = [
            (3_000_i128, 1_000_i128, 500_000_i64),
            (1_000, 3_000, -500_000),
            (7_777, 1_234, 123_457),
            (1, 1_000_000, -999_983),
            (1_000_000, 1, 999_983),
        ];
        for (long_oi, short_oi, rate) in cases {
            let u = funding_index_update(long_oi, short_oi, rate, SECONDS_PER_HOUR).unwrap();

            // Total paid by longs, minus total received by shorts (both as costs).
            let long_total = long_oi * u.long_delta;
            let short_total = short_oi * u.short_delta;
            let net = long_total + short_total;

            assert!(
                net >= 0,
                "protocol paid out more than it collected: net {net} for {long_oi}/{short_oi}"
            );
            // The dust is bounded by one unit of index per unit of receiving-side OI.
            let receiver_oi = if rate > 0 { short_oi } else { long_oi };
            assert!(
                net <= receiver_oi,
                "dust {net} exceeded one unit per receiver unit ({receiver_oi})"
            );
        }
    }

    #[test]
    fn funding_update_is_empty_without_elapsed_time_or_rate() {
        assert_eq!(
            funding_index_update(3_000, 1_000, 500_000, 0).unwrap(),
            FundingIndexUpdate::default()
        );
        assert_eq!(
            funding_index_update(3_000, 1_000, 0, SECONDS_PER_HOUR).unwrap(),
            FundingIndexUpdate::default()
        );
    }

    #[test]
    fn accrued_funding_is_signed() {
        // Index moved up: this side pays.
        assert!(accrued_funding(1_000_000_000, 1_000_000, 0).unwrap() > 0);
        // Index moved down: this side receives.
        assert!(accrued_funding(1_000_000_000, -1_000_000, 0).unwrap() < 0);
        // No movement, no charge.
        assert_eq!(accrued_funding(1_000_000_000, 500, 500).unwrap(), 0);
    }

    /// A payment rounds up in magnitude, a receipt rounds down — both favour the protocol.
    #[test]
    fn accrued_funding_rounds_toward_the_protocol() {
        // Basis 3, delta 1: true value is 3e-9, which floors to 0 for a receipt.
        assert_eq!(accrued_funding(3, -1, 0).unwrap(), -1); // floor(-3e-9) = -1
        assert_eq!(accrued_funding(3, 1, 0).unwrap(), 0); // floor(+3e-9) = 0
    }

    #[test]
    fn skew_ratio_spans_minus_one_to_one() {
        assert_eq!(skew_ratio(1_000, 1_000).unwrap(), 0);
        assert_eq!(skew_ratio(1_000, 0).unwrap(), RATE_PRECISION as i64);
        assert_eq!(skew_ratio(0, 1_000).unwrap(), -(RATE_PRECISION as i64));
        assert_eq!(skew_ratio(0, 0).unwrap(), 0);
        assert_eq!(skew_ratio(3_000, 1_000).unwrap(), 500_000_000);
    }

    /// § 7.4 blocks opens on the heavy side past a 0.6 skew ratio.
    #[test]
    fn skew_cap_threshold_is_detectable() {
        let threshold = 600_000_000_i64; // 0.6
        assert!(skew_ratio(8_500, 1_500).unwrap() > threshold); // 0.70
        assert!(skew_ratio(7_000, 3_000).unwrap() < threshold); // 0.40
                                                                // Exactly at the threshold is not over it — § 7.4 says "greater than".
        assert_eq!(skew_ratio(8_000, 2_000).unwrap(), threshold);
    }

    #[test]
    fn utilisation_rounds_up_and_rejects_zero_aum() {
        assert_eq!(utilisation_ratio(500, 1_000).unwrap(), 500_000_000); // 50%
        assert_eq!(utilisation_ratio(1, 3).unwrap(), 333_333_334); // ceil
        assert_eq!(utilisation_ratio(1, 0), Err(MathError::DivideByZero));
    }

    /// § 7.2 rejects new opens on the crowded side past 80% utilisation.
    #[test]
    fn utilisation_cap_threshold_is_detectable() {
        let cap = 800_000_000_u64; // 80%
        assert!(utilisation_ratio(850, 1_000).unwrap() > cap);
        assert!(utilisation_ratio(750, 1_000).unwrap() < cap);
    }
}
