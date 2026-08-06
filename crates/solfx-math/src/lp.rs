//! Liquidity-pool share arithmetic (`ARCHITECTURE.md` § 8.1 streams 4–5, threat T6).
//!
//! # The one property everything here serves
//!
//! **A deposit or a withdrawal must never move NAV per share in the actor's favour.**
//!
//! An LP joining must not dilute the LPs already there, and an LP leaving must not take more
//! than their share. Every rounding decision below follows from that, and
//! [`crate::lp`]'s property tests check it across the whole parameter space rather than at a
//! few chosen values — because the failure is not a crash, it is a slow leak that only shows
//! up as LPs quietly earning less than they should.
//!
//! # Rounding, and which way
//!
//! | Quantity | Direction | Who it favours |
//! |---|---|---|
//! | Shares minted for a deposit | **down** | the existing pool |
//! | USDC paid for a redemption | **down** | the remaining pool |
//! | Exit fee | **up** | the remaining pool |
//! | Performance fee | **up** | the treasury |
//!
//! Every one rounds against the party initiating the action. That is the same rule
//! [`crate::fixed`] states for traders, applied to LPs.

use crate::constants::{BPS_PRECISION, QUOTE_PRECISION};
use crate::error::{MathError, MathResult};
use crate::fixed::{mul_div_ceil, mul_div_floor, to_u64};

/// NAV per share at [`QUOTE_PRECISION`], i.e. `1_000_000` == $1.00 per share.
///
/// An empty pool is worth par by definition: the first depositor sets the price, and any
/// other convention would either divide by zero or hand them a free multiple.
pub fn nav_per_share(aum: u64, supply: u64) -> MathResult<u64> {
    if supply == 0 {
        return to_u64(QUOTE_PRECISION);
    }
    to_u64(mul_div_floor(
        u128::from(aum),
        QUOTE_PRECISION,
        u128::from(supply),
    )?)
}

/// Shares minted for a USDC deposit, rounded **down**.
///
/// `shares = amount × supply / aum`, or 1:1 into an empty pool.
///
/// # Why an empty pool mints 1:1 rather than at the last NAV
///
/// A pool can reach `supply == 0` with `aum > 0` — every LP exits while the pool holds
/// retained fees. Minting at the old NAV would hand the next depositor those fees at par;
/// minting 1:1 hands them to the depositor as a *higher* starting NAV, which is equally
/// arbitrary but at least cannot be gamed by timing the last exit. The real defence is that
/// [`crate::lp::shares_for_deposit`] is only reached through an instruction that also
/// enforces a cooldown, so the "exit then immediately re-enter" sequence costs a day.
pub fn shares_for_deposit(amount: u64, aum: u64, supply: u64) -> MathResult<u64> {
    if amount == 0 {
        return Ok(0);
    }
    if supply == 0 || aum == 0 {
        return Ok(amount);
    }
    to_u64(mul_div_floor(
        u128::from(amount),
        u128::from(supply),
        u128::from(aum),
    )?)
}

/// USDC returned for burning shares, rounded **down**.
///
/// `usdc = shares × aum / supply`.
pub fn usdc_for_shares(shares: u64, aum: u64, supply: u64) -> MathResult<u64> {
    if shares == 0 || supply == 0 {
        return Ok(0);
    }
    if shares > supply {
        return Err(MathError::InvalidParameter);
    }
    to_u64(mul_div_floor(
        u128::from(shares),
        u128::from(aum),
        u128::from(supply),
    )?)
}

/// The exit fee, rounded **up**.
///
/// # Where it goes, and why that is not what § 8.1 says
///
/// § 8.1 lists the exit fee as revenue stream 5. **This protocol keeps it in the pool
/// instead**, so it accrues to the LPs who stayed.
///
/// The fee's stated purpose is anti-JIT (threat T6): it exists to make "deposit ahead of a
/// known trader loss, withdraw ahead of a known gain" unprofitable. Paying it to the treasury
/// would mean the protocol profits from LP churn while the LPs who carried the risk get
/// nothing — and it is *their* returns the JIT attacker is diluting. Routing it to them
/// aligns the defence with the party being defended.
///
/// The cost is ~1% of projected revenue (§ 8.1's own estimate). That is a cheap price for a
/// defence that pays the right people.
pub fn exit_fee(gross: u64, fee_bps: u16) -> MathResult<u64> {
    if gross == 0 || fee_bps == 0 {
        return Ok(0);
    }
    to_u64(mul_div_ceil(
        u128::from(gross),
        u128::from(fee_bps),
        BPS_PRECISION,
    )?)
}

/// The performance fee on a redemption, rounded **up** (§ 8.1 stream 4).
///
/// Charged only on the gain **above the high-water mark**: 10% of net LP profit, and nothing
/// at all while the pool is below its previous peak.
///
/// # The known limitation of a single global mark
///
/// A per-LP high-water mark would need per-LP state and a separate account for every
/// provider. This protocol keeps **one mark for the whole pool**, which is the standard vault
/// model, and it has one consequence worth stating plainly:
///
/// > An LP who joins after a drawdown pays no performance fee until the pool recovers its
/// > previous peak — even on gains that are entirely theirs.
///
/// That errs toward the LP, which is the right direction for a fee the protocol charges
/// *itself*. The opposite arrangement — charging a fee on a recovery that only restores an
/// earlier LP's losses — is the one that would be indefensible.
pub fn performance_fee(
    shares: u64,
    nav_now_per_share: u64,
    high_water_mark_per_share: u64,
    fee_bps: u16,
) -> MathResult<u64> {
    if shares == 0 || fee_bps == 0 || nav_now_per_share <= high_water_mark_per_share {
        return Ok(0);
    }
    let gain_per_share = nav_now_per_share
        .checked_sub(high_water_mark_per_share)
        .ok_or(MathError::Overflow)?;

    // shares × gain_per_share / QUOTE_PRECISION = the gain in USDC.
    let gain = mul_div_floor(
        u128::from(shares),
        u128::from(gain_per_share),
        QUOTE_PRECISION,
    )?;
    to_u64(mul_div_ceil(gain, u128::from(fee_bps), BPS_PRECISION)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAR: u64 = 1_000_000; // $1.00 per share

    #[test]
    fn an_empty_pool_is_worth_par() {
        assert_eq!(nav_per_share(0, 0).unwrap(), PAR);
    }

    #[test]
    fn the_first_deposit_mints_one_to_one() {
        assert_eq!(shares_for_deposit(1_000_000, 0, 0).unwrap(), 1_000_000);
    }

    #[test]
    fn a_deposit_into_a_profitable_pool_buys_fewer_shares() {
        // The pool doubled: 1,000 USDC of AUM against 500 shares.
        let aum = 1_000_000_000;
        let supply = 500_000_000;
        assert_eq!(nav_per_share(aum, supply).unwrap(), 2_000_000); // $2.00

        // $100 buys 50 shares, not 100.
        assert_eq!(
            shares_for_deposit(100_000_000, aum, supply).unwrap(),
            50_000_000
        );
    }

    #[test]
    fn redemption_returns_the_share_of_aum() {
        let aum = 1_000_000_000;
        let supply = 500_000_000;
        assert_eq!(
            usdc_for_shares(50_000_000, aum, supply).unwrap(),
            100_000_000
        );
    }

    #[test]
    fn redeeming_the_whole_supply_returns_the_whole_pool() {
        let aum = 1_234_567_890;
        let supply = 999_999_999;
        assert_eq!(usdc_for_shares(supply, aum, supply).unwrap(), aum);
    }

    #[test]
    fn redeeming_more_than_exists_is_rejected() {
        assert_eq!(
            usdc_for_shares(101, 1_000, 100),
            Err(MathError::InvalidParameter)
        );
    }

    /// The anti-dilution property, at a value chosen to force rounding.
    #[test]
    fn a_deposit_never_raises_nav_per_share_for_the_depositor() {
        let (aum, supply) = (1_000_000_007u64, 333_333_331u64);
        let before = nav_per_share(aum, supply).unwrap();

        let deposit = 7_777_777;
        let shares = shares_for_deposit(deposit, aum, supply).unwrap();
        let after = nav_per_share(aum + deposit, supply + shares).unwrap();

        assert!(
            after >= before,
            "a deposit diluted the existing pool: {before} -> {after}"
        );
    }

    #[test]
    fn a_withdrawal_never_lowers_nav_per_share_for_those_who_stay() {
        let (aum, supply) = (1_000_000_007u64, 333_333_331u64);
        let before = nav_per_share(aum, supply).unwrap();

        let shares = 111_111_111;
        let paid = usdc_for_shares(shares, aum, supply).unwrap();
        let after = nav_per_share(aum - paid, supply - shares).unwrap();

        assert!(
            after >= before,
            "a withdrawal diluted the remaining pool: {before} -> {after}"
        );
    }

    #[test]
    fn the_exit_fee_rounds_up() {
        // 5 bps of $100.01 is $0.050005 -> rounds to 50_001 units.
        assert_eq!(exit_fee(100_010_000, 5).unwrap(), 50_005);
        // A fee that would floor to zero still charges one unit.
        assert_eq!(exit_fee(1, 5).unwrap(), 1);
        assert_eq!(exit_fee(0, 5).unwrap(), 0);
        assert_eq!(exit_fee(1_000_000, 0).unwrap(), 0);
    }

    #[test]
    fn no_performance_fee_below_the_high_water_mark() {
        // NAV $0.90 against a $1.00 mark.
        assert_eq!(
            performance_fee(1_000_000_000, 900_000, PAR, 1_000).unwrap(),
            0
        );
        // Exactly at the mark is not a gain.
        assert_eq!(performance_fee(1_000_000_000, PAR, PAR, 1_000).unwrap(), 0);
    }

    #[test]
    fn the_performance_fee_is_ten_percent_of_the_gain_above_the_mark() {
        // 1,000 shares, NAV $1.20 against a $1.00 mark: $200 of gain, 10% = $20.
        let fee = performance_fee(1_000_000_000, 1_200_000, PAR, 1_000).unwrap();
        assert_eq!(fee, 20_000_000);
    }

    /// The documented limitation, asserted so it cannot change silently.
    #[test]
    fn an_lp_who_joined_after_a_drawdown_pays_nothing_until_the_peak_returns() {
        let hwm = 2_000_000; // the pool peaked at $2.00
                             // It fell to $1.00 and has recovered to $1.50. Entirely gain for a recent joiner.
        assert_eq!(
            performance_fee(1_000_000_000, 1_500_000, hwm, 1_000).unwrap(),
            0,
            "a single global mark errs toward the LP; see the doc comment"
        );
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert_eq!(shares_for_deposit(0, 100, 100).unwrap(), 0);
        assert_eq!(usdc_for_shares(0, 100, 100).unwrap(), 0);
        assert_eq!(usdc_for_shares(10, 100, 0).unwrap(), 0);
        // A single share against the whole u64 range: NAV per share would be
        // `u64::MAX x 1e6`, which does not fit. It errors rather than wrapping.
        assert_eq!(nav_per_share(u64::MAX, 1), Err(MathError::Overflow));
        assert_eq!(nav_per_share(u64::MAX, 1_000_000).unwrap(), u64::MAX);
        assert!(shares_for_deposit(u64::MAX, 1, u64::MAX).is_err());
    }
}
