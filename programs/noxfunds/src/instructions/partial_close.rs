//! Closing part of a funded position.
//!
//! # Measured, not recomputed
//!
//! `solfx-core`'s `decrease_position` releases margin pro rata (floored), settles the whole
//! position's carry out of its collateral, takes the close fee, and credits the rest to free
//! collateral. It leaves `open_fee_paid` alone. So this instruction learns everything it books
//! from two before-and-after readings, the way `funded_close_position` does:
//!
//! | Reading | What it is |
//! |---|---|
//! | free collateral after − before | what the partial close credited back |
//! | position collateral before − after | the margin (and carry) that left the position |
//! | the difference | the partial close's result, fees and carry included |
//!
//! The open fee stays booked with the surviving position and is charged to the trade when it
//! finally closes — the same figure `reconcile_position` reads for a live position
//! (`collateral + open_fee_paid`), so the mandate's accounting identity survives a partial.
//!
//! # What the record does with it
//!
//! The result moves gross profit or gross loss; it does **not** count as a trade. See
//! `TraderProfile::record_partial` for why: counting it would let one winner become ten wins.

use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::constants::{CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED, TRADER_SEED};
use crate::errors::NoxError;
use crate::events::PartialCloseRecorded;
use crate::instructions::trading::leg;
use crate::state::{Mandate, NoxConfig, TraderProfile};
use crate::SolfxCore;

#[derive(Accounts)]
pub struct FundedReducePosition<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
        constraint = mandate.trader == trader.key() @ NoxError::NotTheTrader,
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
    /// Deserialized and reloaded after the CPI: its free collateral is what the close credited.
    /// `solfx-core` owns it, so Anchor never writes it back on exit.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: Box<Account<'info, solfx_core::state::UserAccount>>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub market: UncheckedAccount<'info>,
    /// Deserialized and reloaded: its collateral before and after is the margin that left it,
    /// and its opening slot is what the minimum hold is judged from.
    #[account(mut)]
    pub position: Box<Account<'info, solfx_core::state::Position>>,
    /// Bound to the mandate's trader by its seed, so a result lands on the right record.
    #[account(
        mut,
        seeds = [TRADER_SEED, mandate.trader.as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == mandate.trader @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,
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

/// Close `size_delta` of a funded position, keeping the rest open with its stop.
///
/// A voluntary close, so it waits for `min_hold_slots` like a whole one. Not gated on the pause
/// flag: it only takes risk off. The stop keeps its original size; `solfx-core` clamps a trigger
/// to what is left when it fires, so it still closes the whole remainder.
pub fn funded_reduce_position(
    ctx: Context<FundedReducePosition>,
    market_index: u16,
    nonce: u8,
    size_delta: u64,
    price_limit: i64,
) -> Result<()> {
    let pos = &ctx.accounts.position;
    // The slot released must be this position's (internal review R-5, on the whole close).
    require!(
        pos.market_index == market_index && pos.nonce == nonce,
        NoxError::PositionArgsMismatch
    );
    let held = Clock::get()?.slot.saturating_sub(pos.opened_at_slot);
    require!(
        held >= ctx.accounts.mandate.min_hold_slots,
        NoxError::MinimumHoldNotMet
    );
    // Strictly partial. A whole close goes through `funded_close_position`, which counts the
    // trade and releases the slot; letting this one take everything would leave a slot booked
    // against a position that no longer exists.
    require!(
        size_delta > 0 && size_delta < pos.size_base,
        NoxError::PositionSizeOutOfBounds
    );

    let free_before = ctx.accounts.user_account.free_collateral;
    let margin_before = pos.collateral;
    let size_before = pos.size_base;

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];
    cpi_decrease(&ctx, seeds, size_delta, price_limit)?;

    // Both are stale the instant the CPI returns; `reload` re-reads the bytes and re-checks the
    // owner, as `funded_close_position` does for the user account.
    ctx.accounts.user_account.reload()?;
    ctx.accounts.position.reload()?;
    let credit = ctx
        .accounts
        .user_account
        .free_collateral
        .checked_sub(free_before)
        .ok_or(NoxError::MathOverflow)?;
    let margin_out = margin_before
        .checked_sub(ctx.accounts.position.collateral)
        .ok_or(NoxError::MathOverflow)?;
    let realized = i64::try_from(
        i128::from(credit)
            .checked_sub(i128::from(margin_out))
            .ok_or(NoxError::MathOverflow)?,
    )
    .map_err(|_| NoxError::MathOverflow)?;

    let m = &mut ctx.accounts.mandate;
    let released = m.release_partial(market_index, nonce, size_delta, size_before)?;
    m.booked_margin_fees = m
        .booked_margin_fees
        .checked_sub(margin_out)
        .ok_or(NoxError::MathOverflow)?;
    m.last_free_collateral = m
        .last_free_collateral
        .checked_add(i64::try_from(credit).map_err(|_| NoxError::MathOverflow)?)
        .ok_or(NoxError::MathOverflow)?;
    m.realized_pnl = m
        .realized_pnl
        .checked_add(realized)
        .ok_or(NoxError::MathOverflow)?;

    let profile = &mut ctx.accounts.trader_profile;
    profile.record_partial(realized)?;

    emit!(PartialCloseRecorded {
        profile: profile.key(),
        mandate: mandate_key,
        trader: m.trader,
        market_index,
        nonce,
        size_closed: size_delta,
        realized_pnl: realized,
        notional_released: released,
        gross_profit: profile.gross_profit,
        gross_loss: profile.gross_loss,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

/// The decrease CPI, in its own frame, for the same reason every CPI here has one.
#[inline(never)]
fn cpi_decrease(
    ctx: &Context<FundedReducePosition>,
    seeds: &[&[&[u8]]],
    size_delta: u64,
    price_limit: i64,
) -> Result<()> {
    let a = &ctx.accounts;
    solfx_core::cpi::decrease_position(
        CpiContext::new_with_signer(
            a.solfx_core_program.key(),
            solfx_core::cpi::accounts::DecreasePosition {
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
            },
            seeds,
        ),
        size_delta,
        price_limit,
    )
}
