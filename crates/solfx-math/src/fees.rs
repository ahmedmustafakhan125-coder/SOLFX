//! Fee rates, fee amounts, and the four-way split.
//!
//! See `ARCHITECTURE.md` § 8.2 and § 8.3, and correction C-2.
//!
//! Rates live at [`RATE_PRECISION`] (1e9), not in whole basis points. The schedule needs
//! 0.8 bps, which is not expressible as an integer number of bps — storing `open_fee_bps: u16`
//! was the mistake C-2 flags.

use crate::constants::{BPS_PRECISION, RATE_PRECISION};
use crate::error::{MathError, MathResult};
use crate::fixed::{mul_div_ceil, mul_div_floor, to_u64};

/// One basis point expressed at [`RATE_PRECISION`]: `1e9 / 10_000`.
pub const ONE_BPS_RATE: u64 = 100_000;

/// The volume-tiered fee schedule (`ARCHITECTURE.md` § 8.2).
///
/// `(30-day volume upper bound exclusive, fee rate per side)`, both in native units —
/// volume in USDC quote units, rate at [`RATE_PRECISION`].
const FEE_TIERS: [(u64, u64); 5] = [
    (1_000_000_000_000, 150_000),  // < $1M    -> 1.5 bps
    (10_000_000_000_000, 120_000), // < $10M   -> 1.2 bps
    (50_000_000_000_000, 100_000), // < $50M   -> 1.0 bps
    (250_000_000_000_000, 80_000), // < $250M  -> 0.8 bps
    (u64::MAX, 60_000),            // >= $250M -> 0.6 bps
];

/// Fee rate per side for a given 30-day volume, at [`RATE_PRECISION`].
#[must_use]
pub fn fee_rate_for_volume(thirty_day_volume: u64) -> u64 {
    let mut rate = 60_000;
    for (bound, tier_rate) in FEE_TIERS {
        if thirty_day_volume < bound {
            rate = tier_rate;
            break;
        }
    }
    rate
}

/// The fee charged on a trade, in USDC. Rounded **up**, with a floor of one unit.
///
/// The floor closes the rounding-loop drain (threat T14): without it, a trade small enough
/// for the fee to floor to zero could be repeated indefinitely at no cost. Callers should
/// also enforce [`crate::pricing::validate_notional`], which rejects such trades outright;
/// this is the second line of defence.
pub fn fee_amount(notional: u64, rate: u64) -> MathResult<u64> {
    if notional == 0 || rate == 0 {
        return Ok(0);
    }
    let fee = mul_div_ceil(u128::from(notional), u128::from(rate), RATE_PRECISION)?;
    Ok(to_u64(fee)?.max(1))
}

/// How a collected fee is divided (`ARCHITECTURE.md` § 8.3).
///
/// The LP share must be the largest. Under-paying LPs is the single most common cause of
/// death for pool-backed perp protocols: no liquidity means no depth, means no traders,
/// means no fees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeSplitBps {
    pub lp: u16,
    pub treasury: u16,
    pub insurance: u16,
    pub referral: u16,
}

impl FeeSplitBps {
    /// The launch configuration: 55 / 25 / 10 / 10.
    pub const LAUNCH: Self = Self {
        lp: 5_500,
        treasury: 2_500,
        insurance: 1_000,
        referral: 1_000,
    };

    /// Shares must total exactly 100%, or fees silently vanish or over-allocate.
    pub fn validate(&self) -> MathResult<()> {
        let total = u32::from(self.lp)
            .checked_add(u32::from(self.treasury))
            .ok_or(MathError::Overflow)?
            .checked_add(u32::from(self.insurance))
            .ok_or(MathError::Overflow)?
            .checked_add(u32::from(self.referral))
            .ok_or(MathError::Overflow)?;
        if u128::from(total) == BPS_PRECISION {
            Ok(())
        } else {
            Err(MathError::InvalidParameter)
        }
    }
}

/// A fee divided into its destinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeeAllocation {
    pub lp: u64,
    pub treasury: u64,
    pub insurance: u64,
    pub referral: u64,
}

impl FeeAllocation {
    pub fn total(&self) -> MathResult<u64> {
        self.lp
            .checked_add(self.treasury)
            .ok_or(MathError::Overflow)?
            .checked_add(self.insurance)
            .ok_or(MathError::Overflow)?
            .checked_add(self.referral)
            .ok_or(MathError::Overflow)
    }
}

/// Split a fee four ways, conserving every unit.
///
/// LP, insurance and referral each floor; **the treasury takes the remainder**. That makes
/// the split exactly conservative by construction — the alternative (rounding each share
/// independently) either loses units to rounding or allocates more than was collected,
/// and invariant I7 in `ARCHITECTURE.md` § 12.3 would catch it only after the fact.
///
/// The treasury absorbing the dust is the right choice: it is the protocol's own revenue,
/// so the rounding error accrues to the protocol rather than away from it.
pub fn split_fee(fee: u64, split: FeeSplitBps) -> MathResult<FeeAllocation> {
    split.validate()?;

    let share = |bps: u16| -> MathResult<u64> {
        let s = mul_div_floor(u128::from(fee), u128::from(bps), BPS_PRECISION)?;
        to_u64(s)
    };

    let lp = share(split.lp)?;
    let insurance = share(split.insurance)?;
    let referral = share(split.referral)?;

    let treasury = fee
        .checked_sub(lp)
        .ok_or(MathError::Overflow)?
        .checked_sub(insurance)
        .ok_or(MathError::Overflow)?
        .checked_sub(referral)
        .ok_or(MathError::Overflow)?;

    Ok(FeeAllocation {
        lp,
        treasury,
        insurance,
        referral,
    })
}

/// Split a liquidation penalty: liquidator 40% / insurance 40% / treasury 20%
/// (`ARCHITECTURE.md` § 6.8).
///
/// The liquidator's share floors and the treasury takes the remainder, for the same
/// conservation reason as [`split_fee`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PenaltyAllocation {
    pub liquidator: u64,
    pub insurance: u64,
    pub treasury: u64,
}

pub fn split_liquidation_penalty(penalty: u64) -> MathResult<PenaltyAllocation> {
    let liquidator = to_u64(mul_div_floor(u128::from(penalty), 4_000, BPS_PRECISION)?)?;
    let insurance = to_u64(mul_div_floor(u128::from(penalty), 4_000, BPS_PRECISION)?)?;
    let treasury = penalty
        .checked_sub(liquidator)
        .ok_or(MathError::Overflow)?
        .checked_sub(insurance)
        .ok_or(MathError::Overflow)?;
    Ok(PenaltyAllocation {
        liquidator,
        insurance,
        treasury,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const USD: u64 = 1_000_000;

    #[test]
    fn fee_tiers_match_the_architecture_schedule() {
        assert_eq!(fee_rate_for_volume(0), 150_000); // 1.5 bps
        assert_eq!(fee_rate_for_volume(5_000_000 * USD), 120_000); // 1.2 bps
        assert_eq!(fee_rate_for_volume(25_000_000 * USD), 100_000); // 1.0 bps
        assert_eq!(fee_rate_for_volume(100_000_000 * USD), 80_000); // 0.8 bps
        assert_eq!(fee_rate_for_volume(500_000_000 * USD), 60_000); // 0.6 bps
    }

    #[test]
    fn fee_tier_boundaries_are_exclusive_upper() {
        assert_eq!(fee_rate_for_volume(1_000_000 * USD - 1), 150_000);
        assert_eq!(fee_rate_for_volume(1_000_000 * USD), 120_000);
        assert_eq!(fee_rate_for_volume(250_000_000 * USD), 60_000);
    }

    #[test]
    fn fee_rate_falls_monotonically_with_volume() {
        let samples = [
            0_u64,
            1_000_000,
            10_000_000,
            50_000_000,
            250_000_000,
            999_999_999,
        ];
        let mut previous = u64::MAX;
        for s in samples {
            let r = fee_rate_for_volume(s * USD);
            assert!(r <= previous, "rate rose at volume {s}");
            previous = r;
        }
    }

    /// C-2's target: 1 bp per side on a 1-lot EUR/USD position ($108,543) is ~$10.85.
    #[test]
    fn one_bps_on_one_lot_is_about_eleven_dollars() {
        let notional = 108_543 * USD;
        assert_eq!(fee_amount(notional, ONE_BPS_RATE).unwrap(), 10_854_300);
    }

    /// FOREX-EXPLAINED.md § 11: round trip at 1 bp/side is ~$22 on a standard lot.
    #[test]
    fn round_trip_cost_matches_the_explainer() {
        let notional = 108_543 * USD;
        let one_side = fee_amount(notional, ONE_BPS_RATE).unwrap();
        let round_trip = one_side * 2;
        assert!((21_000_000..=22_000_000).contains(&round_trip));
    }

    /// The 0.8 bps tier is the reason rates are not stored in whole basis points.
    #[test]
    fn sub_basis_point_rates_are_expressible() {
        let notional = 1_000_000 * USD;
        assert_eq!(fee_amount(notional, 80_000).unwrap(), 80 * USD); // 0.8 bps of $1M
        assert_eq!(fee_amount(notional, 60_000).unwrap(), 60 * USD); // 0.6 bps
    }

    #[test]
    fn fees_round_up() {
        // 1 unit at 1 bp is 0.0001 units — must still charge 1.
        assert_eq!(fee_amount(1, ONE_BPS_RATE).unwrap(), 1);
    }

    /// Threat T14: no positive-notional trade may ever be free.
    #[test]
    fn no_positive_notional_trade_is_free() {
        for notional in [1_u64, 2, 100, 999, 1_000, 1_000_000] {
            assert!(
                fee_amount(notional, ONE_BPS_RATE).unwrap() >= 1,
                "free trade at notional {notional}"
            );
        }
    }

    #[test]
    fn zero_notional_or_zero_rate_charges_nothing() {
        assert_eq!(fee_amount(0, ONE_BPS_RATE).unwrap(), 0);
        assert_eq!(fee_amount(1_000_000, 0).unwrap(), 0);
    }

    #[test]
    fn launch_split_is_valid_and_lp_led() {
        assert!(FeeSplitBps::LAUNCH.validate().is_ok());
        let s = FeeSplitBps::LAUNCH;
        assert!(s.lp > s.treasury, "LP share must exceed treasury");
        assert!(s.lp > s.insurance + s.referral);
    }

    #[test]
    fn a_split_that_does_not_total_100_percent_is_rejected() {
        let bad = FeeSplitBps {
            lp: 5_000,
            treasury: 2_500,
            insurance: 1_000,
            referral: 1_000,
        };
        assert_eq!(bad.validate(), Err(MathError::InvalidParameter));
        assert_eq!(split_fee(100, bad), Err(MathError::InvalidParameter));
    }

    #[test]
    fn split_matches_the_configured_percentages() {
        let a = split_fee(10_000, FeeSplitBps::LAUNCH).unwrap();
        assert_eq!(a.lp, 5_500);
        assert_eq!(a.treasury, 2_500);
        assert_eq!(a.insurance, 1_000);
        assert_eq!(a.referral, 1_000);
    }

    /// Invariant I7: every unit collected must land somewhere. The treasury absorbs the dust.
    #[test]
    fn split_conserves_every_unit() {
        for fee in [
            0_u64,
            1,
            2,
            3,
            7,
            99,
            101,
            9_999,
            1_000_003,
            u32::MAX as u64,
        ] {
            let a = split_fee(fee, FeeSplitBps::LAUNCH).unwrap();
            assert_eq!(a.total().unwrap(), fee, "leak at fee {fee}");
        }
    }

    /// Dust from flooring must accrue to the protocol, never away from it.
    #[test]
    fn split_dust_goes_to_treasury() {
        // 1 unit cannot be divided; all shares floor to 0 and treasury takes it.
        let a = split_fee(1, FeeSplitBps::LAUNCH).unwrap();
        assert_eq!(a.lp, 0);
        assert_eq!(a.treasury, 1);
        assert_eq!(a.total().unwrap(), 1);
    }

    #[test]
    fn liquidation_penalty_splits_forty_forty_twenty() {
        let p = split_liquidation_penalty(1_000).unwrap();
        assert_eq!(p.liquidator, 400);
        assert_eq!(p.insurance, 400);
        assert_eq!(p.treasury, 200);
    }

    #[test]
    fn liquidation_penalty_split_conserves_every_unit() {
        for penalty in [0_u64, 1, 2, 3, 7, 999, 1_000_001] {
            let p = split_liquidation_penalty(penalty).unwrap();
            let total = p.liquidator + p.insurance + p.treasury;
            assert_eq!(total, penalty, "leak at penalty {penalty}");
        }
    }

    /// ARCHITECTURE.md § 6.8: a 40% share of a 0.5% penalty on a $10,000 position is $20 —
    /// far above the ~$0.002 compute cost, which is what keeps liquidations profitable
    /// during the congestion spikes that cause them.
    #[test]
    fn liquidator_reward_dwarfs_transaction_cost() {
        let penalty = fee_amount(10_000 * USD, 50 * ONE_BPS_RATE).unwrap(); // 0.5%
        let p = split_liquidation_penalty(penalty).unwrap();
        assert_eq!(p.liquidator, 20 * USD);
    }
}
