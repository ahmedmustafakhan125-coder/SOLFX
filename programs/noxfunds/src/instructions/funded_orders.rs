//! Orders on a funded mandate: a stop that can be tightened, and resting entries.
//!
//! # A position is never without its stop, even inside one instruction
//!
//! `funded_move_stop` places the new stop **before** it cancels the old one, in the same
//! instruction. At no point between the two is the position unprotected, and if the placement is
//! refused the cancel never happens. That is the property `funded_open_position` was built around
//! (the position and its stop land together), extended to the stop's whole life.
//!
//! # Every rule is checked at the fill, by the code a market order runs
//!
//! `funded_fill_entry_order` calls the same `check_rules` and makes the same two CPIs
//! `funded_open_position` makes — `open_position`, then `place_trigger_order` for the stop — so a
//! resting order that triggers is judged by every rule of the mandate at the price of the moment.
//! The trader's price bound travels with the order and is enforced by `solfx-core`'s own
//! slippage check, so a limit cannot fill worse than its limit.
//!
//! # What no order may be used for
//!
//! Leaving inside the minimum hold. A stop-out is exempt from `min_hold_slots` because it fires
//! inside `solfx-core`, which never consults the mandate. That exemption only ever releases a
//! *loss* — an initial stop sits on the losing side — so it gives a scalper nothing. A stop moved
//! into profit would, so a stop moves only once the hold has passed; and `funded_place_take_profit`
//! applies the same gate to targets.

use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_core::state::{Direction, Market, Position, TriggerKind, TriggerOrder};

use crate::constants::{BPS, CONFIG_SEED, MANDATE_ORDER_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED};
use crate::errors::NoxError;
use crate::events::{
    FundedStopMoved, FundedTradeOpened, MandateEntryOrderCancelled, MandateEntryOrderFilled,
    MandateEntryOrderPlaced,
};
use crate::instructions::eval_orders::is_tighter;
use crate::instructions::trading::{check_rules, leg};
use crate::state::{EntryKind, Mandate, MandateEntryOrder, MandateState, NoxConfig};
use crate::venue::read_user_account;
use crate::SolfxCore;

/// Has `position` been held for the mandate's minimum, in slots — the unit the rule is set in?
fn held_long_enough(mandate: &Mandate, position: &Position) -> Result<bool> {
    let held = Clock::get()?.slot.saturating_sub(position.opened_at_slot);
    Ok(held >= mandate.min_hold_slots)
}

// --- moving the stop ----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct FundedMoveStop<'info> {
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
        constraint = mandate.trader == trader.key() @ NoxError::NotTheTrader,
        constraint = mandate.state != MandateState::Settled @ NoxError::MandateNotActive,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// The SolFX authority. Pays the new stop's rent and is refunded the old one's.
    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: `solfx-core` derives and checks it.
    pub protocol: UncheckedAccount<'info>,

    /// CHECK: bound to this mandate, and checked by `solfx-core` as the position's account with
    /// the mandate signer as its authority.
    #[account(address = mandate.solfx_user_account)]
    pub user_account: UncheckedAccount<'info>,

    /// CHECK: `solfx-core` checks its seeds and that the position belongs to it.
    pub market: UncheckedAccount<'info>,

    /// Deserialized for its direction, size and opening slot. `Account<Position>` checks it is
    /// owned by `solfx-core`; `solfx-core` checks it belongs to `user_account`.
    pub position: Box<Account<'info, Position>>,

    /// The stop being replaced. Deserialized so its kind, its position and the position's
    /// opening slot can be checked; `solfx-core` owns it, so Anchor never writes it back, and its
    /// close in the cancel CPI is not disturbed.
    #[account(
        mut,
        constraint = old_stop.position == position.key() @ NoxError::NotTheStopLoss,
        constraint = old_stop.position_opened_at_slot == position.opened_at_slot
            @ NoxError::NotTheStopLoss,
        constraint = old_stop.kind == TriggerKind::StopLoss @ NoxError::NotTheStopLoss,
    )]
    pub old_stop: Box<Account<'info, TriggerOrder>>,

    /// CHECK: initialized by `solfx-core` at `[TRIGGER_SEED, position, new_order_id]` and paid
    /// for by the mandate signer. An id already in use fails there as "account already in use".
    #[account(mut)]
    pub new_stop: UncheckedAccount<'info>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub system_program: Program<'info, System>,

    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

/// Tighten a funded stop: place the new one, then cancel the old, in one instruction.
///
/// Toward the price only, so the risk the trade was approved on can only fall — no mandate rule
/// can be broken by it, which is why none is re-priced. Only after `min_hold_slots`, so a stop
/// dragged into profit cannot be used to leave inside the hold. Not gated on the pause flag: a
/// tighter stop is less risk, and a pause exists to stop new risk.
pub fn funded_move_stop(
    ctx: Context<FundedMoveStop>,
    new_order_id: u8,
    new_stop: i64,
) -> Result<()> {
    let pos = &ctx.accounts.position;
    require!(
        held_long_enough(&ctx.accounts.mandate, pos)?,
        NoxError::MinimumHoldNotMet
    );
    require!(new_stop > 0, NoxError::StopLossRequired);
    let old = &ctx.accounts.old_stop;
    require!(
        is_tighter(pos.direction, old.trigger_price, new_stop),
        NoxError::StopNotTighter
    );
    let old_stop = old.trigger_price;
    let old_order_id = old.order_id;
    let size_base = pos.size_base;

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    // Place first. `solfx-core` refuses a stop that is already met at the current price, with
    // its own `TriggerAlreadyMet`, so the replacement is a real stop before the old one goes.
    move_place_stop(&ctx, seeds, new_order_id, new_stop, size_base)?;
    move_cancel_old(&ctx, seeds)?;

    emit!(FundedStopMoved {
        mandate: mandate_key,
        position: ctx.accounts.position.key(),
        old_order_id,
        new_order_id,
        old_stop,
        new_stop,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[inline(never)]
fn move_place_stop(
    ctx: &Context<FundedMoveStop>,
    seeds: &[&[&[u8]]],
    order_id: u8,
    stop: i64,
    size_base: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    solfx_core::cpi::place_trigger_order(
        CpiContext::new_with_signer(
            a.solfx_core_program.key(),
            solfx_core::cpi::accounts::PlaceTriggerOrder {
                authority: a.mandate_signer.to_account_info(),
                protocol: a.protocol.to_account_info(),
                user_account: a.user_account.to_account_info(),
                market: a.market.to_account_info(),
                position: a.position.to_account_info(),
                trigger_order: a.new_stop.to_account_info(),
                price_update: a.price_update.to_account_info(),
                secondary_price_update: Some(leg(&a.secondary_price_update, &a.solfx_core_program)),
                quote_conversion_price_update: Some(leg(
                    &a.quote_conversion_price_update,
                    &a.solfx_core_program,
                )),
                system_program: a.system_program.to_account_info(),
            },
            seeds,
        ),
        order_id,
        TriggerKind::StopLoss,
        stop,
        size_base,
    )
}

#[inline(never)]
fn move_cancel_old(ctx: &Context<FundedMoveStop>, seeds: &[&[&[u8]]]) -> Result<()> {
    let a = &ctx.accounts;
    solfx_core::cpi::cancel_trigger_order(CpiContext::new_with_signer(
        a.solfx_core_program.key(),
        solfx_core::cpi::accounts::CancelTriggerOrder {
            authority: a.mandate_signer.to_account_info(),
            trigger_order: a.old_stop.to_account_info(),
        },
        seeds,
    ))
}

// --- resting entry orders -----------------------------------------------------------------------

/// What a funded entry order asks for. One struct, so a stop and a limit cannot be swapped at a
/// call site.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MandateEntryParams {
    pub market_index: u16,
    /// The SolFX position nonce it opens at.
    pub nonce: u8,
    /// The trigger order id the stop takes when it fills.
    pub stop_order_id: u8,
    pub direction: Direction,
    pub kind: EntryKind,
    pub trigger_price: i64,
    /// The fill bound handed to `open_position` — a maximum for a buy, a minimum for a sell.
    /// A limit's bound may be no worse than its trigger; a stop's no better.
    pub price_limit: i64,
    pub size_base: u64,
    pub collateral: u64,
    pub stop_loss_price: i64,
    /// Zero: good until cancelled.
    pub expires_at: i64,
}

/// Does the fill bound sit where this kind of order needs it?
///
/// A buy **limit** never pays more than its limit, so its bound is at or under the trigger; a
/// buy **stop** buys a breakout, so its bound is at or above the trigger. Sells mirror both.
#[must_use]
pub fn price_limit_fits(kind: EntryKind, direction: Direction, trigger: i64, limit: i64) -> bool {
    if limit <= 0 {
        return false;
    }
    match (kind, direction) {
        (EntryKind::Limit, Direction::Long) | (EntryKind::Stop, Direction::Short) => {
            limit <= trigger
        }
        (EntryKind::Limit, Direction::Short) | (EntryKind::Stop, Direction::Long) => {
            limit >= trigger
        }
    }
}

#[derive(Accounts)]
#[instruction(order_id: u8)]
pub struct FundedPlaceEntryOrder<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
        constraint = mandate.trader == trader.key() @ NoxError::NotTheTrader,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    #[account(
        init,
        payer = trader,
        space = 8 + MandateEntryOrder::INIT_SPACE,
        seeds = [MANDATE_ORDER_SEED, mandate.key().as_ref(), &[order_id]],
        bump,
    )]
    pub entry_order: Box<Account<'info, MandateEntryOrder>>,

    pub system_program: Program<'info, System>,
}

/// Rest an entry order on a funded mandate.
///
/// What can be judged without a price is judged now: the market is permitted, the bracket and
/// the bound are the right way round, and the stop is within the mandate's distance limit when
/// measured from the trigger. Everything is judged again by `check_rules` at the fill.
pub fn funded_place_entry_order(
    ctx: Context<FundedPlaceEntryOrder>,
    order_id: u8,
    params: MandateEntryParams,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    let now = Clock::get()?.unix_timestamp;
    let m = &ctx.accounts.mandate;
    let o = params;

    require!(m.state == MandateState::Active, NoxError::MandateNotActive);
    require!(
        m.permits_market(o.market_index),
        NoxError::MarketNotPermitted
    );
    require!(o.size_base > 0 && o.collateral > 0, NoxError::ZeroAmount);
    require!(
        o.expires_at == 0 || o.expires_at > now,
        NoxError::InvalidExpiry
    );
    require!(o.trigger_price > 0, NoxError::EntryNotTriggered);
    require!(o.stop_loss_price > 0, NoxError::StopLossRequired);
    require!(
        TriggerKind::StopLoss.is_placeable(o.direction, o.stop_loss_price, o.trigger_price),
        NoxError::StopOnWrongSide
    );
    require!(
        price_limit_fits(o.kind, o.direction, o.trigger_price, o.price_limit),
        NoxError::InvalidPriceLimit
    );
    // The stop's distance from the trigger, rounded up as the fill will measure it from spot.
    let distance_bps = solfx_math::fixed::mul_div_ceil(
        u128::from(o.trigger_price.abs_diff(o.stop_loss_price)),
        u128::from(BPS),
        u128::try_from(o.trigger_price).map_err(|_| NoxError::MathOverflow)?,
    )
    .map_err(NoxError::from)?;
    require!(
        distance_bps <= u128::from(m.max_stop_distance_bps),
        NoxError::StopTooFar
    );

    let mandate_key = m.key();
    let order = &mut ctx.accounts.entry_order;
    order.mandate = mandate_key;
    order.trader = ctx.accounts.trader.key();
    order.order_id = order_id;
    order.market_index = o.market_index;
    order.nonce = o.nonce;
    order.stop_order_id = o.stop_order_id;
    order.direction = o.direction;
    order.kind = o.kind;
    order.trigger_price = o.trigger_price;
    order.price_limit = o.price_limit;
    order.size_base = o.size_base;
    order.collateral = o.collateral;
    order.stop_loss_price = o.stop_loss_price;
    order.expires_at = o.expires_at;
    order.created_at = now;
    order.bump = ctx.bumps.entry_order;

    emit!(MandateEntryOrderPlaced {
        mandate: mandate_key,
        order: order.key(),
        order_id,
        market_index: o.market_index,
        nonce: o.nonce,
        direction: o.direction as u8,
        kind: o.kind as u8,
        trigger_price: o.trigger_price,
        price_limit: o.price_limit,
        size_base: o.size_base,
        collateral: o.collateral,
        stop_loss_price: o.stop_loss_price,
        expires_at: o.expires_at,
        ts: now,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct FundedCancelEntryOrder<'info> {
    /// The trader at any time; anyone once the order can no longer fill.
    pub caller: Signer<'info>,

    /// CHECK: receives the order's rent. Constrained to the mandate's trader, who paid it.
    #[account(mut, address = mandate.trader @ NoxError::NotTheTrader)]
    pub trader: UncheckedAccount<'info>,

    #[account(
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    #[account(
        mut,
        close = trader,
        seeds = [MANDATE_ORDER_SEED, mandate.key().as_ref(), &[entry_order.order_id]],
        bump = entry_order.bump,
        has_one = mandate @ NoxError::OrderNotOnMandate,
    )]
    pub entry_order: Box<Account<'info, MandateEntryOrder>>,
}

/// Cancel a funded entry order; its rent goes back to the trader. Anyone may once it can no
/// longer fill — expired, or the mandate no longer `Active` — so a dead order is never stranded.
pub fn funded_cancel_entry_order(ctx: Context<FundedCancelEntryOrder>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let o = &ctx.accounts.entry_order;
    let lapsed = (o.expires_at != 0 && now > o.expires_at)
        || ctx.accounts.mandate.state != MandateState::Active;
    require!(
        ctx.accounts.caller.key() == ctx.accounts.mandate.trader || lapsed,
        NoxError::NotYourOrderToCancel
    );
    emit!(MandateEntryOrderCancelled {
        mandate: ctx.accounts.mandate.key(),
        order: o.key(),
        by: ctx.accounts.caller.key(),
        expired: lapsed,
        ts: now,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct FundedFillEntryOrder<'info> {
    /// Anyone. Pays the transaction fee and nothing else: the position's and the stop's rent come
    /// from the mandate signer, exactly as on a market open.
    pub keeper: Signer<'info>,

    /// CHECK: receives the filled order's rent. Constrained to the mandate's trader, who paid it.
    #[account(mut, address = mandate.trader @ NoxError::NotTheTrader)]
    pub trader: UncheckedAccount<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    #[account(
        mut,
        close = trader,
        seeds = [MANDATE_ORDER_SEED, mandate.key().as_ref(), &[entry_order.order_id]],
        bump = entry_order.bump,
        has_one = mandate @ NoxError::OrderNotOnMandate,
    )]
    pub entry_order: Box<Account<'info, MandateEntryOrder>>,

    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    // --- passed through; `solfx-core` re-validates each by its own seeds and constraints ---
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub protocol: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`, and bound to this mandate here.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: UncheckedAccount<'info>,
    /// Deserialized: the rules are priced against it before the CPI.
    #[account(
        mut,
        constraint = market.market_index == entry_order.market_index @ NoxError::MarketNotPermitted,
    )]
    pub market: Box<Account<'info, Market>>,
    /// CHECK: created by the open CPI at the order's nonce, read by the stop CPI.
    #[account(mut)]
    pub position: UncheckedAccount<'info>,
    /// CHECK: created by the stop CPI at the order's `stop_order_id`.
    #[account(mut)]
    pub trigger_order: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub collateral_vault: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub lp_pool: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub lp_vault: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub insurance_fund: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub insurance_vault: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub fee_vault: UncheckedAccount<'info>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

/// Fill a funded entry order whose trigger the oracle has reached. **Permissionless.**
///
/// The order is judged by `check_rules` at this moment and filled by the same two CPIs a market
/// open makes, bounded by the trader's own price limit through `solfx-core`'s slippage check.
pub fn funded_fill_entry_order(ctx: Context<FundedFillEntryOrder>) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    let clock = Clock::get()?;
    let o = (**ctx.accounts.entry_order).clone();
    require!(
        o.expires_at == 0 || clock.unix_timestamp <= o.expires_at,
        NoxError::EntryOrderExpired
    );

    let price = solfx_core::oracle::load_validated_price(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
        ctx.accounts
            .quote_conversion_price_update
            .as_deref()
            .map(|a| &**a),
        &clock,
    )?;
    require!(
        o.kind
            .is_met(o.direction, o.trigger_price, price.spot.price),
        NoxError::EntryNotTriggered
    );
    let margins = check_rules(
        &ctx.accounts.mandate,
        &ctx.accounts.market,
        &price,
        o.market_index,
        o.direction,
        o.size_base,
        o.stop_loss_price,
    )?;

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    let free_before = read_user_account(&ctx.accounts.user_account)?
        .ok_or(NoxError::NotTheTrader)?
        .free_collateral;
    fill_cpi_open(&ctx, seeds, &o)?;
    let free_after = read_user_account(&ctx.accounts.user_account)?
        .ok_or(NoxError::NotTheTrader)?
        .free_collateral;
    let debit = free_before
        .checked_sub(free_after)
        .ok_or(NoxError::MathOverflow)?;
    fill_cpi_place_stop(&ctx, seeds, &o)?;

    let m = &mut ctx.accounts.mandate;
    m.book(o.market_index, o.nonce, margins.notional)?;
    m.booked_margin_fees = m
        .booked_margin_fees
        .checked_add(debit)
        .ok_or(NoxError::MathOverflow)?;
    m.last_free_collateral = m
        .last_free_collateral
        .checked_sub(i64::try_from(debit).map_err(|_| NoxError::MathOverflow)?)
        .ok_or(NoxError::MathOverflow)?;

    emit!(FundedTradeOpened {
        mandate: mandate_key,
        trader: m.trader,
        market_index: o.market_index,
        nonce: o.nonce,
        direction: o.direction as u8,
        size_base: o.size_base,
        notional: margins.notional,
        collateral: o.collateral,
        stop_loss_price: o.stop_loss_price,
        notional_used_bps: margins.notional_used_bps,
        risk_used_bps: margins.risk_used_bps,
        open_positions: m.open_positions,
        ts: clock.unix_timestamp,
    });
    emit!(MandateEntryOrderFilled {
        mandate: mandate_key,
        order: ctx.accounts.entry_order.key(),
        market_index: o.market_index,
        nonce: o.nonce,
        oracle_price: price.spot.price,
        keeper: ctx.accounts.keeper.key(),
        ts: clock.unix_timestamp,
    });
    Ok(())
}

/// The open CPI, in its own frame — the same split `funded_open_position` makes, for the same
/// 4,096-byte reason. Written for this context rather than shared, because sharing would mean
/// building the 16-account `CpiContext` in the caller's frame.
#[inline(never)]
fn fill_cpi_open(
    ctx: &Context<FundedFillEntryOrder>,
    seeds: &[&[&[u8]]],
    o: &MandateEntryOrder,
) -> Result<()> {
    let a = &ctx.accounts;
    solfx_core::cpi::open_position(
        CpiContext::new_with_signer(
            a.solfx_core_program.key(),
            solfx_core::cpi::accounts::OpenPosition {
                authority: a.mandate_signer.to_account_info(),
                protocol: a.protocol.to_account_info(),
                user_account: a.user_account.to_account_info(),
                market: a.market.to_account_info(),
                position: a.position.to_account_info(),
                collateral_vault: a.collateral_vault.to_account_info(),
                lp_pool: a.lp_pool.to_account_info(),
                lp_vault: a.lp_vault.to_account_info(),
                insurance_fund: a.insurance_fund.to_account_info(),
                insurance_vault: a.insurance_vault.to_account_info(),
                fee_vault: a.fee_vault.to_account_info(),
                price_update: a.price_update.to_account_info(),
                secondary_price_update: Some(leg(&a.secondary_price_update, &a.solfx_core_program)),
                quote_conversion_price_update: Some(leg(
                    &a.quote_conversion_price_update,
                    &a.solfx_core_program,
                )),
                token_program: a.token_program.to_account_info(),
                system_program: a.system_program.to_account_info(),
            },
            seeds,
        ),
        o.market_index,
        o.nonce,
        o.direction,
        o.size_base,
        o.collateral,
        o.price_limit,
    )
}

#[inline(never)]
fn fill_cpi_place_stop(
    ctx: &Context<FundedFillEntryOrder>,
    seeds: &[&[&[u8]]],
    o: &MandateEntryOrder,
) -> Result<()> {
    let a = &ctx.accounts;
    solfx_core::cpi::place_trigger_order(
        CpiContext::new_with_signer(
            a.solfx_core_program.key(),
            solfx_core::cpi::accounts::PlaceTriggerOrder {
                authority: a.mandate_signer.to_account_info(),
                protocol: a.protocol.to_account_info(),
                user_account: a.user_account.to_account_info(),
                market: a.market.to_account_info(),
                position: a.position.to_account_info(),
                trigger_order: a.trigger_order.to_account_info(),
                price_update: a.price_update.to_account_info(),
                secondary_price_update: Some(leg(&a.secondary_price_update, &a.solfx_core_program)),
                quote_conversion_price_update: Some(leg(
                    &a.quote_conversion_price_update,
                    &a.solfx_core_program,
                )),
                system_program: a.system_program.to_account_info(),
            },
            seeds,
        ),
        o.stop_order_id,
        TriggerKind::StopLoss,
        o.stop_loss_price,
        o.size_base,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_bound_never_lets_it_fill_worse_than_its_price() {
        use Direction::{Long, Short};
        assert!(price_limit_fits(EntryKind::Limit, Long, 100, 100));
        assert!(price_limit_fits(EntryKind::Limit, Long, 100, 99));
        assert!(!price_limit_fits(EntryKind::Limit, Long, 100, 101));
        assert!(price_limit_fits(EntryKind::Limit, Short, 100, 101));
        assert!(!price_limit_fits(EntryKind::Limit, Short, 100, 99));
    }

    #[test]
    fn a_stop_bound_sits_beyond_its_trigger() {
        use Direction::{Long, Short};
        assert!(price_limit_fits(EntryKind::Stop, Long, 100, 105));
        assert!(!price_limit_fits(EntryKind::Stop, Long, 100, 99));
        assert!(price_limit_fits(EntryKind::Stop, Short, 100, 95));
        assert!(!price_limit_fits(EntryKind::Stop, Short, 100, 101));
    }

    #[test]
    fn there_is_no_disabled_bound() {
        assert!(!price_limit_fits(EntryKind::Stop, Direction::Short, 100, 0));
        assert!(!price_limit_fits(
            EntryKind::Limit,
            Direction::Long,
            100,
            -1
        ));
    }
}
