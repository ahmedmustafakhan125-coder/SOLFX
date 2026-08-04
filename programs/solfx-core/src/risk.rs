//! Position health: what a position is worth right now, and whether it can still stand.
//!
//! Shared by liquidation, auto-deleveraging, margin withdrawal and trigger execution, so the
//! four cannot disagree about what a position is worth — a disagreement between them is a
//! position that is liquidatable by one measure and not by another.
//!
//! # Two different equities, both correct
//!
//! `ARCHITECTURE.md` gives two formulas that differ, and the difference is deliberate:
//!
//! - **§ 6.5** — `collateral + uPnL − carry − funding − close_fee`. What the trader would
//!   actually walk away with on a *voluntary* close. This is the number to display, and the
//!   one a margin-withdrawal check must use.
//! - **§ 6.8 step 2** — the same, **excluding the close fee**. This is the liquidation test.
//!
//! Excluding it is not a concession to the trader. At liquidation the position pays the
//! *penalty* (`liquidation_fee_bps`), not a close fee — including both would charge twice for
//! the same exit and would liquidate positions that are in fact solvent.

use anchor_lang::prelude::*;
use solfx_math::{funding, margin, pnl, Side, TradeAction};

use crate::errors::{IntoProgramResult, SolfxError};
use crate::oracle::MarketPrice;
use crate::state::{Market, Position};

/// A position marked to the current oracle, with every cost accrued.
#[derive(Debug, Clone, Copy)]
pub struct Health {
    /// The adverse-side price this position would exit at.
    pub mark: i64,
    /// Current notional in USDC.
    pub notional: u64,
    /// Unrealised PnL in USDC, signed.
    pub upnl: i64,
    /// Carry accrued since entry, in USDC. Always a charge.
    pub carry: u64,
    /// Funding accrued since entry, in USDC. Signed — the light side of the book receives.
    pub funding: i64,
    /// Estimated cost of a voluntary close.
    pub close_fee: u64,
    /// § 6.5 equity: what a voluntary close would return. Includes the close fee.
    pub equity: i64,
    /// § 6.8 equity: the liquidation test's input. Excludes the close fee, because the
    /// penalty replaces it.
    pub liquidation_equity: i64,
    /// Maintenance margin, with the § 6.3 size tier applied.
    pub maintenance_margin: u64,
}

impl Health {
    /// The § 6.8 test. Strict: equity exactly equal to the requirement is **not**
    /// liquidatable, because a loose comparison is liquidation griefing (threat T7).
    #[must_use]
    pub fn is_liquidatable(&self) -> bool {
        margin::is_liquidatable(self.liquidation_equity, self.maintenance_margin)
    }

    /// Health factor scaled so `10_000` == 1.0. Presentation only — branch on
    /// [`Self::is_liquidatable`], never on this.
    pub fn health_factor_bps(&self) -> Result<u64> {
        margin::health_factor_bps(self.equity, self.maintenance_margin).or_program_err()
    }
}

/// Mark a position and accrue every cost against it.
///
/// The mark is taken on the **adverse** side — the one the trader would have to cross to get
/// out. Marking at the mid would value every position half a spread better than it can
/// actually be realised, which is a systematic overstatement of solvency across the whole
/// book.
pub fn assess(position: &Position, market: &Market, price: &MarketPrice) -> Result<Health> {
    let side = Side::resolve(position.direction.into(), TradeAction::Close);
    let mark = crate::instructions::trader::open_position::execution_price_for(
        market,
        price,
        position.size_base,
        side,
    )?;

    let conversion = market.quote_conversion();
    let rate = price.quote_conversion_rate.unwrap_or(0);

    let notional_quote = pnl::notional_in_quote(position.size_base, mark).or_program_err()?;
    let notional =
        pnl::convert_cost_to_collateral(notional_quote, conversion, rate).or_program_err()?;

    let upnl_quote = pnl::upnl_in_quote(
        position.size_base,
        position.entry_price,
        mark,
        position.direction.into(),
    )
    .or_program_err()?;
    let upnl = pnl::convert_pnl_to_collateral(upnl_quote, conversion, rate).or_program_err()?;

    let carry = accrued_carry(position, market)?;
    let funding = accrued_funding(position, market)?;

    let close_fee =
        solfx_math::fees::fee_amount(notional, market.close_fee_rate).or_program_err()?;

    let costs = margin::AccruedCosts {
        carry,
        funding,
        close_fee,
    };
    let equity = margin::equity(position.collateral, upnl, costs).or_program_err()?;

    let liquidation_costs = margin::AccruedCosts {
        carry,
        funding,
        close_fee: 0,
    };
    let liquidation_equity =
        margin::equity(position.collateral, upnl, liquidation_costs).or_program_err()?;

    let maintenance_margin =
        margin::maintenance_margin(notional, market.mmr_bps).or_program_err()?;

    Ok(Health {
        mark,
        notional,
        upnl,
        carry,
        funding,
        close_fee,
        equity,
        liquidation_equity,
        maintenance_margin,
    })
}

/// Carry owed since entry, in USDC.
///
/// The basis is the position's **notional at entry**, not its base size: carry is interest on
/// borrowed notional, and pinning it to entry keeps the charge deterministic rather than
/// drifting with the mark.
pub fn accrued_carry(position: &Position, market: &Market) -> Result<u64> {
    funding::accrued_carry(
        position.entry_notional,
        market.cum_borrow_index,
        position.cum_borrow_entry,
    )
    .or_program_err()
}

/// Funding owed since entry, in USDC. Negative means the trader receives.
///
/// The basis is **base size**, matching the units the funding index accrues in.
pub fn accrued_funding(position: &Position, market: &Market) -> Result<i64> {
    let index_now = match position.direction {
        crate::state::Direction::Long => market.cum_funding_long,
        crate::state::Direction::Short => market.cum_funding_short,
    };
    funding::accrued_funding(position.size_base, index_now, position.cum_funding_entry)
        .or_program_err()
}

/// Settle accrued funding into a position's collateral, and into the market's funding pool.
///
/// # Why funding never leaves the collateral vault
///
/// Funding is a transfer **between traders** (§ 6.7). Both sides' money is already in the
/// collateral vault, so settling it moves no tokens — it only reallocates between
/// `Position::collateral` and `Market::funding_balance`. That is what keeps invariant I3
/// true by construction: funding cannot touch LP capital because it never goes near the LP
/// vault.
///
/// `Market::funding_balance` holds what payers have paid but receivers have not yet claimed.
/// Invariant I1 counts it, because it is still user money — just not yet attributed to a
/// particular user.
///
/// # Why a receipt is capped at the pool
///
/// `funding_index_update` floors the receiving side's rate so payouts can never exceed
/// collections *at crank time*. Over an interval that guarantee weakens: a position that
/// existed for only part of the interval still accrues the whole index delta, which is the
/// standard cumulative-index approximation. Capping the payout at the balance actually held
/// closes the gap for good — a receiver is paid what exists, never more.
///
/// Returns the signed amount applied to the position's collateral.
pub fn settle_funding(position: &mut Position, market: &mut Market) -> Result<i64> {
    let owed = accrued_funding(position, market)?;

    let applied = if owed > 0 {
        // The trader pays. Cap at their collateral: a position cannot pay funding it does
        // not have, and the shortfall is a liquidation problem, not a funding problem.
        let payable =
            owed.min(i64::try_from(position.collateral).map_err(|_| SolfxError::MathOverflow)?);
        position.collateral = position
            .collateral
            .checked_sub(u64::try_from(payable).map_err(|_| SolfxError::MathOverflow)?)
            .ok_or(SolfxError::MathOverflow)?;
        market.funding_balance = market
            .funding_balance
            .checked_add(u64::try_from(payable).map_err(|_| SolfxError::MathOverflow)?)
            .ok_or(SolfxError::MathOverflow)?;
        -payable
    } else if owed < 0 {
        // The trader receives, from what payers have actually paid in.
        let wanted = u64::try_from(-i128::from(owed)).map_err(|_| SolfxError::MathOverflow)?;
        let paid = wanted.min(market.funding_balance);
        position.collateral = position
            .collateral
            .checked_add(paid)
            .ok_or(SolfxError::MathOverflow)?;
        market.funding_balance = market
            .funding_balance
            .checked_sub(paid)
            .ok_or(SolfxError::MathOverflow)?;
        i64::try_from(paid).map_err(|_| SolfxError::MathOverflow)?
    } else {
        0
    };

    // Re-snapshot so the same accrual cannot be charged twice.
    position.cum_funding_entry = match position.direction {
        crate::state::Direction::Long => market.cum_funding_long,
        crate::state::Direction::Short => market.cum_funding_short,
    };
    position.realized_funding = position.realized_funding.saturating_add(applied);

    Ok(applied)
}

/// Charge accrued carry to a position's collateral and re-snapshot the index.
///
/// Unlike funding, carry **does** leave trader ownership: it is revenue, routed through the
/// same four-way split as a trading fee (§ 8.1 stream 2). The caller passes the returned
/// amount to `trader::settle` as a fee.
///
/// The charge is capped at the position's collateral for the same reason funding is — a
/// position that cannot pay its carry is a liquidation, and pretending otherwise would drive
/// `collateral` negative.
pub fn settle_carry(position: &mut Position, market: &Market) -> Result<u64> {
    let owed = accrued_carry(position, market)?;
    let payable = owed.min(position.collateral);

    position.collateral = position
        .collateral
        .checked_sub(payable)
        .ok_or(SolfxError::MathOverflow)?;
    position.cum_borrow_entry = market.cum_borrow_index;

    Ok(payable)
}
