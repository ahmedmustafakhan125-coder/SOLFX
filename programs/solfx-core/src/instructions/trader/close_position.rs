use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_math::{fees, pnl, pricing, Side, TradeAction};

use crate::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, POSITION_SEED, PROTOCOL_SEED, USER_SEED,
};
use crate::errors::{IntoProgramResult, SolfxError};
use crate::events::{BadDebtIncurred, PositionDecreased};
use crate::oracle::load_validated_price;
use crate::state::{InsuranceFund, LpPool, Market, Position, Protocol, UserAccount};

use super::flows::compute_flows;
use super::open_position::execution_price_for;
use super::{settle, SettlementInput, VaultTransfer};

/// Accounts for a **partial** reduction. The position survives.
#[derive(Accounts)]
pub struct DecreasePosition<'info> {
    pub authority: Signer<'info>,

    #[account(mut, seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [USER_SEED, authority.key().as_ref()],
        bump = user_account.bump,
        has_one = authority @ SolfxError::AuthorityMismatch,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        seeds = [
            POSITION_SEED,
            user_account.key().as_ref(),
            &position.market_index.to_le_bytes(),
            &[position.nonce],
        ],
        bump = position.bump,
        constraint = position.user_account == user_account.key() @ SolfxError::AuthorityMismatch,
        constraint = position.market_index == market.market_index @ SolfxError::PositionMarketMismatch,
    )]
    pub position: Box<Account<'info, Position>>,

    #[account(mut, seeds = [COLLATERAL_VAULT_SEED], bump = protocol.collateral_vault_bump)]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [LP_POOL_SEED], bump = lp_pool.bump)]
    pub lp_pool: Box<Account<'info, LpPool>>,
    #[account(mut, seeds = [LP_VAULT_SEED], bump = lp_pool.lp_vault_bump)]
    pub lp_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [INSURANCE_FUND_SEED], bump = insurance_fund.bump)]
    pub insurance_fund: Box<Account<'info, InsuranceFund>>,
    #[account(mut, seeds = [INSURANCE_VAULT_SEED], bump = insurance_fund.vault_bump)]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [FEE_VAULT_SEED], bump = protocol.fee_vault_bump)]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub token_program: Program<'info, Token>,
}

/// Accounts for a **full** close. Identical to [`DecreasePosition`] except the position
/// account is closed and its rent returned to the trader (§ 6.8 step 8).
#[derive(Accounts)]
pub struct ClosePosition<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(mut, seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [USER_SEED, authority.key().as_ref()],
        bump = user_account.bump,
        has_one = authority @ SolfxError::AuthorityMismatch,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        close = authority,
        seeds = [
            POSITION_SEED,
            user_account.key().as_ref(),
            &position.market_index.to_le_bytes(),
            &[position.nonce],
        ],
        bump = position.bump,
        constraint = position.user_account == user_account.key() @ SolfxError::AuthorityMismatch,
        constraint = position.market_index == market.market_index @ SolfxError::PositionMarketMismatch,
    )]
    pub position: Box<Account<'info, Position>>,

    #[account(mut, seeds = [COLLATERAL_VAULT_SEED], bump = protocol.collateral_vault_bump)]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [LP_POOL_SEED], bump = lp_pool.bump)]
    pub lp_pool: Box<Account<'info, LpPool>>,
    #[account(mut, seeds = [LP_VAULT_SEED], bump = lp_pool.lp_vault_bump)]
    pub lp_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [INSURANCE_FUND_SEED], bump = insurance_fund.bump)]
    pub insurance_fund: Box<Account<'info, InsuranceFund>>,
    #[account(mut, seeds = [INSURANCE_VAULT_SEED], bump = insurance_fund.vault_bump)]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [FEE_VAULT_SEED], bump = protocol.fee_vault_bump)]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub token_program: Program<'info, Token>,
}

/// Everything a reduction needs, so the two entry points share one implementation.
///
/// The duplication above is in the *account structs* only — Anchor cannot express "the same
/// accounts, plus `close`" — while the logic below runs once. A second copy of the settlement
/// arithmetic is exactly where a partial close and a full close would drift apart.
struct ReduceRefs<'a, 'info> {
    protocol: &'a mut Protocol,
    user_account: &'a mut UserAccount,
    market: &'a mut Market,
    position: &'a mut Position,
    lp_pool: &'a mut LpPool,
    insurance: &'a mut InsuranceFund,
    transfer: VaultTransfer<'info>,
    lp_vault_balance: u64,
    position_key: Pubkey,
}

pub fn decrease_position(
    ctx: Context<DecreasePosition>,
    size_delta: u64,
    price_limit: i64,
) -> Result<()> {
    let clock = Clock::get()?;
    let price = read_price_for_decrease(&ctx, &clock)?;
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
    let lp_vault_balance = ctx.accounts.lp_vault.amount;
    let position_key = ctx.accounts.position.key();

    require!(
        size_delta < ctx.accounts.position.size_base,
        SolfxError::ReductionExceedsSize
    );

    reduce(
        ReduceRefs {
            protocol: &mut ctx.accounts.protocol,
            user_account: &mut ctx.accounts.user_account,
            market: &mut ctx.accounts.market,
            position: &mut ctx.accounts.position,
            lp_pool: &mut ctx.accounts.lp_pool,
            insurance: &mut ctx.accounts.insurance_fund,
            transfer,
            lp_vault_balance,
            position_key,
        },
        size_delta,
        price_limit,
        &price,
        &clock,
    )
}

/// Close the whole position and reclaim its rent.
pub fn close_position(ctx: Context<ClosePosition>, price_limit: i64) -> Result<()> {
    let clock = Clock::get()?;
    let price = read_price_for_close(&ctx, &clock)?;
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
    let lp_vault_balance = ctx.accounts.lp_vault.amount;
    let position_key = ctx.accounts.position.key();
    let size = ctx.accounts.position.size_base;

    reduce(
        ReduceRefs {
            protocol: &mut ctx.accounts.protocol,
            user_account: &mut ctx.accounts.user_account,
            market: &mut ctx.accounts.market,
            position: &mut ctx.accounts.position,
            lp_pool: &mut ctx.accounts.lp_pool,
            insurance: &mut ctx.accounts.insurance_fund,
            transfer,
            lp_vault_balance,
            position_key,
        },
        size,
        price_limit,
        &price,
        &clock,
    )
}

/// The shared reduction.
///
/// # Bad debt without a liquidation engine
///
/// Phase 4 owns liquidation. Until it exists a position can run past the point its margin
/// covers, and closing it would owe the pool more than the trader has. The loss is therefore
/// **capped at what the trader posted**, and the shortfall is recorded on `Protocol` and
/// emitted as an event rather than quietly absorbed.
///
/// The pool eats it in the meantime, which is § 6.9's *last* waterfall step reached first
/// because the two above it are not built. That is the honest Phase 3 behaviour: the money
/// is conserved, the shortfall is visible, and Phase 4 inserts the insurance fund and ADL
/// ahead of it.
fn reduce(
    refs: ReduceRefs<'_, '_>,
    size_delta: u64,
    price_limit: i64,
    price: &crate::oracle::MarketPrice,
    clock: &Clock,
) -> Result<()> {
    let ReduceRefs {
        protocol,
        user_account,
        market,
        position,
        lp_pool,
        insurance,
        transfer,
        lp_vault_balance,
        position_key,
    } = refs;

    require!(market.status.allows_close(), SolfxError::MarketHalted);
    require!(size_delta > 0, SolfxError::ZeroAmount);
    require!(
        size_delta <= position.size_base,
        SolfxError::ReductionExceedsSize
    );

    // Minimum hold (§ 6.6). A trader who opens and closes inside one slot pays two fees but
    // takes zero risk; if the protocol's spread is ever narrower than the true market spread
    // that is riskless profit, repeated by bots.
    if market.min_hold_slots > 0 {
        let held = clock.slot.saturating_sub(position.opened_at_slot);
        require!(held >= market.min_hold_slots, SolfxError::MinHoldTimeNotMet);
    }

    let side = Side::resolve(position.direction.into(), TradeAction::Close);
    let exec_price = execution_price_for(market, price, size_delta, side)?;
    pricing::validate_slippage(exec_price, price_limit, side)
        .map_err(|_| error!(SolfxError::SlippageExceeded))?;

    // --- PnL on the closed portion, in the quote currency then in USDC (C-3) ---
    let pnl_quote = pnl::upnl_in_quote(
        size_delta,
        position.entry_price,
        exec_price,
        position.direction.into(),
    )
    .or_program_err()?;
    let conversion_rate = price.quote_conversion_rate.unwrap_or(0);
    let realized_pnl =
        pnl::convert_pnl_to_collateral(pnl_quote, market.quote_conversion(), conversion_rate)
            .or_program_err()?;

    // --- close fee on the closed notional ---
    let notional_quote = pnl::notional_in_quote(size_delta, exec_price).or_program_err()?;
    let notional =
        pnl::convert_cost_to_collateral(notional_quote, market.quote_conversion(), conversion_rate)
            .or_program_err()?;
    let tier_rate = fees::fee_rate_for_volume(user_account.thirty_day_volume);
    let fee = fees::fee_amount(notional, market.close_fee_rate.min(tier_rate)).or_program_err()?;

    // --- collateral released, proportional to the fraction closed ---
    //
    // Floored, so a partial close never releases more than its share. The remainder stays
    // with the surviving position rather than accruing to the trader.
    let released = if size_delta == position.size_base {
        position.collateral
    } else {
        u64::try_from(
            u128::from(position.collateral)
                .checked_mul(u128::from(size_delta))
                .ok_or(SolfxError::MathOverflow)?
                .checked_div(u128::from(position.size_base))
                .ok_or(SolfxError::DivideByZero)?,
        )
        .map_err(|_| SolfxError::MathOverflow)?
    };

    // --- cap the loss at what the trader actually posted ---
    let available = i128::from(released) - i128::from(fee);
    let raw_equity = available + i128::from(realized_pnl);

    let (effective_pnl, credit, bad_debt) = if raw_equity >= 0 {
        (
            realized_pnl,
            u64::try_from(raw_equity).map_err(|_| SolfxError::MathOverflow)?,
            0u64,
        )
    } else {
        // The trader cannot pay the whole loss, so cap it at what they posted. The capped
        // PnL is `-available`, which makes the trader's net movement exactly `-released`:
        //
        //     collateral_delta = effective_pnl - fee = -(released - fee) - fee = -released
        //
        // They lose everything they put up and no more. The remainder is the shortfall.
        let capped = i64::try_from(-available).map_err(|_| SolfxError::MathOverflow)?;
        let shortfall = u64::try_from(-raw_equity).map_err(|_| SolfxError::MathOverflow)?;
        (capped, 0u64, shortfall)
    };

    let (alloc, flows) = compute_flows(fee, effective_pnl, protocol.fee_split())?;

    settle(
        SettlementInput {
            fee,
            pnl: effective_pnl,
            market_index: market.market_index,
            user_account: position.user_account,
            referrer: position.referrer,
        },
        flows,
        alloc,
        &transfer,
        protocol,
        lp_pool,
        insurance,
        market,
        lp_vault_balance,
        clock.unix_timestamp,
    )?;

    // The released margin returns to free collateral. `settle` has already moved the PnL and
    // the fee across the vault boundary, so only the reclassification is left here — and it
    // must not touch `total_user_collateral`, which those movements already adjusted.
    user_account.credit(credit)?;

    if bad_debt > 0 {
        protocol.total_bad_debt = protocol
            .total_bad_debt
            .checked_add(bad_debt)
            .ok_or(SolfxError::MathOverflow)?;
        emit!(BadDebtIncurred {
            market_index: market.market_index,
            position: position_key,
            amount: bad_debt,
            ts: clock.unix_timestamp,
        });
    }

    // --- book-keeping ---
    // Remove OI at the notional recorded when this size entered the book, not at today's
    // price. See `Position::entry_notional`.
    let closed_notional = if size_delta == position.size_base {
        position.entry_notional
    } else {
        u64::try_from(
            u128::from(position.entry_notional)
                .checked_mul(u128::from(size_delta))
                .ok_or(SolfxError::MathOverflow)?
                .checked_div(u128::from(position.size_base))
                .ok_or(SolfxError::DivideByZero)?,
        )
        .map_err(|_| SolfxError::MathOverflow)?
    };
    market.remove_open_interest(position.direction, closed_notional, size_delta)?;
    position.entry_notional = position.entry_notional.saturating_sub(closed_notional);

    position.size_base = position
        .size_base
        .checked_sub(size_delta)
        .ok_or(SolfxError::MathOverflow)?;
    position.collateral = position
        .collateral
        .checked_sub(released)
        .ok_or(SolfxError::MathOverflow)?;
    position.last_updated_at = clock.unix_timestamp;

    let fully_closed = position.size_base == 0;
    if fully_closed {
        market.open_position_count = market.open_position_count.saturating_sub(1);
        user_account.open_positions = user_account.open_positions.saturating_sub(1);
    } else {
        // Invariant I5: no position may hold size with zero collateral.
        require!(position.collateral > 0, SolfxError::PositionNotEmpty);
    }

    user_account.thirty_day_volume = user_account.thirty_day_volume.saturating_add(notional);

    emit!(PositionDecreased {
        position: position_key,
        user_account: position.user_account,
        market_index: market.market_index,
        size_closed: size_delta,
        remaining_size: position.size_base,
        oracle_price: price.spot.price,
        exec_price,
        realized_pnl: effective_pnl,
        fee,
        collateral_returned: credit,
        fully_closed,
        ts: clock.unix_timestamp,
    });

    Ok(())
}

fn read_price_for_decrease(
    ctx: &Context<DecreasePosition>,
    clock: &Clock,
) -> Result<crate::oracle::MarketPrice> {
    load_validated_price(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
        ctx.accounts
            .quote_conversion_price_update
            .as_deref()
            .map(|a| &**a),
        clock,
    )
}

fn read_price_for_close(
    ctx: &Context<ClosePosition>,
    clock: &Clock,
) -> Result<crate::oracle::MarketPrice> {
    load_validated_price(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
        ctx.accounts
            .quote_conversion_price_update
            .as_deref()
            .map(|a| &**a),
        clock,
    )
}
