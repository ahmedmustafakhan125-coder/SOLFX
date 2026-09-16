//! How a settled mandate's money is divided. Pure arithmetic, no accounts.
//!
//! Kept apart from the instruction so the one property that matters — **every unit of the
//! final balance goes to exactly one party** (invariant N4) — can be tested across thousands
//! of inputs rather than the three a transaction test can afford.
//!
//! # The order of operations, from `docs/NOXFUNDS-PLAN.md` §5.1
//!
//! The 5% comes off the **gross** profit first, so the 70/30 split stays exactly 70/30 on what
//! remains. The alternative, a 70/25/5 three-way split, would quietly cut the investor to 25%.
//!
//! ```text
//! principal $3,500, final $4,500
//! gross profit                     $1,000
//! − protocol fee (5% of gross)     $   50
//! net                              $  950
//!   → trader   70% of net          $  665
//!   → investor principal + 30%     $3,785
//! ```
//!
//! # Rounding
//!
//! Every division rounds against the party it is computed for, and the investor receives the
//! remainder. The fee is a ceiling (a charge rounds toward whoever charges it, as everywhere in
//! `solfx-math`); the trader's share is a floor (a payout rounds down); the investor gets
//! whatever is left, so rounding dust accrues to the party whose capital was at risk rather
//! than disappearing. Because the investor's figure is a subtraction and not a third division,
//! the parts sum to the whole **by construction**.
//!
//! # A loss
//!
//! There is no profit to share and no fee to take. The investor receives everything that is
//! left — the protocol never earns from a losing mandate, and the trader is never charged for
//! one. That is what "the investor bears the trading loss" means in money.

use solfx_math::fixed::{mul_div_ceil, mul_div_floor, to_u64};
use solfx_math::MathResult;

use crate::constants::BPS;

/// Who receives what, in USDC at `QUOTE_PRECISION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    pub investor: u64,
    pub trader: u64,
    pub protocol: u64,
    /// `final − principal` when positive, else zero. Emitted, not paid.
    pub gross_profit: u64,
}

/// Divide `final_equity` between investor, trader and protocol.
///
/// `fee_bps` is the protocol's share of **gross** profit; `trader_bps` is the trader's share of
/// profit **net** of that fee.
pub fn split(
    principal: u64,
    final_equity: u64,
    fee_bps: u16,
    trader_bps: u16,
) -> MathResult<Split> {
    let Some(gross) = final_equity.checked_sub(principal).filter(|g| *g > 0) else {
        // At or below principal: no profit, no fee, no trader share.
        return Ok(Split {
            investor: final_equity,
            trader: 0,
            protocol: 0,
            gross_profit: 0,
        });
    };

    // Ceiling: a fee rounds toward the party charging it. Capped at `gross` so a misconfigured
    // fee above 100% cannot take more than the profit.
    let protocol = to_u64(mul_div_ceil(
        u128::from(gross),
        u128::from(fee_bps),
        u128::from(BPS),
    )?)?
    .min(gross);
    let net = gross.saturating_sub(protocol);

    // Floor: a payout rounds down. Capped at `net` for the same reason.
    let trader = to_u64(mul_div_floor(
        u128::from(net),
        u128::from(trader_bps),
        u128::from(BPS),
    )?)?
    .min(net);

    // The remainder, not a third division — so the three parts sum to `final_equity` exactly.
    let investor = final_equity.saturating_sub(protocol).saturating_sub(trader);

    Ok(Split {
        investor,
        trader,
        protocol,
        gross_profit: gross,
    })
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::integer_division
)]
mod tests {
    use super::*;

    const USDC: u64 = 1_000_000;

    /// §5.1's worked example, to the cent.
    #[test]
    fn the_worked_example_from_the_plan() {
        let s = split(3_500 * USDC, 4_500 * USDC, 500, 7_000).unwrap();
        assert_eq!(s.gross_profit, 1_000 * USDC);
        assert_eq!(s.protocol, 50 * USDC);
        assert_eq!(s.trader, 665 * USDC);
        assert_eq!(s.investor, 3_785 * USDC);
    }

    #[test]
    fn a_loss_goes_entirely_to_the_investor() {
        let s = split(10_000 * USDC, 9_400 * USDC, 500, 7_000).unwrap();
        assert_eq!(
            s,
            Split {
                investor: 9_400 * USDC,
                trader: 0,
                protocol: 0,
                gross_profit: 0
            }
        );
    }

    #[test]
    fn breaking_even_earns_nobody_anything() {
        let s = split(10_000 * USDC, 10_000 * USDC, 500, 7_000).unwrap();
        assert_eq!((s.trader, s.protocol), (0, 0));
        assert_eq!(s.investor, 10_000 * USDC);
    }

    /// A one-unit profit: the fee's ceiling takes it all, and nothing underflows.
    #[test]
    fn the_smallest_possible_profit_does_not_underflow() {
        let s = split(1_000, 1_001, 500, 7_000).unwrap();
        assert_eq!(s.protocol, 1);
        assert_eq!(s.trader, 0);
        assert_eq!(s.investor, 1_000);
    }

    /// **N4.** Across a grid of principals, outcomes, fees and splits, every unit of the final
    /// balance is assigned to exactly one party, the investor never receives less than they
    /// would with no trader at all when the mandate made money, and no share exceeds its cap.
    #[test]
    fn every_unit_goes_to_exactly_one_party() {
        let principals = [1, 999, 1_000 * USDC, 3_500 * USDC, 123_456_789];
        let fees = [0u16, 1, 500, 2_500, 10_000];
        let splits = [0u16, 1, 7_000, 9_999, 10_000];

        for &principal in &principals {
            for step in 0..=40u64 {
                // From a total wipe-out to roughly tripling, with odd increments so rounding
                // boundaries actually get hit.
                let final_equity = principal / 20 * step + step * 7;
                for &fee in &fees {
                    for &tr in &splits {
                        let s = split(principal, final_equity, fee, tr).unwrap();
                        assert_eq!(
                            s.investor + s.trader + s.protocol,
                            final_equity,
                            "conservation: p={principal} f={final_equity} fee={fee} tr={tr}"
                        );
                        if final_equity > principal {
                            assert!(
                                s.investor >= principal || fee as u64 + tr as u64 > BPS,
                                "a profitable mandate returns the principal: {s:?}"
                            );
                            assert!(s.protocol <= s.gross_profit);
                            assert!(s.trader <= s.gross_profit - s.protocol);
                        } else {
                            assert_eq!((s.trader, s.protocol), (0, 0));
                        }
                    }
                }
            }
        }
    }
}
