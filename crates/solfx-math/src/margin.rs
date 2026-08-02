//! Margin requirements, account equity and the liquidation test.
//!
//! See `ARCHITECTURE.md` § 6.3 and § 6.5.

use crate::constants::{BPS_PRECISION, NOTIONAL_DIVISOR_I128};
use crate::error::{MathError, MathResult};
use crate::fixed::{
    div_ceil, i128_to_u128, mul_div_ceil, mul_div_floor, mul_div_floor_signed, to_i64, to_u64,
};
use crate::types::Direction;

/// Health factor scaled by [`BPS_PRECISION`]: `10_000` is exactly 1.0.
pub const HEALTH_FACTOR_ONE: u64 = 10_000;

/// Size tiers for the maintenance-margin multiplier (`ARCHITECTURE.md` § 6.3).
///
/// A $50k position and a $5M position are not the same risk, so MMR scales with notional.
/// Each entry is `(upper_bound_exclusive, multiplier_bps)`, bounds in USDC quote units.
const MMR_TIERS: [(u64, u64); 4] = [
    (100_000_000_000, 10_000),   // < $100k -> 1.0x
    (1_000_000_000_000, 15_000), // < $1M   -> 1.5x
    (5_000_000_000_000, 25_000), // < $5M   -> 2.5x
    (u64::MAX, 40_000),          // >= $5M  -> 4.0x
];

/// The MMR multiplier for a given notional, in basis points (`10_000` = 1.0x).
#[must_use]
pub fn mmr_multiplier_bps(notional: u64) -> u64 {
    let mut result = 40_000;
    for (bound, multiplier) in MMR_TIERS {
        if notional < bound {
            result = multiplier;
            break;
        }
    }
    result
}

/// Initial margin required to open, in USDC. Rounded **up**.
pub fn initial_margin(notional: u64, imr_bps: u16) -> MathResult<u64> {
    if imr_bps == 0 {
        return Err(MathError::InvalidParameter);
    }
    let m = mul_div_ceil(u128::from(notional), u128::from(imr_bps), BPS_PRECISION)?;
    to_u64(m)
}

/// Maintenance margin, in USDC, with the size tier applied. Rounded **up**.
///
/// This is the number equity is compared against; it must never round down, or a position
/// stays alive one tick past the point the vault can cover it.
pub fn maintenance_margin(notional: u64, mmr_bps: u16) -> MathResult<u64> {
    if mmr_bps == 0 {
        return Err(MathError::InvalidParameter);
    }
    let multiplier = mmr_multiplier_bps(notional);
    let base = mul_div_ceil(u128::from(notional), u128::from(mmr_bps), BPS_PRECISION)?;
    let tiered = mul_div_ceil(base, u128::from(multiplier), BPS_PRECISION)?;
    to_u64(tiered)
}

/// Effective leverage of a position: `notional / collateral`, rounded **up**.
pub fn effective_leverage(notional: u64, collateral: u64) -> MathResult<u64> {
    if collateral == 0 {
        return Err(MathError::DivideByZero);
    }
    let l = div_ceil(u128::from(notional), u128::from(collateral))?;
    to_u64(l)
}

/// All costs accrued against a position. Each component must already be rounded in the
/// protocol's favour by whoever produced it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccruedCosts {
    /// Carry (the FX swap equivalent), in USDC. Always a charge.
    pub carry: u64,
    /// Funding owed, in USDC. Signed — a position on the light side of the book *receives*.
    pub funding: i64,
    /// Estimated cost to close, in USDC.
    pub close_fee: u64,
}

/// Position equity: what the trader would walk away with if closed right now.
///
/// `collateral + uPnL − carry − funding − close_fee`
///
/// Signed, because it genuinely can go negative — that is bad debt, and
/// `ARCHITECTURE.md` § 6.9 defines the waterfall that absorbs it.
pub fn equity(collateral: u64, upnl: i64, costs: AccruedCosts) -> MathResult<i64> {
    let e = i128::from(collateral)
        .checked_add(i128::from(upnl))
        .ok_or(MathError::Overflow)?
        .checked_sub(i128::from(costs.carry))
        .ok_or(MathError::Overflow)?
        .checked_sub(i128::from(costs.funding))
        .ok_or(MathError::Overflow)?
        .checked_sub(i128::from(costs.close_fee))
        .ok_or(MathError::Overflow)?;
    to_i64(e)
}

/// The liquidation test.
///
/// A direct comparison rather than a ratio: it avoids a division and the divide-by-zero
/// branch that comes with it (`ARCHITECTURE.md` § 6.5).
///
/// **Strict.** Equity exactly equal to the maintenance margin is *not* liquidatable — a
/// loose comparison here is liquidation griefing (threat T7).
#[must_use]
pub fn is_liquidatable(equity: i64, maintenance_margin: u64) -> bool {
    i128::from(equity) < i128::from(maintenance_margin)
}

/// Health factor scaled so `10_000` == 1.0. Presentation only — never branch on this,
/// use [`is_liquidatable`].
///
/// Non-positive equity returns `0`; a zero maintenance margin returns `u64::MAX`
/// (a position that cannot be liquidated by definition).
pub fn health_factor_bps(equity: i64, maintenance_margin: u64) -> MathResult<u64> {
    if equity <= 0 {
        return Ok(0);
    }
    if maintenance_margin == 0 {
        return Ok(u64::MAX);
    }
    let hf = mul_div_floor(
        i128_to_u128(i128::from(equity))?,
        BPS_PRECISION,
        u128::from(maintenance_margin),
    )?;
    to_u64(hf)
}

/// The price at which this position becomes liquidatable — the number the trader
/// actually watches (`ARCHITECTURE.md` § 10.2).
///
/// Solves `equity(price) == maintenance_margin` for price, holding size and costs fixed.
///
/// Only valid for USD-quoted markets. A non-USD-quoted market's liquidation price also
/// moves with the conversion rate, so the caller must solve that case itself.
pub fn liquidation_price(
    size_base: u64,
    entry_price: i64,
    collateral: u64,
    maintenance_margin: u64,
    costs: AccruedCosts,
    direction: Direction,
) -> MathResult<i64> {
    if size_base == 0 {
        return Err(MathError::DivideByZero);
    }
    if entry_price <= 0 {
        return Err(MathError::InvalidPrice);
    }

    // How much PnL the position can lose before equity reaches the maintenance margin.
    let budget = i128::from(collateral)
        .checked_sub(i128::from(costs.carry))
        .ok_or(MathError::Overflow)?
        .checked_sub(i128::from(costs.funding))
        .ok_or(MathError::Overflow)?
        .checked_sub(i128::from(costs.close_fee))
        .ok_or(MathError::Overflow)?
        .checked_sub(i128::from(maintenance_margin))
        .ok_or(MathError::Overflow)?;

    // Convert that quote-unit budget back into a price move.
    let move_magnitude =
        mul_div_floor_signed(budget, NOTIONAL_DIVISOR_I128, i128::from(size_base))?;

    // A long is liquidated by a fall, a short by a rise.
    let liq = i128::from(entry_price)
        .checked_sub(
            move_magnitude
                .checked_mul(direction.sign())
                .ok_or(MathError::Overflow)?,
        )
        .ok_or(MathError::Overflow)?;

    // A position already past liquidation has no meaningful price; clamp at 1 unit.
    if liq <= 0 {
        return Ok(1);
    }
    to_i64(liq)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One USDC in native units.
    const USD: u64 = 1_000_000;

    #[test]
    fn mmr_tiers_match_the_architecture_table() {
        assert_eq!(mmr_multiplier_bps(50_000 * USD), 10_000); // 1.0x
        assert_eq!(mmr_multiplier_bps(500_000 * USD), 15_000); // 1.5x
        assert_eq!(mmr_multiplier_bps(2_000_000 * USD), 25_000); // 2.5x
        assert_eq!(mmr_multiplier_bps(10_000_000 * USD), 40_000); // 4.0x
    }

    /// Boundaries are exclusive-upper: exactly $100k is already in the next tier.
    #[test]
    fn mmr_tier_boundaries_are_exclusive_upper() {
        assert_eq!(mmr_multiplier_bps(100_000 * USD - 1), 10_000);
        assert_eq!(mmr_multiplier_bps(100_000 * USD), 15_000);
        assert_eq!(mmr_multiplier_bps(1_000_000 * USD - 1), 15_000);
        assert_eq!(mmr_multiplier_bps(1_000_000 * USD), 25_000);
        assert_eq!(mmr_multiplier_bps(5_000_000 * USD - 1), 25_000);
        assert_eq!(mmr_multiplier_bps(5_000_000 * USD), 40_000);
    }

    #[test]
    fn mmr_multiplier_is_monotonic_in_notional() {
        let samples = [
            0, 1, 99_999, 100_000, 999_999, 1_000_000, 4_999_999, 5_000_000,
        ];
        let mut previous = 0;
        for s in samples {
            let m = mmr_multiplier_bps(s * USD);
            assert!(m >= previous, "multiplier fell at notional {s}");
            previous = m;
        }
    }

    /// FOREX-EXPLAINED.md § 8: 1 lot EUR/USD at 1.0850 needs $2,170 initial at 50x.
    #[test]
    fn initial_margin_matches_the_explainer_worked_example() {
        let notional = 108_500 * USD;
        assert_eq!(initial_margin(notional, 200).unwrap(), 2_170 * USD); // 2% = 50x
    }

    /// $108.5k lands in the 1.5x tier, so 1% MMR becomes an effective 1.5%.
    #[test]
    fn maintenance_margin_applies_the_size_tier() {
        let notional = 108_500 * USD;
        assert_eq!(maintenance_margin(notional, 100).unwrap(), 1_627_500_000);
    }

    /// Under $100k the tier multiplier is 1.0x, so MMR is the raw percentage.
    #[test]
    fn maintenance_margin_untiered_below_100k() {
        assert_eq!(maintenance_margin(50_000 * USD, 100).unwrap(), 500 * USD);
    }

    #[test]
    fn margin_requirements_round_up() {
        // 1 quote unit at 1 bp is 0.0001 units — must still charge 1.
        assert_eq!(initial_margin(1, 1).unwrap(), 1);
        assert_eq!(maintenance_margin(1, 1).unwrap(), 1);
    }

    #[test]
    fn zero_margin_ratio_is_rejected() {
        assert_eq!(initial_margin(1000, 0), Err(MathError::InvalidParameter));
        assert_eq!(
            maintenance_margin(1000, 0),
            Err(MathError::InvalidParameter)
        );
    }

    #[test]
    fn equity_subtracts_every_cost() {
        let costs = AccruedCosts {
            carry: 10 * USD,
            funding: 5_000_000,
            close_fee: 2 * USD,
        };
        let e = equity(1_000 * USD, 100_000_000, costs).unwrap();
        assert_eq!(e, 1_083_000_000); // 1000 + 100 - 10 - 5 - 2
    }

    /// Funding is signed: a position on the light side of the book receives it.
    #[test]
    fn negative_funding_increases_equity() {
        let costs = AccruedCosts {
            carry: 0,
            funding: -20_000_000,
            close_fee: 0,
        };
        assert_eq!(equity(1_000 * USD, 0, costs).unwrap(), 1_020_000_000);
    }

    #[test]
    fn equity_can_go_negative() {
        let e = equity(100 * USD, -500_000_000, AccruedCosts::default()).unwrap();
        assert_eq!(e, -400_000_000);
        assert!(is_liquidatable(e, 1));
    }

    /// A loose comparison here is liquidation griefing (threat T7).
    #[test]
    fn liquidation_boundary_is_strict() {
        let mm = 1_000 * USD;
        assert!(!is_liquidatable(1_000_000_000, mm));
        assert!(is_liquidatable(999_999_999, mm));
        assert!(!is_liquidatable(1_000_000_001, mm));
    }

    /// Negative equity is always liquidatable, even against a zero margin requirement.
    #[test]
    fn negative_equity_is_always_liquidatable() {
        assert!(is_liquidatable(-1, 0));
        assert!(is_liquidatable(i64::MIN, 0));
    }

    #[test]
    fn health_factor_reads_one_at_the_boundary() {
        let mm = 1_085 * USD;
        assert_eq!(
            health_factor_bps(1_085_000_000, mm).unwrap(),
            HEALTH_FACTOR_ONE
        );
        // FOREX-EXPLAINED.md § 8: $2,170 equity against $1,085 maintenance is 2.00.
        assert_eq!(health_factor_bps(2_170_000_000, mm).unwrap(), 20_000);
    }

    #[test]
    fn health_factor_handles_degenerate_inputs() {
        assert_eq!(health_factor_bps(-1, 1_000).unwrap(), 0);
        assert_eq!(health_factor_bps(0, 1_000).unwrap(), 0);
        assert_eq!(health_factor_bps(1_000, 0).unwrap(), u64::MAX);
    }

    /// The displayed health factor and the actual liquidation test must never disagree,
    /// or the UI shows "safe" on a position a keeper is about to close.
    #[test]
    fn health_factor_and_liquidatable_agree() {
        let mm = 1_000 * USD;
        for e in [
            0_i64,
            1,
            999_999_999,
            1_000_000_000,
            1_000_000_001,
            5_000_000_000,
        ] {
            let hf = health_factor_bps(e, mm).unwrap();
            assert_eq!(
                hf < HEALTH_FACTOR_ONE,
                is_liquidatable(e, mm),
                "disagreement at equity {e} (hf {hf})"
            );
        }
    }

    #[test]
    fn effective_leverage_rounds_up() {
        assert_eq!(effective_leverage(100_000, 10_000).unwrap(), 10);
        assert_eq!(effective_leverage(100_001, 10_000).unwrap(), 11);
        assert_eq!(effective_leverage(1, 0), Err(MathError::DivideByZero));
    }

    /// FOREX-EXPLAINED.md § 8: long 1 lot at 1.0850 with $2,170 margin and $1,085
    /// maintenance liquidates at 1.07415 — a 1% move.
    #[test]
    fn liquidation_price_matches_the_explainer_worked_example() {
        let liq = liquidation_price(
            100_000_000_000_000, // 1 lot
            1_085_000_000,       // 1.0850
            2_170 * USD,
            1_085 * USD,
            AccruedCosts::default(),
            Direction::Long,
        )
        .unwrap();
        assert_eq!(liq, 1_074_150_000); // 1.07415
    }

    /// A short is liquidated by a rise, so its liquidation price sits above entry.
    #[test]
    fn short_liquidation_price_is_above_entry() {
        let entry = 1_085_000_000;
        let liq = liquidation_price(
            100_000_000_000_000,
            entry,
            2_170 * USD,
            1_085 * USD,
            AccruedCosts::default(),
            Direction::Short,
        )
        .unwrap();
        assert_eq!(liq, 1_095_850_000);
        assert!(liq > entry);
    }

    /// Accrued costs eat the buffer, so liquidation moves closer to entry.
    #[test]
    fn accrued_costs_pull_liquidation_price_toward_entry() {
        let clean = liquidation_price(
            100_000_000_000_000,
            1_085_000_000,
            2_170 * USD,
            1_085 * USD,
            AccruedCosts::default(),
            Direction::Long,
        )
        .unwrap();
        let dirty = liquidation_price(
            100_000_000_000_000,
            1_085_000_000,
            2_170 * USD,
            1_085 * USD,
            AccruedCosts {
                carry: 100 * USD,
                funding: 0,
                close_fee: 0,
            },
            Direction::Long,
        )
        .unwrap();
        assert!(
            dirty > clean,
            "carry should raise a long's liquidation price"
        );
    }

    #[test]
    fn liquidation_price_rejects_degenerate_inputs() {
        assert_eq!(
            liquidation_price(0, 1, 1, 1, AccruedCosts::default(), Direction::Long),
            Err(MathError::DivideByZero)
        );
        assert_eq!(
            liquidation_price(1, 0, 1, 1, AccruedCosts::default(), Direction::Long),
            Err(MathError::InvalidPrice)
        );
    }

    /// An already-underwater long clamps at 1 rather than returning a negative price.
    #[test]
    fn liquidation_price_clamps_instead_of_going_negative() {
        let liq = liquidation_price(
            100_000_000_000_000,
            1_000_000_000,
            10_000_000 * USD, // absurd collateral -> price would go far below zero
            0,
            AccruedCosts::default(),
            Direction::Long,
        )
        .unwrap();
        assert_eq!(liq, 1);
    }
}
