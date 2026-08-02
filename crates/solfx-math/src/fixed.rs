//! Checked fixed-point arithmetic with **explicit** rounding direction.
//!
//! This is the only module in the protocol permitted to use raw `/` and `%`. Everything else
//! calls these primitives, so there is exactly one place to audit for rounding bugs
//! (`ARCHITECTURE.md` ADR-005).
//!
//! # The rounding rule
//!
//! Always round in the protocol's favour:
//!
//! | Quantity | Direction | Function |
//! |---|---|---|
//! | Fees, margin requirements, costs charged to the trader | up | [`mul_div_ceil`] |
//! | Payouts to the trader | down | [`mul_div_floor`] |
//! | Signed trader PnL | toward −∞ | [`mul_div_floor_signed`] |
//!
//! # Why signed PnL needs its own function
//!
//! Rust's `/` truncates **toward zero**, so `-7 / 2 == -3`. A trader owing 3.5 units would be
//! charged 3 — the protocol eating the remainder on every losing position. [`mul_div_floor_signed`]
//! rounds toward −∞ (`-4`), which shrinks gains and grows losses. That is protocol-favourable in
//! both signs, which is the property [`crate::pnl`] depends on.

// The single audited exception to the workspace-wide bans. Raw `/` and `%` appear only here,
// each guarded by an explicit zero check, and each with its rounding direction named.
#![allow(clippy::integer_division, clippy::arithmetic_side_effects)]

use crate::error::{MathError, MathResult};

/// `a * b / d`, rounded **down**. Use for payouts.
pub fn mul_div_floor(a: u128, b: u128, d: u128) -> MathResult<u128> {
    if d == 0 {
        return Err(MathError::DivideByZero);
    }
    let n = a.checked_mul(b).ok_or(MathError::Overflow)?;
    Ok(n / d)
}

/// `a * b / d`, rounded **up**. Use for fees, margin requirements and any cost charged
/// to the trader.
pub fn mul_div_ceil(a: u128, b: u128, d: u128) -> MathResult<u128> {
    if d == 0 {
        return Err(MathError::DivideByZero);
    }
    let n = a.checked_mul(b).ok_or(MathError::Overflow)?;
    let q = n / d;
    if n.is_multiple_of(d) {
        Ok(q)
    } else {
        q.checked_add(1).ok_or(MathError::Overflow)
    }
}

/// `a * b / d`, rounded toward **−∞** (true floor, not truncation).
///
/// `d` must be positive. Use for any signed quantity representing trader PnL: flooring
/// shrinks gains and grows losses, so it favours the protocol regardless of sign.
pub fn mul_div_floor_signed(a: i128, b: i128, d: i128) -> MathResult<i128> {
    if d == 0 {
        return Err(MathError::DivideByZero);
    }
    if d < 0 {
        return Err(MathError::InvalidParameter);
    }
    let n = a.checked_mul(b).ok_or(MathError::Overflow)?;
    let q = n / d;
    let r = n % d;
    if r < 0 {
        q.checked_sub(1).ok_or(MathError::Overflow)
    } else {
        Ok(q)
    }
}

/// `a * b / d`, rounded toward **+∞** (true ceiling, not truncation).
///
/// `d` must be positive. Use for any signed quantity representing a *cost rate* charged to
/// the trader: ceiling grows a charge and shrinks a credit, so it favours the protocol
/// regardless of sign. This is the mirror of [`mul_div_floor_signed`], which is correct for
/// trader *value* (PnL) — the two must not be interchanged.
pub fn mul_div_ceil_signed(a: i128, b: i128, d: i128) -> MathResult<i128> {
    if d == 0 {
        return Err(MathError::DivideByZero);
    }
    if d < 0 {
        return Err(MathError::InvalidParameter);
    }
    let n = a.checked_mul(b).ok_or(MathError::Overflow)?;
    let q = n / d;
    let r = n % d;
    if r > 0 {
        q.checked_add(1).ok_or(MathError::Overflow)
    } else {
        Ok(q)
    }
}

/// Integer division rounded **up**. `d` must be non-zero.
pub fn div_ceil(n: u128, d: u128) -> MathResult<u128> {
    if d == 0 {
        return Err(MathError::DivideByZero);
    }
    let q = n / d;
    if n.is_multiple_of(d) {
        Ok(q)
    } else {
        q.checked_add(1).ok_or(MathError::Overflow)
    }
}

/// Integer division rounded **down**. `d` must be non-zero.
pub fn div_floor(n: u128, d: u128) -> MathResult<u128> {
    if d == 0 {
        return Err(MathError::DivideByZero);
    }
    Ok(n / d)
}

/// `|v|` without the `i128::MIN` panic.
pub fn abs_i128(v: i128) -> MathResult<i128> {
    v.checked_abs().ok_or(MathError::Overflow)
}

// --- narrowing casts that fail loudly instead of truncating -------------------------------

pub fn to_u64(v: u128) -> MathResult<u64> {
    u64::try_from(v).map_err(|_| MathError::Overflow)
}

pub fn to_i64(v: i128) -> MathResult<i64> {
    i64::try_from(v).map_err(|_| MathError::Overflow)
}

/// Reinterpret a non-negative signed value as unsigned.
pub fn i128_to_u128(v: i128) -> MathResult<u128> {
    u128::try_from(v).map_err(|_| MathError::Overflow)
}

/// Widen a **strictly positive** price or rate into `u128`.
///
/// Rejects zero and negatives rather than casting them, so the "price must be positive"
/// precondition is enforced by the type conversion itself rather than by a separate
/// check a caller could forget.
pub fn positive_to_u128(v: i64) -> MathResult<u128> {
    if v <= 0 {
        return Err(MathError::InvalidPrice);
    }
    u128::try_from(v).map_err(|_| MathError::Overflow)
}

/// Widen an unsigned value into signed space.
pub fn u128_to_i128(v: u128) -> MathResult<i128> {
    i128::try_from(v).map_err(|_| MathError::Overflow)
}

/// Clamp `v` into `[-bound, bound]`. `bound` must be non-negative.
pub fn clamp_symmetric(v: i128, bound: i128) -> MathResult<i128> {
    if bound < 0 {
        return Err(MathError::InvalidParameter);
    }
    let lower = bound.checked_neg().ok_or(MathError::Overflow)?;
    Ok(v.clamp(lower, bound))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_and_ceil_agree_on_exact_division() {
        assert_eq!(mul_div_floor(10, 10, 5).unwrap(), 20);
        assert_eq!(mul_div_ceil(10, 10, 5).unwrap(), 20);
    }

    #[test]
    fn ceil_rounds_up_on_remainder() {
        assert_eq!(mul_div_floor(10, 10, 3).unwrap(), 33);
        assert_eq!(mul_div_ceil(10, 10, 3).unwrap(), 34);
    }

    /// The bug this module exists to prevent: `/` truncates toward zero, which would
    /// round a trader's loss in the trader's favour.
    #[test]
    fn signed_floor_rounds_toward_negative_infinity_not_toward_zero() {
        assert_eq!(-7_i128 / 2, -3); // what Rust does
        assert_eq!(mul_div_floor_signed(-7, 1, 2).unwrap(), -4); // what we need
    }

    /// The mirror of the above: a signed *cost* must round the other way, growing a charge
    /// and shrinking a credit.
    #[test]
    fn signed_ceil_grows_charges_and_shrinks_credits() {
        assert_eq!(mul_div_ceil_signed(7, 1, 2).unwrap(), 4); // +3.5 -> +4 (charge grows)
        assert_eq!(mul_div_ceil_signed(-7, 1, 2).unwrap(), -3); // -3.5 -> -3 (credit shrinks)
    }

    /// Floor and ceil must bracket the true value, never both land on the same side.
    #[test]
    fn signed_floor_and_ceil_bracket_the_true_value() {
        for n in [-9_i128, -7, -1, 0, 1, 7, 9] {
            let f = mul_div_floor_signed(n, 1, 2).unwrap();
            let c = mul_div_ceil_signed(n, 1, 2).unwrap();
            assert!(c >= f, "ceil below floor at {n}");
            assert!(c - f <= 1, "gap wider than one unit at {n}");
        }
    }

    #[test]
    fn signed_floor_shrinks_gains_and_grows_losses() {
        // +3.5 -> +3 (gain shrinks)
        assert_eq!(mul_div_floor_signed(7, 1, 2).unwrap(), 3);
        // -3.5 -> -4 (loss grows)
        assert_eq!(mul_div_floor_signed(-7, 1, 2).unwrap(), -4);
    }

    #[test]
    fn divide_by_zero_is_an_error_not_a_panic() {
        assert_eq!(mul_div_floor(1, 1, 0), Err(MathError::DivideByZero));
        assert_eq!(mul_div_ceil(1, 1, 0), Err(MathError::DivideByZero));
        assert_eq!(mul_div_floor_signed(1, 1, 0), Err(MathError::DivideByZero));
        assert_eq!(div_ceil(1, 0), Err(MathError::DivideByZero));
        assert_eq!(div_floor(1, 0), Err(MathError::DivideByZero));
    }

    #[test]
    fn negative_divisor_is_rejected() {
        assert_eq!(
            mul_div_floor_signed(1, 1, -2),
            Err(MathError::InvalidParameter)
        );
        assert_eq!(
            mul_div_ceil_signed(1, 1, -2),
            Err(MathError::InvalidParameter)
        );
    }

    #[test]
    fn signed_ceil_rejects_degenerate_divisors() {
        assert_eq!(mul_div_ceil_signed(1, 1, 0), Err(MathError::DivideByZero));
        assert_eq!(
            mul_div_ceil_signed(i128::MAX, 2, 1),
            Err(MathError::Overflow)
        );
    }

    #[test]
    fn overflow_is_an_error_not_a_wrap() {
        assert_eq!(mul_div_floor(u128::MAX, 2, 1), Err(MathError::Overflow));
        assert_eq!(mul_div_ceil(u128::MAX, 2, 1), Err(MathError::Overflow));
        assert_eq!(
            mul_div_floor_signed(i128::MAX, 2, 1),
            Err(MathError::Overflow)
        );
    }

    #[test]
    fn abs_handles_i128_min_without_panicking() {
        assert_eq!(abs_i128(i128::MIN), Err(MathError::Overflow));
        assert_eq!(abs_i128(-5).unwrap(), 5);
    }

    #[test]
    fn narrowing_casts_reject_rather_than_truncate() {
        assert_eq!(to_u64(u128::from(u64::MAX) + 1), Err(MathError::Overflow));
        assert_eq!(to_i64(i128::from(i64::MAX) + 1), Err(MathError::Overflow));
        assert_eq!(i128_to_u128(-1), Err(MathError::Overflow));
        assert_eq!(to_u64(u128::from(u64::MAX)).unwrap(), u64::MAX);
    }

    #[test]
    fn positive_to_u128_enforces_its_precondition() {
        assert_eq!(positive_to_u128(0), Err(MathError::InvalidPrice));
        assert_eq!(positive_to_u128(-1), Err(MathError::InvalidPrice));
        assert_eq!(positive_to_u128(5).unwrap(), 5);
    }

    #[test]
    fn clamp_symmetric_bounds_both_directions() {
        assert_eq!(clamp_symmetric(100, 10).unwrap(), 10);
        assert_eq!(clamp_symmetric(-100, 10).unwrap(), -10);
        assert_eq!(clamp_symmetric(5, 10).unwrap(), 5);
        assert_eq!(clamp_symmetric(5, -1), Err(MathError::InvalidParameter));
    }
}
