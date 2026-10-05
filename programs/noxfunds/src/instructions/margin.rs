//! Moving margin into or out of a funded position.
//!
//! `solfx-core`'s `add_position_collateral` and `remove_position_collateral` only reclassify the
//! mandate's own money: free collateral becomes a position's margin, or back. Equity does not
//! move, so no mandate limit is touched. What does move is the bookkeeping `reconcile_position`
//! relies on — `booked_margin_fees` must equal what the open positions hold, and
//! `last_free_collateral` must track free collateral exactly — so both are adjusted by the change
//! **measured** in free collateral, never by the argument.
//!
//! Adding margin only lowers a position's risk, so it is allowed whatever the mandate's state
//! short of `Settled`, and while paused. Removing it raises the risk of liquidation, so it needs an
//! `Active`, unpaused mandate — and `solfx-core` still refuses to leave the position below initial
//! margin or above the market's leverage.

use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::constants::{CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED};
use crate::errors::NoxError;
use crate::events::FundedMarginMoved;
use crate::instructions::trading::leg;
use crate::state::{Mandate, MandateState, NoxConfig};
use crate::SolfxCore;

#[derive(Accounts)]
pub struct FundedAdjustMargin<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
        constraint = mandate.trader == trader.key() @ NoxError::NotTheTrader,
        constraint = mandate.state != MandateState::Settled @ NoxError::MandateNotActive,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub protocol: UncheckedAccount<'info>,
    /// Deserialized and reloaded after the CPI: the change in its free collateral is the amount
    /// that moved. `solfx-core` owns it, so Anchor never writes it back.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: Box<Account<'info, solfx_core::state::UserAccount>>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub market: UncheckedAccount<'info>,
    /// Deserialized so the position named is the position adjusted; `solfx-core` checks it
    /// belongs to `user_account`.
    #[account(mut)]
    pub position: Box<Account<'info, solfx_core::state::Position>>,
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
    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

/// Move `amount` of the mandate's free collateral into a position's margin.
pub fn funded_add_margin(
    ctx: Context<FundedAdjustMargin>,
    market_index: u16,
    nonce: u8,
    amount: u64,
) -> Result<()> {
    adjust(ctx, market_index, nonce, amount, true)
}

/// Move `amount` of a position's margin back to the mandate's free collateral.
pub fn funded_remove_margin(
    ctx: Context<FundedAdjustMargin>,
    market_index: u16,
    nonce: u8,
    amount: u64,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    require!(
        ctx.accounts.mandate.state == MandateState::Active,
        NoxError::MandateNotActive
    );
    adjust(ctx, market_index, nonce, amount, false)
}

fn adjust(
    ctx: Context<FundedAdjustMargin>,
    market_index: u16,
    nonce: u8,
    amount: u64,
    add: bool,
) -> Result<()> {
    require!(
        ctx.accounts.position.market_index == market_index && ctx.accounts.position.nonce == nonce,
        NoxError::PositionArgsMismatch
    );
    require!(amount > 0, NoxError::ZeroAmount);
    let free_before = ctx.accounts.user_account.free_collateral;

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];
    cpi_adjust(&ctx, seeds, amount, add)?;

    ctx.accounts.user_account.reload()?;
    let free_after = ctx.accounts.user_account.free_collateral;
    let m = &mut ctx.accounts.mandate;
    let moved = if add {
        let moved = free_before
            .checked_sub(free_after)
            .ok_or(NoxError::MathOverflow)?;
        m.booked_margin_fees = m
            .booked_margin_fees
            .checked_add(moved)
            .ok_or(NoxError::MathOverflow)?;
        m.last_free_collateral = m
            .last_free_collateral
            .checked_sub(i64::try_from(moved).map_err(|_| NoxError::MathOverflow)?)
            .ok_or(NoxError::MathOverflow)?;
        moved
    } else {
        let moved = free_after
            .checked_sub(free_before)
            .ok_or(NoxError::MathOverflow)?;
        m.booked_margin_fees = m
            .booked_margin_fees
            .checked_sub(moved)
            .ok_or(NoxError::MathOverflow)?;
        m.last_free_collateral = m
            .last_free_collateral
            .checked_add(i64::try_from(moved).map_err(|_| NoxError::MathOverflow)?)
            .ok_or(NoxError::MathOverflow)?;
        moved
    };

    emit!(FundedMarginMoved {
        mandate: mandate_key,
        market_index,
        nonce,
        added: add,
        amount: moved,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[inline(never)]
fn cpi_adjust(
    ctx: &Context<FundedAdjustMargin>,
    seeds: &[&[&[u8]]],
    amount: u64,
    add: bool,
) -> Result<()> {
    let a = &ctx.accounts;
    let accounts = solfx_core::cpi::accounts::DecreasePosition {
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
    };
    let cpi = CpiContext::new_with_signer(a.solfx_core_program.key(), accounts, seeds);
    if add {
        solfx_core::cpi::add_position_collateral(cpi, amount)
    } else {
        solfx_core::cpi::remove_position_collateral(cpi, amount)
    }
}
