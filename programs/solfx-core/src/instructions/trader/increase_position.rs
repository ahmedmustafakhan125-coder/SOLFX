use anchor_lang::prelude::*;
use solfx_math::{fees, margin, pnl, pricing, Side, TradeAction};

use crate::errors::{IntoProgramResult, SolfxError};
use crate::events::PositionIncreased;
use crate::oracle::load_validated_price;

use super::close_position::DecreasePosition;
use super::flows::compute_flows;
use super::open_position::execution_price_for;
use super::{settle, SettlementInput, VaultTransfer};

/// Add size (and optionally margin) to an existing position.
///
/// Reuses [`DecreasePosition`]'s account set: an increase touches exactly the same accounts
/// as a decrease — the position survives either way, and both settle a fee through the four
/// vaults. Only the direction of the size change differs.
///
/// # The weighted entry price is rounded against the trader
///
/// `solfx_math::pnl::weighted_entry_price` rounds a long's average entry **up** and a short's
/// **down**. Both directions mean a slightly worse entry, so a trader cannot manufacture a
/// better average by topping up in small increments — which would otherwise be a free-money
/// edge repeated on every add.
pub fn increase_position(
    ctx: Context<DecreasePosition>,
    size_delta: u64,
    collateral_delta: u64,
    price_limit: i64,
) -> Result<()> {
    let clock = Clock::get()?;

    require!(!ctx.accounts.protocol.paused, SolfxError::ProtocolPaused);
    require!(
        ctx.accounts.market.status.allows_open(),
        SolfxError::MarketClosedForOpens
    );
    require!(size_delta > 0, SolfxError::ZeroAmount);
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

    let direction = ctx.accounts.position.direction;
    let side = Side::resolve(direction.into(), TradeAction::Open);
    let exec_price = execution_price_for(&ctx.accounts.market, &price, size_delta, side)?;
    pricing::validate_slippage(exec_price, price_limit, side)
        .map_err(|_| error!(SolfxError::SlippageExceeded))?;

    let conversion_rate = price.quote_conversion_rate.unwrap_or(0);
    let conversion = ctx.accounts.market.quote_conversion();

    // Notional of the *added* slice, for the fee and for open interest.
    let added_quote = pnl::notional_in_quote(size_delta, exec_price).or_program_err()?;
    let added_notional = pnl::convert_cost_to_collateral(added_quote, conversion, conversion_rate)
        .or_program_err()?;
    pricing::validate_notional(added_notional).or_program_err()?;

    let new_size = ctx
        .accounts
        .position
        .size_base
        .checked_add(size_delta)
        .ok_or(SolfxError::MathOverflow)?;
    require!(
        new_size <= ctx.accounts.market.max_position_size,
        SolfxError::PositionSizeOutOfBounds
    );

    let new_entry = pnl::weighted_entry_price(
        ctx.accounts.position.size_base,
        ctx.accounts.position.entry_price,
        size_delta,
        exec_price,
        direction.into(),
    )
    .or_program_err()?;

    let new_collateral = ctx
        .accounts
        .position
        .collateral
        .checked_add(collateral_delta)
        .ok_or(SolfxError::MathOverflow)?;
    let new_notional = ctx
        .accounts
        .position
        .entry_notional
        .checked_add(added_notional)
        .ok_or(SolfxError::MathOverflow)?;

    // The *whole* position must satisfy the margin rules afterwards, not just the addition.
    // Checking only the increment would let a trader ratchet past the leverage cap one small
    // top-up at a time.
    let leverage = margin::effective_leverage(new_notional, new_collateral).or_program_err()?;
    require!(
        leverage <= u64::from(ctx.accounts.market.effective_max_leverage()),
        SolfxError::LeverageTooHigh
    );
    let required =
        margin::initial_margin(new_notional, ctx.accounts.market.imr_bps).or_program_err()?;
    require!(new_collateral >= required, SolfxError::InsufficientMargin);

    let tier_rate = fees::fee_rate_for_volume(ctx.accounts.user_account.thirty_day_volume);
    let fee = fees::fee_amount(
        added_notional,
        ctx.accounts.market.open_fee_rate.min(tier_rate),
    )
    .or_program_err()?;

    let debit = collateral_delta
        .checked_add(fee)
        .ok_or(SolfxError::MathOverflow)?;
    ctx.accounts.user_account.debit(debit)?;

    let (alloc, flows) = compute_flows(fee, 0, ctx.accounts.protocol.fee_split())?;

    let transfer = VaultTransfer {
        token_program: ctx.accounts.token_program.to_account_info(),
        collateral_vault: ctx.accounts.collateral_vault.to_account_info(),
        lp_vault: ctx.accounts.lp_vault.to_account_info(),
        insurance_vault: ctx.accounts.insurance_vault.to_account_info(),
        fee_vault: ctx.accounts.fee_vault.to_account_info(),
        protocol: ctx.accounts.protocol.to_account_info(),
        protocol_bump: ctx.accounts.protocol.bump,
        lp_pool: ctx.accounts.lp_pool.to_account_info(),
        lp_pool_bump: ctx.accounts.lp_pool.bump,
    };
    let lp_balance = ctx.accounts.lp_vault.amount;
    let user_key = ctx.accounts.user_account.key();
    let referrer = ctx.accounts.position.referrer;
    let market_index = ctx.accounts.market.market_index;

    settle(
        SettlementInput {
            fee,
            pnl: 0,
            market_index,
            user_account: user_key,
            referrer,
        },
        flows,
        alloc,
        &transfer,
        &mut ctx.accounts.protocol,
        &mut ctx.accounts.lp_pool,
        &mut ctx.accounts.insurance_fund,
        &mut ctx.accounts.market,
        lp_balance,
        clock.unix_timestamp,
    )?;

    let market = &mut ctx.accounts.market;
    market.add_open_interest(direction, added_notional, size_delta)?;
    market.check_oi_cap(direction)?;

    let position = &mut ctx.accounts.position;
    position.size_base = new_size;
    position.entry_price = new_entry;
    position.collateral = new_collateral;
    position.entry_notional = new_notional;
    position.open_fee_paid = position.open_fee_paid.saturating_add(fee);
    position.last_updated_at = clock.unix_timestamp;

    let user = &mut ctx.accounts.user_account;
    user.thirty_day_volume = user.thirty_day_volume.saturating_add(added_notional);
    // Record the referral-pool contribution alongside volume. The share is zero when the
    // trader has no referrer, matching what `settle` earmarked (§ 8.3).
    let referral_share = if user.referrer == Pubkey::default() {
        0
    } else {
        alloc.referral
    };
    user.record_trade(added_notional, referral_share)?;

    emit!(PositionIncreased {
        position: position.key(),
        market_index,
        size_added: size_delta,
        collateral_added: collateral_delta,
        exec_price,
        new_size_base: new_size,
        new_entry_price: new_entry,
        fee,
        ts: clock.unix_timestamp,
    });

    Ok(())
}
