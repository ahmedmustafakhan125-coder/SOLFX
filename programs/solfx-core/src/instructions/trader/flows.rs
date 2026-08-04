//! Where the money actually goes when a trade settles.
//!
//! Four vaults hold USDC and every trade moves value between them. Getting a single sign
//! wrong here breaks invariants I1, I2, I6 and I7 simultaneously and silently, so the
//! arithmetic is separated from the CPI plumbing and computed in one place:
//!
//! | Vault | Holds | Invariant |
//! |---|---|---|
//! | `collateral_vault` | trader free collateral + position margin | I1 |
//! | `lp_vault` | the counterparty pool | I2 (`== LpPool.aum`) |
//! | `insurance_vault` | bad-debt reserve | I6 (`== InsuranceFund.balance`) |
//! | `fee_vault` | treasury + unclaimed referral accruals | — |
//!
//! # Conservation
//!
//! [`compute_flows`] returns deltas that sum to exactly zero. That is asserted directly in
//! [`Flows::is_conservative`] and property-tested, because "the money went somewhere" is not
//! a claim worth taking on trust in the one module whose job is moving it.

use anchor_lang::prelude::*;
use solfx_math::fees::{split_fee, FeeAllocation, FeeSplitBps};

use crate::errors::{IntoProgramResult, SolfxError};

/// Net movement into (positive) or out of (negative) each vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flows {
    /// Into the LP vault. Negative when the pool pays a winning trader.
    pub lp_delta: i64,
    /// Into the insurance vault. Never negative — Phase 4's bad-debt waterfall is the only
    /// thing that draws it down.
    pub insurance_in: u64,
    /// Into the fee vault: treasury share plus the referral accrual.
    pub fee_vault_in: u64,
    /// Into the collateral vault. Negative when a fee leaves trader ownership.
    pub collateral_delta: i64,
}

impl Flows {
    /// Every unit that leaves one vault must arrive in another.
    #[must_use]
    pub fn is_conservative(&self) -> bool {
        let sum = i128::from(self.lp_delta)
            + i128::from(self.insurance_in)
            + i128::from(self.fee_vault_in)
            + i128::from(self.collateral_delta);
        sum == 0
    }
}

/// Split a fee and net it against realised PnL.
///
/// `pnl` is the trader's realised profit (positive) or loss (negative), already converted
/// into USDC. `fee` is charged to the trader on top.
///
/// # Why the LP flow is netted rather than transferred twice
///
/// The pool both receives the LP share of the fee and pays (or receives) the PnL. Doing those
/// as two transfers costs an extra ~5k CU per trade and can order them so the vault is
/// momentarily short of its own obligation. Netting them into one signed movement is cheaper
/// and cannot transiently under-fund the pool.
pub fn compute_flows(fee: u64, pnl: i64, split: FeeSplitBps) -> Result<(FeeAllocation, Flows)> {
    let alloc = split_fee(fee, split).or_program_err()?;

    // The pool takes its fee share and settles the trader's PnL in the opposite direction.
    let lp_delta = i64::try_from(
        i128::from(alloc.lp)
            .checked_sub(i128::from(pnl))
            .ok_or(SolfxError::MathOverflow)?,
    )
    .map_err(|_| SolfxError::MathOverflow)?;

    let fee_vault_in = alloc
        .treasury
        .checked_add(alloc.referral)
        .ok_or(SolfxError::MathOverflow)?;

    // The trader's side: they receive their PnL and pay the whole fee.
    let collateral_delta = i64::try_from(
        i128::from(pnl)
            .checked_sub(i128::from(fee))
            .ok_or(SolfxError::MathOverflow)?,
    )
    .map_err(|_| SolfxError::MathOverflow)?;

    let flows = Flows {
        lp_delta,
        insurance_in: alloc.insurance,
        fee_vault_in,
        collateral_delta,
    };

    // Not a debug assertion. If this ever fails, USDC has been created or destroyed, and the
    // correct response is to fail the transaction rather than to continue and let invariant
    // I7 discover it later.
    require!(flows.is_conservative(), SolfxError::MathOverflow);

    Ok((alloc, flows))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCH: FeeSplitBps = FeeSplitBps::LAUNCH;

    #[test]
    fn a_fee_with_no_pnl_leaves_the_trader_and_lands_in_the_vaults() {
        let (alloc, flows) = compute_flows(10_000, 0, LAUNCH).unwrap();
        assert_eq!(alloc.total().unwrap(), 10_000);
        assert_eq!(flows.collateral_delta, -10_000);
        assert_eq!(flows.lp_delta, i64::try_from(alloc.lp).unwrap());
        assert_eq!(flows.insurance_in, alloc.insurance);
        assert_eq!(flows.fee_vault_in, alloc.treasury + alloc.referral);
        assert!(flows.is_conservative());
    }

    /// The B-book mechanic: a winning trader is paid by the pool.
    #[test]
    fn a_winning_trade_draws_from_the_pool() {
        let (alloc, flows) = compute_flows(1_000, 50_000, LAUNCH).unwrap();
        assert_eq!(flows.collateral_delta, 49_000, "profit minus the fee");
        assert_eq!(
            flows.lp_delta,
            i64::try_from(alloc.lp).unwrap() - 50_000,
            "the pool nets its fee share against the payout"
        );
        assert!(flows.lp_delta < 0, "the pool must be paying");
        assert!(flows.is_conservative());
    }

    #[test]
    fn a_losing_trade_pays_the_pool() {
        let (alloc, flows) = compute_flows(1_000, -50_000, LAUNCH).unwrap();
        assert_eq!(flows.collateral_delta, -51_000, "loss plus the fee");
        assert_eq!(flows.lp_delta, i64::try_from(alloc.lp).unwrap() + 50_000);
        assert!(flows.lp_delta > 0);
        assert!(flows.is_conservative());
    }

    /// Rounding must not create or destroy a unit. `split_fee` gives the treasury the
    /// remainder precisely so this holds at every fee size.
    #[test]
    fn conservation_holds_across_awkward_fee_sizes() {
        for fee in [1_u64, 2, 3, 7, 999, 1_001, 123_457] {
            for pnl in [-1_000_i64, -1, 0, 1, 1_000] {
                let (alloc, flows) = compute_flows(fee, pnl, LAUNCH).unwrap();
                assert_eq!(alloc.total().unwrap(), fee, "fee {fee} was not fully split");
                assert!(
                    flows.is_conservative(),
                    "flows did not net to zero at fee {fee}, pnl {pnl}"
                );
            }
        }
    }

    #[test]
    fn a_zero_fee_still_settles_pnl() {
        let (alloc, flows) = compute_flows(0, 25_000, LAUNCH).unwrap();
        assert_eq!(alloc.total().unwrap(), 0);
        assert_eq!(flows.collateral_delta, 25_000);
        assert_eq!(flows.lp_delta, -25_000);
        assert!(flows.is_conservative());
    }
}
