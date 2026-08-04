use anchor_lang::prelude::*;
use solfx_math::{margin, pnl, Side, TradeAction};

use crate::errors::{IntoProgramResult, SolfxError};
use crate::events::PositionCollateralChanged;
use crate::oracle::load_validated_price;

use super::close_position::DecreasePosition;
use super::open_position::execution_price_for;

/// Move USDC from free collateral into a position's isolated margin.
///
/// Purely a reclassification — no tokens cross the vault boundary, so `total_user_collateral`
/// is untouched and invariant I1 holds trivially.
///
/// Permitted while the protocol is paused, for the same reason deposits are: adding margin
/// reduces risk, and blocking it during an incident would push positions toward liquidation
/// at exactly the wrong moment.
pub fn add_position_collateral(ctx: Context<DecreasePosition>, amount: u64) -> Result<()> {
    require!(amount > 0, SolfxError::ZeroAmount);
    require!(
        ctx.accounts.position.size_base > 0,
        SolfxError::PositionNotEmpty
    );

    ctx.accounts.user_account.debit(amount)?;

    let position = &mut ctx.accounts.position;
    position.collateral = position
        .collateral
        .checked_add(amount)
        .ok_or(SolfxError::MathOverflow)?;
    position.last_updated_at = Clock::get()?.unix_timestamp;

    emit!(PositionCollateralChanged {
        position: position.key(),
        market_index: position.market_index,
        delta: i64::try_from(amount).map_err(|_| SolfxError::MathOverflow)?,
        collateral_after: position.collateral,
        ts: position.last_updated_at,
    });
    Ok(())
}

/// Move USDC out of a position's margin and back into free collateral.
///
/// # Why this one needs a price and the other does not
///
/// Removing margin raises leverage, so the position has to be re-checked against the market's
/// rules afterwards — and both checks need a current mark:
///
/// * **Initial margin**, not maintenance. A trader must not be able to walk a position down
///   to the edge of liquidation and leave it there; the position has to remain openable at
///   its new margin, which is the same standard it had to meet to exist.
/// * **Not already liquidatable.** Withdrawing margin from an underwater position is
///   extracting value the pool is about to be owed.
///
/// Mark-to-market uses the **adverse** side, exactly as a close would. Valuing the position
/// at the mid here would let a trader withdraw against half a spread they have not yet paid.
pub fn remove_position_collateral(ctx: Context<DecreasePosition>, amount: u64) -> Result<()> {
    let clock = Clock::get()?;
    require!(amount > 0, SolfxError::ZeroAmount);
    require!(
        ctx.accounts.position.size_base > 0,
        SolfxError::PositionNotEmpty
    );

    let price = load_validated_price(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
        ctx.accounts
            .quote_conversion_price_update
            .as_deref()
            .map(|a| &**a),
        &clock,
    )?;

    let position = &ctx.accounts.position;
    let market = &ctx.accounts.market;

    let remaining = position
        .collateral
        .checked_sub(amount)
        .ok_or(SolfxError::InsufficientCollateral)?;
    require!(remaining > 0, SolfxError::PositionNotEmpty);

    // Mark on the side the trader would have to cross to get out.
    let side = Side::resolve(position.direction.into(), TradeAction::Close);
    let mark = execution_price_for(market, &price, position.size_base, side)?;
    let conversion_rate = price.quote_conversion_rate.unwrap_or(0);
    let conversion = market.quote_conversion();

    let notional_quote = pnl::notional_in_quote(position.size_base, mark).or_program_err()?;
    let notional = pnl::convert_cost_to_collateral(notional_quote, conversion, conversion_rate)
        .or_program_err()?;

    let upnl_quote = pnl::upnl_in_quote(
        position.size_base,
        position.entry_price,
        mark,
        position.direction.into(),
    )
    .or_program_err()?;
    let upnl =
        pnl::convert_pnl_to_collateral(upnl_quote, conversion, conversion_rate).or_program_err()?;

    // Costs are zero until Phase 4 wires funding and carry; the shape is already correct so
    // that adding them is a change of inputs rather than of logic.
    let costs = margin::AccruedCosts::default();

    let equity_now = margin::equity(position.collateral, upnl, costs).or_program_err()?;
    let mm = margin::maintenance_margin(notional, market.mmr_bps).or_program_err()?;
    require!(
        !margin::is_liquidatable(equity_now, mm),
        SolfxError::PositionLiquidatable
    );

    let equity_after = margin::equity(remaining, upnl, costs).or_program_err()?;
    let imr = margin::initial_margin(notional, market.imr_bps).or_program_err()?;
    require!(
        equity_after >= i64::try_from(imr).map_err(|_| SolfxError::MathOverflow)?,
        SolfxError::WouldBreachMaintenanceMargin
    );

    let leverage = margin::effective_leverage(notional, remaining).or_program_err()?;
    require!(
        leverage <= u64::from(market.effective_max_leverage()),
        SolfxError::LeverageTooHigh
    );

    let position = &mut ctx.accounts.position;
    position.collateral = remaining;
    position.last_updated_at = clock.unix_timestamp;
    let position_key = position.key();
    let market_index = position.market_index;

    ctx.accounts.user_account.credit(amount)?;

    emit!(PositionCollateralChanged {
        position: position_key,
        market_index,
        delta: -i64::try_from(amount).map_err(|_| SolfxError::MathOverflow)?,
        collateral_after: remaining,
        ts: clock.unix_timestamp,
    });
    Ok(())
}
