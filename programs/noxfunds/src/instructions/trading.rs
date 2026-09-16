//! The instruction that is the whole product.
//!
//! # Why a rule check here is different from a rule check anywhere else
//!
//! Every other prop firm watches trades after they fill and punishes violations. They have no
//! choice: their contracts cannot see the order before the venue executes it. NOXFUNDS owns
//! the venue, so the check goes **in front of** the fill.
//!
//! A trade that breaks the rules is not detected and punished. It fails as a transaction. It
//! never existed, and the investor never took the loss.
//!
//! # Why the stop is placed in the same instruction
//!
//! SolFX has no `Position -> TriggerOrder` link, so "every funded trade carries a stop" is
//! only *true* — as opposed to monitored — if both land atomically. Two transactions means a
//! window where the position exists unprotected, and that window is exactly when a mandate
//! would be breached.

use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_core::state::{Direction, Market, TriggerKind};

use crate::constants::{BPS, CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED};
use crate::errors::NoxError;
use crate::events::{FundedTradeClosed, FundedTradeOpened, StopCancelled};
use crate::state::{Mandate, MandateState, NoxConfig};
use crate::SolfxCore;

/// Resolve an optional oracle leg to the `AccountInfo` the CPI should carry.
///
/// Present: the leg itself. Absent: the callee's own program id, which is the sentinel Anchor
/// uses for a missing optional account and which `solfx-core` decodes back to `None`. Passing
/// `None` here instead would emit an account meta with no backing `AccountInfo` in the
/// `invoke_signed` slice — `ToAccountMetas for Option<T>` writes `crate::ID`, `ToAccountInfos`
/// writes nothing.
fn leg<'info>(
    opt: &Option<Box<Account<'info, PriceUpdateV2>>>,
    fallback: &Program<'info, SolfxCore>,
) -> AccountInfo<'info> {
    match opt {
        Some(a) => a.to_account_info(),
        None => fallback.to_account_info(),
    }
}

#[derive(Accounts)]
pub struct FundedOpenPosition<'info> {
    /// The trader. Pays the transaction fee and signs; pays no rent and never holds custody.
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

    /// The SolFX authority: dataless, system-owned, signs both CPIs and pays both rents.
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
    /// CHECK: validated by `solfx-core`, and bound to this mandate below.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: UncheckedAccount<'info>,
    /// Deserialized: the rules are priced against it before the CPI.
    #[account(mut)]
    pub market: Box<Account<'info, Market>>,
    /// CHECK: created by CPI #1, read by CPI #2; validated by `solfx-core` both times.
    #[account(mut)]
    pub position: UncheckedAccount<'info>,
    /// CHECK: created by CPI #2; validated by `solfx-core`.
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

/// What a passing trade passed *by*. Emitted, so an investor can see a trader running at 99%
/// of every limit rather than only that nothing failed.
struct Margins {
    notional: u64,
    notional_used_bps: u64,
    risk_used_bps: u64,
}

/// Check every rule this trade can be judged against before it exists.
///
/// Ordered so the cheapest and most specific refusals come first: a trader who picked a
/// forbidden market should read `MarketNotPermitted`, not a stop-distance complaint.
#[allow(clippy::too_many_arguments)]
fn check_rules(
    mandate: &Mandate,
    market: &Market,
    price: &solfx_core::oracle::MarketPrice,
    market_index: u16,
    direction: Direction,
    size_base: u64,
    stop_loss_price: i64,
) -> Result<Margins> {
    require!(
        mandate.state == MandateState::Active,
        NoxError::MandateNotActive
    );
    require!(
        mandate.permits_market(market_index),
        NoxError::MarketNotPermitted
    );
    require!(
        mandate.open_positions < mandate.max_concurrent_positions,
        NoxError::TooManyOpenPositions
    );

    // --- the stop must exist, be on the right side, and be close enough to mean something --
    require!(stop_loss_price > 0, NoxError::StopLossRequired);
    let entry = price.spot.price;
    require!(entry > 0, NoxError::StopLossRequired);
    let correct_side = match direction {
        Direction::Long => stop_loss_price < entry,
        Direction::Short => stop_loss_price > entry,
    };
    require!(correct_side, NoxError::StopOnWrongSide);

    let distance = entry.abs_diff(stop_loss_price);
    // Ceiling, not floor. This is a limit the trade must stay *under*, so rounding the
    // measurement down would admit a stop fractionally further out than the mandate allows.
    // Every rounding on this path is adverse to whoever initiated the trade.
    let distance_bps = solfx_math::fixed::mul_div_ceil(
        u128::from(distance),
        u128::from(BPS),
        u128::try_from(entry).map_err(|_| NoxError::MathOverflow)?,
    )
    .map_err(NoxError::from)?;
    require!(
        distance_bps <= u128::from(mandate.max_stop_distance_bps),
        NoxError::StopTooFar
    );

    // --- notional, in USDC, exactly as solfx-core will compute it -------------------------
    let conversion_rate = price.quote_conversion_rate.unwrap_or(0);
    let notional = solfx_math::pnl::notional_in_collateral(
        size_base,
        entry,
        market.quote_conversion(),
        conversion_rate,
    )
    .map_err(NoxError::from)?;
    require!(
        notional <= mandate.max_trade_notional,
        NoxError::TradeExceedsMandate
    );

    // The per-trade ceiling bounds one mistake; this bounds the book. A mandate allowed three
    // positions at its per-trade ceiling could otherwise carry three times the exposure the
    // investor thought they were authorising.
    let total_after = mandate
        .open_notional
        .checked_add(notional)
        .ok_or(NoxError::MathOverflow)?;
    require!(
        total_after <= mandate.max_total_notional,
        NoxError::TotalNotionalExceeded
    );

    // --- risk at the stop -------------------------------------------------------------------
    //
    // The rule no centralized firm can enforce before the fill. It is `size × |entry − stop|`
    // rather than a position-size cap, so a trader may take a large position with a tight stop
    // or a small one with a wide stop, but never risk more than the mandate allows.
    let risk = solfx_math::pnl::notional_in_collateral(
        size_base,
        i64::try_from(distance).map_err(|_| NoxError::MathOverflow)?,
        market.quote_conversion(),
        conversion_rate,
    )
    .map_err(NoxError::from)?;
    let equity = mandate.peak_equity.max(1);
    let risk_used_bps =
        solfx_math::fixed::mul_div_ceil(u128::from(risk), u128::from(BPS), u128::from(equity))
            .map_err(NoxError::from)?;
    require!(
        risk_used_bps <= u128::from(mandate.max_risk_per_trade_bps),
        NoxError::RiskPerTradeExceeded
    );

    let notional_used_bps = solfx_math::fixed::mul_div_ceil(
        u128::from(notional),
        u128::from(BPS),
        u128::from(mandate.max_trade_notional.max(1)),
    )
    .map_err(NoxError::from)?;

    Ok(Margins {
        notional,
        notional_used_bps: solfx_math::fixed::to_u64(notional_used_bps).map_err(NoxError::from)?,
        risk_used_bps: solfx_math::fixed::to_u64(risk_used_bps).map_err(NoxError::from)?,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn funded_open_position(
    ctx: Context<FundedOpenPosition>,
    market_index: u16,
    nonce: u8,
    direction: Direction,
    size_base: u64,
    collateral: u64,
    price_limit: i64,
    order_id: u8,
    stop_loss_price: i64,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    let clock = Clock::get()?;

    // Priced with the venue's own function, through the venue's own gates. Not an
    // approximation of what solfx-core will see — the same code, on the same accounts.
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

    let margins = check_rules(
        &ctx.accounts.mandate,
        &ctx.accounts.market,
        &price,
        market_index,
        direction,
        size_base,
        stop_loss_price,
    )?;

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    cpi_open(
        &ctx,
        seeds,
        market_index,
        nonce,
        direction,
        size_base,
        collateral,
        price_limit,
    )?;
    cpi_place_stop(&ctx, seeds, order_id, stop_loss_price, size_base)?;

    let m = &mut ctx.accounts.mandate;
    m.open_positions = m.open_positions.saturating_add(1);
    m.open_notional = m
        .open_notional
        .checked_add(margins.notional)
        .ok_or(NoxError::MathOverflow)?;

    emit!(FundedTradeOpened {
        mandate: mandate_key,
        trader: m.trader,
        market_index,
        nonce,
        direction: direction as u8,
        size_base,
        notional: margins.notional,
        collateral,
        stop_loss_price,
        notional_used_bps: margins.notional_used_bps,
        risk_used_bps: margins.risk_used_bps,
        open_positions: m.open_positions,
        ts: clock.unix_timestamp,
    });
    Ok(())
}

/// `#[inline(never)]` so this `CpiContext` — 16 `AccountInfo` by value plus overhead, 840
/// bytes — cannot share a stack frame with the one below.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn cpi_open(
    ctx: &Context<FundedOpenPosition>,
    seeds: &[&[&[u8]]],
    market_index: u16,
    nonce: u8,
    direction: Direction,
    size_base: u64,
    collateral: u64,
    price_limit: i64,
) -> Result<()> {
    solfx_core::cpi::open_position(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::OpenPosition {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                market: ctx.accounts.market.to_account_info(),
                position: ctx.accounts.position.to_account_info(),
                collateral_vault: ctx.accounts.collateral_vault.to_account_info(),
                lp_pool: ctx.accounts.lp_pool.to_account_info(),
                lp_vault: ctx.accounts.lp_vault.to_account_info(),
                insurance_fund: ctx.accounts.insurance_fund.to_account_info(),
                insurance_vault: ctx.accounts.insurance_vault.to_account_info(),
                fee_vault: ctx.accounts.fee_vault.to_account_info(),
                price_update: ctx.accounts.price_update.to_account_info(),
                secondary_price_update: Some(leg(
                    &ctx.accounts.secondary_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                quote_conversion_price_update: Some(leg(
                    &ctx.accounts.quote_conversion_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                token_program: ctx.accounts.token_program.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
            },
            seeds,
        ),
        market_index,
        nonce,
        direction,
        size_base,
        collateral,
        price_limit,
    )
}

/// `#[inline(never)]` for the same reason — 552 bytes.
#[inline(never)]
fn cpi_place_stop(
    ctx: &Context<FundedOpenPosition>,
    seeds: &[&[&[u8]]],
    order_id: u8,
    stop_loss_price: i64,
    size_base: u64,
) -> Result<()> {
    solfx_core::cpi::place_trigger_order(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::PlaceTriggerOrder {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                market: ctx.accounts.market.to_account_info(),
                position: ctx.accounts.position.to_account_info(),
                trigger_order: ctx.accounts.trigger_order.to_account_info(),
                price_update: ctx.accounts.price_update.to_account_info(),
                secondary_price_update: Some(leg(
                    &ctx.accounts.secondary_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                quote_conversion_price_update: Some(leg(
                    &ctx.accounts.quote_conversion_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                system_program: ctx.accounts.system_program.to_account_info(),
            },
            seeds,
        ),
        order_id,
        TriggerKind::StopLoss,
        stop_loss_price,
        size_base,
    )
}

// --- closing ----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct FundedClosePosition<'info> {
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
    /// CHECK: validated by `solfx-core`, and bound to this mandate.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub market: UncheckedAccount<'info>,
    /// Deserialized here, unlike on the open path: the minimum-hold rule is judged against
    /// `opened_at_slot`, and the position's booked notional is what leaves the mandate's open
    /// book when it closes.
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

pub fn funded_close_position(
    ctx: Context<FundedClosePosition>,
    market_index: u16,
    nonce: u8,
    price_limit: i64,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);

    // The no-scalping rule, and **only on a voluntary close**.
    //
    // A stop-out is exempt by construction rather than by an exception: a stop fires through
    // `solfx-core`'s own `execute_trigger_order`, which never enters this instruction. So a
    // trader stopped out three minutes after entry does not breach a ten-minute floor through
    // something they did not do. Phase 7 reached the same conclusion for `min_hold_slots`.
    let held = Clock::get()?
        .slot
        .saturating_sub(ctx.accounts.position.opened_at_slot);
    require!(
        held >= ctx.accounts.mandate.min_hold_slots,
        NoxError::MinimumHoldNotMet
    );
    let booked = ctx.accounts.position.entry_notional;

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    cpi_close(&ctx, seeds, price_limit)?;

    let m = &mut ctx.accounts.mandate;
    m.open_positions = m.open_positions.saturating_sub(1);
    // Removed at the figure it was added at. Recomputing from the current price would make
    // the book drift on a mandate that merely held — the same reason SolFX stores
    // `entry_notional` rather than re-deriving open interest at close.
    m.open_notional = m.open_notional.saturating_sub(booked);

    emit!(FundedTradeClosed {
        mandate: mandate_key,
        trader: m.trader,
        market_index,
        nonce,
        open_positions: m.open_positions,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[inline(never)]
fn cpi_close(
    ctx: &Context<FundedClosePosition>,
    seeds: &[&[&[u8]]],
    price_limit: i64,
) -> Result<()> {
    solfx_core::cpi::close_position(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::ClosePosition {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                market: ctx.accounts.market.to_account_info(),
                position: ctx.accounts.position.to_account_info(),
                collateral_vault: ctx.accounts.collateral_vault.to_account_info(),
                lp_pool: ctx.accounts.lp_pool.to_account_info(),
                lp_vault: ctx.accounts.lp_vault.to_account_info(),
                insurance_fund: ctx.accounts.insurance_fund.to_account_info(),
                insurance_vault: ctx.accounts.insurance_vault.to_account_info(),
                fee_vault: ctx.accounts.fee_vault.to_account_info(),
                price_update: ctx.accounts.price_update.to_account_info(),
                secondary_price_update: Some(leg(
                    &ctx.accounts.secondary_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                quote_conversion_price_update: Some(leg(
                    &ctx.accounts.quote_conversion_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                token_program: ctx.accounts.token_program.to_account_info(),
            },
            seeds,
        ),
        price_limit,
    )
}

// --- reclaiming a stop's rent --------------------------------------------------------------

/// Cancel a resting stop and return its rent to the mandate signer.
///
/// # Why this exists
///
/// A `TriggerOrder`'s rent goes to the keeper when the stop *fires*, and to the authority when
/// it is *cancelled*. There is no third path. So a voluntary close leaves an orphaned order
/// holding 2,039,280 lamports with nothing able to reclaim it, and a mandate would bleed
/// ~0.002 SOL per closed trade. The original plan's instruction list omitted this.
#[derive(Accounts)]
pub struct FundedCancelStop<'info> {
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
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: validated by `solfx-core`, which also checks the authority matches.
    #[account(mut)]
    pub trigger_order: UncheckedAccount<'info>,

    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

pub fn funded_cancel_stop(
    ctx: Context<FundedCancelStop>,
    market_index: u16,
    nonce: u8,
    order_id: u8,
) -> Result<()> {
    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    solfx_core::cpi::cancel_trigger_order(CpiContext::new_with_signer(
        ctx.accounts.solfx_core_program.key(),
        solfx_core::cpi::accounts::CancelTriggerOrder {
            authority: ctx.accounts.mandate_signer.to_account_info(),
            trigger_order: ctx.accounts.trigger_order.to_account_info(),
        },
        seeds,
    ))?;

    emit!(StopCancelled {
        mandate: mandate_key,
        market_index,
        nonce,
        order_id,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
