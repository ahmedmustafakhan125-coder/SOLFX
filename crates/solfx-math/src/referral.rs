//! Introducing-broker rebate arithmetic (`ARCHITECTURE.md` § 8.5).
//!
//! # Why this is the moat
//!
//! XM and Exness did not win on spreads or platform quality — they won on **IB networks**,
//! armies of affiliates paid a per-lot rebate to bring in traders. Every IB in that industry
//! has the same complaint: *the broker controls the ledger.* Rebates get miscounted, clients
//! get reassigned, payouts get delayed, and the IB has no way to verify any of it.
//!
//! The protocol is replicable in months. A network of brokers who trust your ledger because
//! they can audit it is not. So the arithmetic below is deliberately simple and entirely
//! derivable from public on-chain state: an IB can recompute every figure themselves without
//! trusting us, the indexer, or the referral program.
//!
//! # The pool is the budget, the tier is the entitlement
//!
//! § 8.3 sets the referral pool at **10% of fees**. § 8.5 sets tiers at **8 / 10 / 13 / 16%
//! of fees**. Those two numbers do not reconcile: a Diamond IB cannot be paid 16% of fees
//! out of a pool holding 10%.
//!
//! This module resolves it the honest way — [`entitlement`] pays `tier% of fees`, **capped
//! at what the pool actually holds**, and the remainder sweeps to the treasury exactly as
//! § 8.3 describes. At the launch split that means Bronze and Silver are paid in full while
//! Gold and Diamond are capped at 10%.
//!
//! Raising `fee_split_referral_bps` is an admin parameter, so paying the top tiers in full
//! is a configuration decision made when a Gold IB actually exists — not a promise written
//! into the code before one does.

use crate::constants::BPS_PRECISION;
use crate::error::MathResult;
use crate::fixed::{mul_div_floor, to_u64};

/// Volume thresholds and shares from § 8.5, in USDC at `QUOTE_PRECISION`.
const TIER_TABLE: [(u64, u16); 4] = [
    (5_000_000_000_000, 800),     // < $5M      -> Bronze  8%
    (25_000_000_000_000, 1_000),  // < $25M     -> Silver 10%
    (100_000_000_000_000, 1_300), // < $100M    -> Gold   13%
    (u64::MAX, 1_600),            // >= $100M   -> Diamond 16%
];

/// An IB's tier, set by the 30-day volume of the traders they introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IbTier {
    Bronze = 0,
    Silver = 1,
    Gold = 2,
    Diamond = 3,
}

impl IbTier {
    /// The tier for a given 30-day referred volume.
    ///
    /// Boundaries are **inclusive at the bottom of the higher tier**: exactly $5M is Silver,
    /// not Bronze. Stated explicitly because a boundary that moves under an IB is precisely
    /// the kind of dispute this whole design exists to prevent.
    #[must_use]
    pub fn for_volume(thirty_day_volume: u64) -> Self {
        if thirty_day_volume < TIER_TABLE[0].0 {
            Self::Bronze
        } else if thirty_day_volume < TIER_TABLE[1].0 {
            Self::Silver
        } else if thirty_day_volume < TIER_TABLE[2].0 {
            Self::Gold
        } else {
            Self::Diamond
        }
    }

    /// The IB's share of trading fees, in bps (§ 8.5).
    #[must_use]
    pub fn share_bps(self) -> u16 {
        match self {
            Self::Bronze => TIER_TABLE[0].1,
            Self::Silver => TIER_TABLE[1].1,
            Self::Gold => TIER_TABLE[2].1,
            Self::Diamond => TIER_TABLE[3].1,
        }
    }

    #[must_use]
    pub fn from_index(i: u8) -> Self {
        match i {
            1 => Self::Silver,
            2 => Self::Gold,
            3 => Self::Diamond,
            _ => Self::Bronze,
        }
    }
}

/// What an IB has earned on `pool_share` of accrued referral money.
///
/// `pool_share` is the referral pool's contribution from the referred trader's fees — i.e.
/// `fee × fee_split_referral_bps / 10_000`, which is what `solfx-core` has already set
/// aside. `pool_split_bps` is that same split, needed to reconstruct the original fee.
///
/// ```text
/// fees_generated = pool_share × 10_000 / pool_split_bps
/// entitlement    = min(fees_generated × tier_bps / 10_000,  pool_share)
/// ```
///
/// The cap is the whole point: **an IB can never be paid more than was earmarked for
/// referrals.** Without it a Gold or Diamond tier would quietly draw on treasury money
/// sitting in the same vault.
///
/// Rounds **down** — the protocol never over-pays a rebate on a rounding boundary. The
/// difference is at most one unit and it stays in the pool for the next claim.
pub fn entitlement(pool_share: u64, pool_split_bps: u16, tier: IbTier) -> MathResult<u64> {
    if pool_share == 0 || pool_split_bps == 0 {
        return Ok(0);
    }
    let fees_generated = mul_div_floor(
        u128::from(pool_share),
        BPS_PRECISION,
        u128::from(pool_split_bps),
    )?;
    let desired = mul_div_floor(fees_generated, u128::from(tier.share_bps()), BPS_PRECISION)?;
    to_u64(desired.min(u128::from(pool_share)))
}

/// A parent IB's override on a sub-broker's earnings (§ 8.5's two-level structure).
///
/// § 8.5 asks for a 2-level network because that is what existing FX affiliate networks
/// expect, and matching their mental model lowers switching cost. The parent earns
/// `override_bps` of what the sub-broker earned — **not** a second bite of the trader's
/// fees. It is paid from the same pool and deducted from the same budget, so a two-level
/// chain can never cost more than a one-level one.
///
/// Rounds **down**, and is capped at the remaining budget by the caller.
pub fn parent_override(child_earned: u64, override_bps: u16) -> MathResult<u64> {
    if child_earned == 0 || override_bps == 0 {
        return Ok(0);
    }
    to_u64(mul_div_floor(
        u128::from(child_earned),
        u128::from(override_bps),
        BPS_PRECISION,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: u64 = 1_000_000_000_000; // $1M at QUOTE_PRECISION
    const LAUNCH_SPLIT: u16 = 1_000; // 10% referral pool

    #[test]
    fn tiers_match_the_published_table() {
        assert_eq!(IbTier::for_volume(0), IbTier::Bronze);
        assert_eq!(IbTier::for_volume(4 * M), IbTier::Bronze);
        assert_eq!(IbTier::for_volume(10 * M), IbTier::Silver);
        assert_eq!(IbTier::for_volume(50 * M), IbTier::Gold);
        assert_eq!(IbTier::for_volume(500 * M), IbTier::Diamond);

        assert_eq!(IbTier::Bronze.share_bps(), 800);
        assert_eq!(IbTier::Silver.share_bps(), 1_000);
        assert_eq!(IbTier::Gold.share_bps(), 1_300);
        assert_eq!(IbTier::Diamond.share_bps(), 1_600);
    }

    /// A boundary that moves under an IB is the dispute this design exists to prevent, so
    /// each one is pinned exactly.
    #[test]
    fn tier_boundaries_are_inclusive_at_the_bottom_of_the_higher_tier() {
        assert_eq!(IbTier::for_volume(5 * M - 1), IbTier::Bronze);
        assert_eq!(IbTier::for_volume(5 * M), IbTier::Silver);
        assert_eq!(IbTier::for_volume(25 * M - 1), IbTier::Silver);
        assert_eq!(IbTier::for_volume(25 * M), IbTier::Gold);
        assert_eq!(IbTier::for_volume(100 * M - 1), IbTier::Gold);
        assert_eq!(IbTier::for_volume(100 * M), IbTier::Diamond);
    }

    /// Bronze at 8% of fees against a 10% pool: paid in full, 20% of the pool sweeps to
    /// treasury.
    #[test]
    fn a_bronze_ib_is_paid_in_full_and_the_rest_sweeps() {
        // $100 of fees -> $10 of pool share.
        let pool_share = 10_000_000;
        let earned = entitlement(pool_share, LAUNCH_SPLIT, IbTier::Bronze).unwrap();
        assert_eq!(earned, 8_000_000, "8% of $100 of fees");
        assert_eq!(pool_share - earned, 2_000_000, "sweeps to treasury");
    }

    #[test]
    fn a_silver_ib_takes_exactly_the_whole_pool() {
        let pool_share = 10_000_000;
        assert_eq!(
            entitlement(pool_share, LAUNCH_SPLIT, IbTier::Silver).unwrap(),
            pool_share
        );
    }

    /// **The cap.** Gold wants 13% of fees; the pool holds 10%. The IB gets the pool and no
    /// more — the extra 3% would otherwise come out of the treasury's share of the same
    /// vault.
    #[test]
    fn the_top_tiers_are_capped_at_the_pool() {
        let pool_share = 10_000_000;
        assert_eq!(
            entitlement(pool_share, LAUNCH_SPLIT, IbTier::Gold).unwrap(),
            pool_share
        );
        assert_eq!(
            entitlement(pool_share, LAUNCH_SPLIT, IbTier::Diamond).unwrap(),
            pool_share
        );
    }

    /// Raising the referral split is what actually funds the top tiers — a configuration
    /// decision, made when a Gold IB exists.
    #[test]
    fn a_wider_pool_pays_the_top_tiers_in_full() {
        // 20% pool: $100 of fees -> $20 of pool share. Gold's 13% = $13, now affordable.
        let pool_share = 20_000_000;
        assert_eq!(
            entitlement(pool_share, 2_000, IbTier::Gold).unwrap(),
            13_000_000
        );
        assert_eq!(
            entitlement(pool_share, 2_000, IbTier::Diamond).unwrap(),
            16_000_000
        );
    }

    #[test]
    fn the_parent_override_is_a_share_of_the_child_not_of_the_trader() {
        // Sub-broker earned $8; parent's 20% override is $1.60.
        assert_eq!(parent_override(8_000_000, 2_000).unwrap(), 1_600_000);
        assert_eq!(parent_override(0, 2_000).unwrap(), 0);
        assert_eq!(parent_override(8_000_000, 0).unwrap(), 0);
    }

    #[test]
    fn degenerate_inputs_return_zero_rather_than_erroring() {
        assert_eq!(entitlement(0, LAUNCH_SPLIT, IbTier::Gold).unwrap(), 0);
        assert_eq!(entitlement(1_000, 0, IbTier::Gold).unwrap(), 0);
    }

    #[test]
    fn an_entitlement_never_exceeds_the_pool_share_at_any_tier() {
        for share in [1u64, 7, 999, 1_000_000, u64::from(u32::MAX)] {
            for tier in [
                IbTier::Bronze,
                IbTier::Silver,
                IbTier::Gold,
                IbTier::Diamond,
            ] {
                let e = entitlement(share, LAUNCH_SPLIT, tier).unwrap();
                assert!(e <= share, "tier {tier:?} over-paid on share {share}");
            }
        }
    }
}
