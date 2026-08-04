use anchor_lang::prelude::*;
use solfx_math::margin;

use crate::errors::{IntoProgramResult, SolfxError};
use crate::events::PositionCollateralChanged;
use crate::oracle::load_validated_price;
use crate::risk;

use super::close_position::DecreasePosition;

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

    let remaining = ctx
        .accounts
        .position
        .collateral
        .checked_sub(amount)
        .ok_or(SolfxError::InsufficientCollateral)?;
    require!(remaining > 0, SolfxError::PositionNotEmpty);

    // Costs are **assessed, not settled**. `risk::assess` already nets accrued carry and
    // funding into equity, so the check sees the position's true worth — and settling here
    // would be wrong twice over: carry is revenue that needs the four-vault routing this
    // instruction does not carry, and a margin *query* should not move money.
    let health = risk::assess(&ctx.accounts.position, &ctx.accounts.market, &price)?;

    // Withdrawing margin from an underwater position is extracting value the pool is about
    // to be owed.
    require!(!health.is_liquidatable(), SolfxError::PositionLiquidatable);

    // The position must still meet **initial** margin afterwards, not merely maintenance: a
    // trader must not be able to walk a position to the edge of liquidation and leave it
    // there. That is the same standard it had to meet to exist.
    //
    // `health.equity` is computed on the current collateral, so the withdrawal is applied to
    // it directly rather than reassessing — the mark, PnL and accrued costs are unchanged by
    // moving margin.
    let equity_after = i128::from(health.equity)
        .checked_sub(i128::from(amount))
        .ok_or(SolfxError::MathOverflow)?;
    let imr =
        margin::initial_margin(health.notional, ctx.accounts.market.imr_bps).or_program_err()?;
    require!(
        equity_after >= i128::from(imr),
        SolfxError::WouldBreachMaintenanceMargin
    );

    let leverage = margin::effective_leverage(health.notional, remaining).or_program_err()?;
    require!(
        leverage <= u64::from(ctx.accounts.market.effective_max_leverage()),
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
