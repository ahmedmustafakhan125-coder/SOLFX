//! Take-profit and stop-loss orders (`ARCHITECTURE.md` § 5.4).
//!
//! # Why these waited for Phase 7
//!
//! The on-chain half was ready in Phase 4. It was deliberately held back, because **an order
//! that nothing fires is worse than no order at all**: a trader who places a stop-loss and
//! believes they are protected, on a protocol with no executor running, is worse off than one
//! who knows they must watch the position themselves. So the account, the instructions and
//! the keeper that fires them all land together.
//!
//! # What a trigger is, and is not
//!
//! It **is** a public, permissionless instruction: anyone may execute an order whose
//! condition is met, and be paid a tip for doing it. The trader does not have to trust our
//! bot to be running, because they are not relying on *our* bot.
//!
//! It is **not** a guaranteed fill price. The condition is checked against the *oracle*
//! price; the close then fills at the execution price, which crosses the adverse spread and,
//! in a gap, may be well past the trigger. Every broker works this way and the UI must say so
//! plainly — the alternative is a trader who thinks they were promised a price and finds out
//! during the one event where it matters.

use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, POSITION_SEED, PROTOCOL_SEED, TRIGGER_SEED, USER_SEED,
};
use crate::errors::SolfxError;
use crate::events::{TriggerOrderCancelled, TriggerOrderExecuted, TriggerOrderPlaced};
use crate::oracle::load_validated_price;
use crate::state::{
    InsuranceFund, LpPool, Market, Position, Protocol, TriggerKind, TriggerOrder, UserAccount,
};

use super::close_position::{reduce, ReduceRefs};
use super::VaultTransfer;

#[derive(Accounts)]
#[instruction(order_id: u8)]
pub struct PlaceTriggerOrder<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        seeds = [USER_SEED, authority.key().as_ref()],
        bump = user_account.bump,
        has_one = authority @ SolfxError::AuthorityMismatch,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,

    #[account(
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        seeds = [
            POSITION_SEED,
            user_account.key().as_ref(),
            &position.market_index.to_le_bytes(),
            &[position.nonce],
        ],
        bump = position.bump,
        constraint = position.user_account == user_account.key() @ SolfxError::AuthorityMismatch,
    )]
    pub position: Box<Account<'info, Position>>,

    #[account(
        init,
        payer = authority,
        space = 8 + TriggerOrder::INIT_SPACE,
        seeds = [TRIGGER_SEED, position.key().as_ref(), &[order_id]],
        bump,
    )]
    pub trigger_order: Box<Account<'info, TriggerOrder>>,

    /// Needed to check the trigger sits on the correct side of the current price.
    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub system_program: Program<'info, System>,
}

/// Attach a take-profit or stop-loss to a position.
///
/// The trigger must sit on the side of the market that has not happened yet — a take-profit
/// below a long, or a stop above it, is already met and would fire on the next keeper pass.
/// That is not an order; it is a delayed market close, and refusing it here tells the trader
/// immediately instead of a second later.
pub fn place_trigger_order(
    ctx: Context<PlaceTriggerOrder>,
    order_id: u8,
    kind: TriggerKind,
    trigger_price: i64,
    size_base: u64,
) -> Result<()> {
    require!(trigger_price > 0, SolfxError::InvalidTriggerPrice);
    require!(size_base > 0, SolfxError::ZeroAmount);
    require!(
        size_base <= ctx.accounts.position.size_base,
        SolfxError::ReductionExceedsSize
    );

    let clock = Clock::get()?;
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

    require!(
        kind.is_placeable(
            ctx.accounts.position.direction,
            trigger_price,
            price.spot.price
        ),
        SolfxError::TriggerAlreadyMet
    );

    let order = &mut ctx.accounts.trigger_order;
    order.position = ctx.accounts.position.key();
    order.user_account = ctx.accounts.user_account.key();
    order.authority = ctx.accounts.authority.key();
    order.market_index = ctx.accounts.market.market_index;
    order.order_id = order_id;
    order.kind = kind;
    order.trigger_price = trigger_price;
    order.size_base = size_base;
    order.created_at = clock.unix_timestamp;
    order.bump = ctx.bumps.trigger_order;

    emit!(TriggerOrderPlaced {
        order: order.key(),
        position: order.position,
        authority: order.authority,
        market_index: order.market_index,
        order_id,
        kind: kind as u8,
        trigger_price,
        size_base,
        spot_at_placement: price.spot.price,
        ts: clock.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct CancelTriggerOrder<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        mut,
        close = authority,
        seeds = [TRIGGER_SEED, trigger_order.position.as_ref(), &[trigger_order.order_id]],
        bump = trigger_order.bump,
        constraint = trigger_order.authority == authority.key() @ SolfxError::AuthorityMismatch,
    )]
    pub trigger_order: Box<Account<'info, TriggerOrder>>,
}

/// Withdraw a resting order and reclaim its rent. Owner only — a trigger nobody but its owner
/// can cancel is the point of putting it on chain.
pub fn cancel_trigger_order(ctx: Context<CancelTriggerOrder>) -> Result<()> {
    let order = &ctx.accounts.trigger_order;
    emit!(TriggerOrderCancelled {
        order: order.key(),
        position: order.position,
        authority: order.authority,
        order_id: order.order_id,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ExecuteTriggerOrder<'info> {
    /// Anyone. The tip is what makes that true in practice rather than only in principle.
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(mut, seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [USER_SEED, user_account.authority.as_ref()],
        bump = user_account.bump,
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

    /// **Closed to the keeper — its rent is the tip.**
    ///
    /// The trader paid this rent when they placed the order, so the fee is pre-funded by the
    /// party who wanted the order and costs the protocol nothing. It needs no token account,
    /// moves no USDC, and therefore adds no term to invariant I7 — which is also why the
    /// account list stays inside the BPF stack frame.
    #[account(
        mut,
        close = keeper,
        seeds = [TRIGGER_SEED, position.key().as_ref(), &[trigger_order.order_id]],
        bump = trigger_order.bump,
        constraint = trigger_order.position == position.key() @ SolfxError::AuthorityMismatch,
    )]
    pub trigger_order: Box<Account<'info, TriggerOrder>>,

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

/// Fire a trigger whose condition the oracle has met.
///
/// # Permissionless, and paid — in rent
///
/// Anyone may call this, and whoever lands the transaction is closed the order account and
/// keeps its **rent** (~0.0016 SOL). Both halves matter: permissionless means a trader's stop
/// does not depend on *our* bot being up, and paid means somebody other than us has a reason
/// to run one. A permissionless instruction nobody is paid to call is a promise with no
/// mechanism behind it.
///
/// Paying in rent rather than USDC is not a shortcut. The trader funded that rent when they
/// placed the order, so the fee is **pre-paid by the party who wanted the order**, costs the
/// protocol nothing, requires no token account, and moves no USDC — which is why executing a
/// trigger adds no term to invariant I7 and why the instruction fits its stack frame.
///
/// # Why the minimum-hold rule does not apply
///
/// § 6.6's `min_hold_slots` stops a trader opening and closing in the same slot to scalp a
/// spread. A trigger cannot be used that way: the trader does not choose when it fires, the
/// market does, and the close still crosses the adverse spread and pays the close fee — so
/// the round trip still loses. Applying the hold here would instead mean a stop-loss that
/// cannot fire during the first seconds of a position, which is when a bad fill is most
/// likely to need one.
pub fn execute_trigger_order(ctx: Context<ExecuteTriggerOrder>) -> Result<()> {
    let clock = Clock::get()?;

    require!(
        ctx.accounts.market.status.allows_close(),
        SolfxError::MarketHalted
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

    let order = &ctx.accounts.trigger_order;
    let direction = ctx.accounts.position.direction;

    // The condition is checked against the **oracle** price, not the fill price. A trigger is
    // a trigger, not a promise about where it fills.
    require!(
        order
            .kind
            .is_met(direction, order.trigger_price, price.spot.price),
        SolfxError::TriggerNotMet
    );

    // The position may have shrunk since the order was placed.
    let size_delta = order.size_base.min(ctx.accounts.position.size_base);
    require!(size_delta > 0, SolfxError::PositionNotEmpty);

    let order_id = order.order_id;
    let kind = order.kind;
    let trigger_price = order.trigger_price;
    let order_key = order.key();
    let position_key = ctx.accounts.position.key();

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

    // No slippage bound: the trader already expressed their limit as the trigger, and a bound
    // here would mean a stop that silently fails to close in exactly the fast market it was
    // placed for. The same reasoning as liquidation.
    let unbounded = match direction {
        crate::state::Direction::Long => 1,
        crate::state::Direction::Short => i64::MAX,
    };

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
        unbounded,
        &price,
        &clock,
    )?;

    // A trigger may be partial, so the position is closed **conditionally** here rather than
    // by a `close =` constraint, which would fire unconditionally and delete a position that
    // still has size. Anchor's `exit` skips re-serialising an account closed this way
    // (`is_closed`), so this is the supported form.
    //
    // Its rent goes to the keeper as well. The trader would have got it back on a manual
    // close, so an automated close costs them roughly 0.002 SOL — that is the price of not
    // having to be awake, and it is what makes a stranger's bot worth running.
    let mut tip = ctx.accounts.trigger_order.to_account_info().lamports();
    if ctx.accounts.position.size_base == 0 {
        let position_rent = ctx.accounts.position.to_account_info().lamports();
        ctx.accounts
            .position
            .close(ctx.accounts.keeper.to_account_info())?;
        tip = tip
            .checked_add(position_rent)
            .ok_or(SolfxError::MathOverflow)?;
    }

    emit!(TriggerOrderExecuted {
        order: order_key,
        position: position_key,
        keeper: ctx.accounts.keeper.key(),
        market_index: ctx.accounts.market.market_index,
        order_id,
        kind: kind as u8,
        trigger_price,
        oracle_price: price.spot.price,
        size_base: size_delta,
        keeper_tip_lamports: tip,
        ts: clock.unix_timestamp,
    });
    Ok(())
}
