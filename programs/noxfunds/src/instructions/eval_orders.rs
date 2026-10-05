//! Orders on an evaluation: a take-profit, a stop that can be tightened, and resting entries.
//!
//! # The two rules that make these safe to offer
//!
//! **Every rule of an open is checked at the fill.** An entry order is a request, not a
//! reservation: `eval_fill_entry_order` runs the same `precheck_open` and `open_virtual` a market
//! open runs, against the price and the balance at the moment it fills. An order that would
//! break a rule when it triggers does not fill; it waits, or expires.
//!
//! **No order is a way round the hold rules.** Stop-outs are exempt from the ten-minute hold and
//! from the 45-minute average, because a stop is the discipline the rules demand. That exemption
//! would be a loophole if a trader could drag the stop up to the price in minute one, or hang a
//! target one tick away. So a stop can be moved, and a target can fire, **only once the minimum
//! hold has passed** — and a target that fires counts as the voluntary close it is.
//!
//! # Oracle, then execution
//!
//! Triggers are judged on the oracle price, exactly as SolFX judges its own stops
//! (`TriggerKind::is_met`). A **limit** entry additionally never fills at an execution price
//! worse than its limit, spread included: a buy limit at 80,000 does not buy at 80,040. A **stop**
//! entry has no such bound — that is what a stop is.

use anchor_lang::prelude::*;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_core::state::{Direction, Market, TriggerKind};

use crate::constants::{
    CONFIG_SEED, EVAL_MAX_PENDING, EVAL_MAX_RISK_BPS, EVAL_MIN_HOLD_SECS, EVAL_ORDER_SEED,
    EVAL_SEED, VPOS_SEED,
};
use crate::errors::NoxError;
use crate::events::{
    EvalEntryOrderCancelled, EvalEntryOrderFilled, EvalEntryOrderPlaced, EvalStopMoved,
    EvalTakeProfitFired, EvalTakeProfitSet,
};
use crate::instructions::evaluation::{
    book_close, load_legs, open_virtual, precheck_open, realise, risk_bps, Opening,
};
use crate::state::{
    EntryKind, EvalEntryOrder, Evaluation, EvaluationState, NoxConfig, VirtualPosition,
};

/// **A filler is never out of pocket.** The filler pays the new position's rent and is paid the
/// order's; rent is linear in size, so the order being at least as large is the whole guarantee.
/// A compile-time check, so a field added to `VirtualPosition` that breaks it fails the build.
const _: () = assert!(EvalEntryOrder::INIT_SPACE >= VirtualPosition::INIT_SPACE);

/// Has this position been held for the ten-minute minimum?
fn held_long_enough(opened_at: i64, now: i64) -> bool {
    now.saturating_sub(opened_at) >= EVAL_MIN_HOLD_SECS
}

/// Can a stop at `new` replace one at `old` — strictly toward the price, so risk only falls?
///
/// Pure, and the whole of the tighten-only rule, so the property tests can hold it directly.
#[must_use]
pub fn is_tighter(direction: Direction, old: i64, new: i64) -> bool {
    match direction {
        Direction::Long => new > old,
        Direction::Short => new < old,
    }
}

/// Is a limit entry's execution price at or better than its limit?
#[must_use]
pub fn within_limit(direction: Direction, limit: i64, execution: i64) -> bool {
    match direction {
        Direction::Long => execution <= limit,
        Direction::Short => execution >= limit,
    }
}

// --- the take-profit --------------------------------------------------------------------------

#[derive(Accounts)]
pub struct EvalSetTakeProfit<'info> {
    pub trader: Signer<'info>,

    #[account(
        seeds = [EVAL_SEED, trader.key().as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
        has_one = trader @ NoxError::NotTheTrader,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(
        mut,
        seeds = [
            VPOS_SEED,
            evaluation.key().as_ref(),
            &virtual_position.market_index.to_le_bytes(),
            &[virtual_position.nonce],
        ],
        bump = virtual_position.bump,
        has_one = evaluation @ NoxError::PositionNotInEvaluation,
    )]
    pub virtual_position: Box<Account<'info, VirtualPosition>>,

    #[account(
        constraint = market.market_index == virtual_position.market_index
            @ NoxError::MarketNotPermitted,
    )]
    pub market: Box<Account<'info, Market>>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    /// The second leg of a synthetic market. Absent for a direct one; `load_validated_price`
    /// refuses a synthetic market without it, and checks it is the market's own feed.
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    /// The conversion leg of a market not quoted in USD, checked the same way.
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
}

/// Set, move, or clear (`trigger_price = 0`) a simulated position's take-profit.
///
/// Placed only where it has not already happened, by the test SolFX applies to its own: a
/// target below a long is not an order, it is a delayed close. Clearing reads no price, so a
/// target can be removed even while the feed is stale.
pub fn eval_set_take_profit(ctx: Context<EvalSetTakeProfit>, trigger_price: i64) -> Result<()> {
    let clock = Clock::get()?;
    if trigger_price != 0 {
        require!(trigger_price > 0, NoxError::TakeProfitOnWrongSide);
        let price = load_legs(
            &ctx.accounts.market,
            &ctx.accounts.price_update,
            &ctx.accounts.secondary_price_update,
            &ctx.accounts.quote_conversion_price_update,
            &clock,
        )?;
        require!(
            TriggerKind::TakeProfit.is_placeable(
                ctx.accounts.virtual_position.direction,
                trigger_price,
                price.spot.price
            ),
            NoxError::TakeProfitOnWrongSide
        );
    }
    let p = &mut ctx.accounts.virtual_position;
    p.take_profit_price = trigger_price;
    emit!(EvalTakeProfitSet {
        evaluation: ctx.accounts.evaluation.key(),
        position: p.key(),
        trigger_price,
        ts: clock.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct EvalTriggerTakeProfit<'info> {
    /// Anyone. A target that only its owner can fire is one the owner has to be awake for.
    pub keeper: Signer<'info>,

    /// CHECK: receives the closed position's rent. Constrained to the evaluation's trader, so a
    /// keeper cannot redirect it.
    #[account(mut, address = evaluation.trader @ NoxError::NotTheTrader)]
    pub trader: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [EVAL_SEED, evaluation.trader.as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(
        mut,
        close = trader,
        seeds = [
            VPOS_SEED,
            evaluation.key().as_ref(),
            &virtual_position.market_index.to_le_bytes(),
            &[virtual_position.nonce],
        ],
        bump = virtual_position.bump,
        has_one = evaluation @ NoxError::PositionNotInEvaluation,
    )]
    pub virtual_position: Box<Account<'info, VirtualPosition>>,

    #[account(
        constraint = market.market_index == virtual_position.market_index
            @ NoxError::MarketNotPermitted,
    )]
    pub market: Box<Account<'info, Market>>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    /// The second leg of a synthetic market. Absent for a direct one; `load_validated_price`
    /// refuses a synthetic market without it, and checks it is the market's own feed.
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    /// The conversion leg of a market not quoted in USD, checked the same way.
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
}

/// Fire a simulated take-profit. **Permissionless.**
///
/// On the oracle, inclusively, as SolFX fires a real one — and only once the ten-minute hold has
/// passed, because a target is a chosen exit. It is booked as a voluntary close, so it joins the
/// hold average like one.
pub fn eval_trigger_take_profit(ctx: Context<EvalTriggerTakeProfit>) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    require!(
        ctx.accounts.market.status.allows_close(),
        NoxError::MarketNotOpen
    );
    let pos = &ctx.accounts.virtual_position;
    require!(pos.take_profit_price > 0, NoxError::TakeProfitNotTriggered);
    require!(
        held_long_enough(pos.opened_at, now),
        NoxError::MinimumHoldNotMet
    );
    let price = load_legs(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        &ctx.accounts.secondary_price_update,
        &ctx.accounts.quote_conversion_price_update,
        &clock,
    )?;
    require!(
        TriggerKind::TakeProfit.is_met(pos.direction, pos.take_profit_price, price.spot.price),
        NoxError::TakeProfitNotTriggered
    );

    let r = realise(pos, &ctx.accounts.market, &price)?;
    let pos_key = pos.key();
    let pos = (**pos).clone();
    let key = ctx.accounts.evaluation.key();
    emit!(EvalTakeProfitFired {
        evaluation: key,
        position: pos_key,
        trigger_price: pos.take_profit_price,
        oracle_price: price.spot.price,
        keeper: ctx.accounts.keeper.key(),
        ts: now,
    });
    book_close(
        &mut ctx.accounts.evaluation,
        key,
        &pos,
        pos_key,
        &r,
        now,
        false,
    )
}

// --- moving the stop --------------------------------------------------------------------------

#[derive(Accounts)]
pub struct EvalMoveStop<'info> {
    pub trader: Signer<'info>,

    #[account(
        seeds = [EVAL_SEED, trader.key().as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
        has_one = trader @ NoxError::NotTheTrader,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(
        mut,
        seeds = [
            VPOS_SEED,
            evaluation.key().as_ref(),
            &virtual_position.market_index.to_le_bytes(),
            &[virtual_position.nonce],
        ],
        bump = virtual_position.bump,
        has_one = evaluation @ NoxError::PositionNotInEvaluation,
    )]
    pub virtual_position: Box<Account<'info, VirtualPosition>>,

    #[account(
        constraint = market.market_index == virtual_position.market_index
            @ NoxError::MarketNotPermitted,
    )]
    pub market: Box<Account<'info, Market>>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    /// The second leg of a synthetic market. Absent for a direct one; `load_validated_price`
    /// refuses a synthetic market without it, and checks it is the market's own feed.
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    /// The conversion leg of a market not quoted in USD, checked the same way.
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
}

/// Tighten a simulated stop.
///
/// **Toward the price only**, so the risk the trade was judged on can only fall and no rule
/// needs re-pricing. **Never to where it is already met** — that would be a close, not a stop.
/// **Only after the ten-minute hold**, so the stop-out exemption cannot be used to leave early.
pub fn eval_move_stop(ctx: Context<EvalMoveStop>, new_stop: i64) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let pos = &ctx.accounts.virtual_position;
    require!(
        held_long_enough(pos.opened_at, now),
        NoxError::MinimumHoldNotMet
    );
    require!(new_stop > 0, NoxError::StopLossRequired);
    require!(
        is_tighter(pos.direction, pos.stop_price, new_stop),
        NoxError::StopNotTighter
    );
    let price = load_legs(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        &ctx.accounts.secondary_price_update,
        &ctx.accounts.quote_conversion_price_update,
        &clock,
    )?;
    require!(
        TriggerKind::StopLoss.is_placeable(pos.direction, new_stop, price.spot.price),
        NoxError::TriggerAlreadyMet
    );

    let p = &mut ctx.accounts.virtual_position;
    let old_stop = p.stop_price;
    p.stop_price = new_stop;
    emit!(EvalStopMoved {
        evaluation: ctx.accounts.evaluation.key(),
        position: p.key(),
        old_stop,
        new_stop,
        ts: now,
    });
    Ok(())
}

// --- resting entry orders --------------------------------------------------------------------

/// What an entry order asks for. One struct, because ten loose arguments at a call site is how
/// a stop and a target get swapped.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryOrderParams {
    pub market_index: u16,
    pub nonce: u8,
    pub direction: Direction,
    pub kind: EntryKind,
    pub trigger_price: i64,
    pub size_base: u64,
    pub stop_loss_price: i64,
    /// Zero for none.
    pub take_profit_price: i64,
    /// Zero: good until cancelled.
    pub expires_at: i64,
}

#[derive(Accounts)]
#[instruction(order_id: u8)]
pub struct EvalPlaceEntryOrder<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [EVAL_SEED, trader.key().as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
        has_one = trader @ NoxError::NotTheTrader,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(
        init,
        payer = trader,
        space = 8 + EvalEntryOrder::INIT_SPACE,
        seeds = [EVAL_ORDER_SEED, evaluation.key().as_ref(), &[order_id]],
        bump,
    )]
    pub entry_order: Box<Account<'info, EvalEntryOrder>>,

    /// SolFX's own market account, read for its size bounds and its price shape. No price is
    /// read: an order may rest across a closed session, and every price rule runs at the fill.
    pub market: Box<Account<'info, Market>>,

    pub system_program: Program<'info, System>,
}

/// Rest an entry order on an evaluation.
///
/// What can be judged without a price is judged now — the shape of the bracket, the size, the
/// risk measured at the trigger — so an order that could never fill is refused here rather than
/// left resting. Everything is judged again, against the real price and balance, at the fill.
pub fn eval_place_entry_order(
    ctx: Context<EvalPlaceEntryOrder>,
    order_id: u8,
    params: EntryOrderParams,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    let now = Clock::get()?.unix_timestamp;
    let e = &ctx.accounts.evaluation;
    let market = &ctx.accounts.market;
    let o = params;

    require!(
        e.state == EvaluationState::Active,
        NoxError::EvaluationNotActive
    );
    require!(
        e.pending_orders < EVAL_MAX_PENDING,
        NoxError::TooManyPendingOrders
    );
    require!(
        market.market_index == o.market_index,
        NoxError::MarketNotPermitted
    );
    require!(o.size_base > 0, NoxError::ZeroAmount);
    require!(
        o.size_base >= market.min_position_size && o.size_base <= market.max_position_size,
        NoxError::PositionSizeOutOfBounds
    );
    require!(
        o.expires_at == 0 || o.expires_at > now,
        NoxError::InvalidExpiry
    );
    require!(o.trigger_price > 0, NoxError::EntryNotTriggered);

    // The bracket, around the trigger: a stop on the losing side, a target on the winning one.
    require!(o.stop_loss_price > 0, NoxError::StopLossRequired);
    require!(
        TriggerKind::StopLoss.is_placeable(o.direction, o.stop_loss_price, o.trigger_price),
        NoxError::StopOnWrongSide
    );
    if o.take_profit_price != 0 {
        require!(
            o.take_profit_price > 0
                && TriggerKind::TakeProfit.is_placeable(
                    o.direction,
                    o.take_profit_price,
                    o.trigger_price
                ),
            NoxError::TakeProfitOnWrongSide
        );
    }

    // Risk measured at the trigger, against today's balance — for a USD-quoted market, where it
    // needs no rate. A screen, not the rule: the fill measures it again, with the conversion rate
    // of the moment for a market that needs one.
    if !market.needs_quote_conversion() {
        let balance = Evaluation::floor_equity(i128::from(e.balance));
        let risk = risk_bps(
            market,
            0,
            o.size_base,
            o.trigger_price,
            o.stop_loss_price,
            balance,
        )?;
        require!(
            risk <= u64::from(EVAL_MAX_RISK_BPS),
            NoxError::RiskPerTradeExceeded
        );
    }

    let e_key = ctx.accounts.evaluation.key();
    let order = &mut ctx.accounts.entry_order;
    order.evaluation = e_key;
    order.trader = ctx.accounts.trader.key();
    order.order_id = order_id;
    order.market_index = o.market_index;
    order.nonce = o.nonce;
    order.direction = o.direction;
    order.kind = o.kind;
    order.trigger_price = o.trigger_price;
    order.size_base = o.size_base;
    order.stop_loss_price = o.stop_loss_price;
    order.take_profit_price = o.take_profit_price;
    order.expires_at = o.expires_at;
    order.created_at = now;
    order.bump = ctx.bumps.entry_order;

    let e = &mut ctx.accounts.evaluation;
    e.pending_orders = e.pending_orders.saturating_add(1);

    emit!(EvalEntryOrderPlaced {
        evaluation: e_key,
        order: order.key(),
        order_id,
        market_index: o.market_index,
        nonce: o.nonce,
        direction: o.direction as u8,
        kind: o.kind as u8,
        trigger_price: o.trigger_price,
        size_base: o.size_base,
        stop_loss_price: o.stop_loss_price,
        take_profit_price: o.take_profit_price,
        expires_at: o.expires_at,
        ts: now,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct EvalFillEntryOrder<'info> {
    /// Anyone. Pays the new position's rent and receives the order's, which is at least as much.
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [EVAL_SEED, evaluation.trader.as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(
        mut,
        close = keeper,
        seeds = [EVAL_ORDER_SEED, evaluation.key().as_ref(), &[entry_order.order_id]],
        bump = entry_order.bump,
        has_one = evaluation @ NoxError::PositionNotInEvaluation,
    )]
    pub entry_order: Box<Account<'info, EvalEntryOrder>>,

    #[account(
        init,
        payer = keeper,
        space = 8 + VirtualPosition::INIT_SPACE,
        seeds = [
            VPOS_SEED,
            evaluation.key().as_ref(),
            &entry_order.market_index.to_le_bytes(),
            &[entry_order.nonce],
        ],
        bump,
    )]
    pub virtual_position: Box<Account<'info, VirtualPosition>>,

    #[account(
        constraint = market.market_index == entry_order.market_index
            @ NoxError::MarketNotPermitted,
    )]
    pub market: Box<Account<'info, Market>>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    /// The second leg of a synthetic market. Absent for a direct one; `load_validated_price`
    /// refuses a synthetic market without it, and checks it is the market's own feed.
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    /// The conversion leg of a market not quoted in USD, checked the same way.
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub system_program: Program<'info, System>,
}

/// Fill an entry order whose trigger the oracle has reached. **Permissionless.**
///
/// Runs the whole of a market open — `precheck_open`, then `open_virtual` — at the price and
/// balance of the moment, so a fill is refused for exactly the reasons a market order would be.
pub fn eval_fill_entry_order(ctx: Context<EvalFillEntryOrder>) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let o = (**ctx.accounts.entry_order).clone();
    require!(
        o.expires_at == 0 || now <= o.expires_at,
        NoxError::EntryOrderExpired
    );

    precheck_open(&ctx.accounts.evaluation, &ctx.accounts.market, o.size_base)?;
    let price = load_legs(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        &ctx.accounts.secondary_price_update,
        &ctx.accounts.quote_conversion_price_update,
        &clock,
    )?;
    require!(
        o.kind
            .is_met(o.direction, o.trigger_price, price.spot.price),
        NoxError::EntryNotTriggered
    );

    let e_key = ctx.accounts.evaluation.key();
    let p_key = ctx.accounts.virtual_position.key();
    open_virtual(
        &mut ctx.accounts.evaluation,
        e_key,
        &mut ctx.accounts.virtual_position,
        p_key,
        &ctx.accounts.market,
        &price,
        Opening {
            market_index: o.market_index,
            nonce: o.nonce,
            bump: ctx.bumps.virtual_position,
            direction: o.direction,
            size_base: o.size_base,
            stop_loss_price: o.stop_loss_price,
            take_profit_price: o.take_profit_price,
        },
        now,
    )?;
    // A limit never fills worse than its price, spread included.
    if o.kind == EntryKind::Limit {
        require!(
            within_limit(
                o.direction,
                o.trigger_price,
                ctx.accounts.virtual_position.entry_price
            ),
            NoxError::EntryNotTriggered
        );
    }

    let e = &mut ctx.accounts.evaluation;
    e.pending_orders = e.pending_orders.saturating_sub(1);
    emit!(EvalEntryOrderFilled {
        evaluation: e_key,
        order: ctx.accounts.entry_order.key(),
        position: p_key,
        oracle_price: price.spot.price,
        keeper: ctx.accounts.keeper.key(),
        ts: now,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct EvalCancelEntryOrder<'info> {
    /// The trader at any time; anyone once the order can no longer fill.
    pub caller: Signer<'info>,

    /// CHECK: receives the order's rent. Constrained to the evaluation's trader.
    #[account(mut, address = evaluation.trader @ NoxError::NotTheTrader)]
    pub trader: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [EVAL_SEED, evaluation.trader.as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(
        mut,
        close = trader,
        seeds = [EVAL_ORDER_SEED, evaluation.key().as_ref(), &[entry_order.order_id]],
        bump = entry_order.bump,
        has_one = evaluation @ NoxError::PositionNotInEvaluation,
    )]
    pub entry_order: Box<Account<'info, EvalEntryOrder>>,
}

/// Cancel an entry order; its rent goes back to the trader.
///
/// The trader may cancel at any time. Anyone else may once it can no longer fill — expired, or
/// its evaluation over — so an abandoned order does not hold `pending_orders` up forever.
pub fn eval_cancel_entry_order(ctx: Context<EvalCancelEntryOrder>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let o = &ctx.accounts.entry_order;
    let lapsed = (o.expires_at != 0 && now > o.expires_at)
        || ctx.accounts.evaluation.state != EvaluationState::Active;
    require!(
        ctx.accounts.caller.key() == o.trader || lapsed,
        NoxError::NotYourOrderToCancel
    );
    let order = o.key();
    let e = &mut ctx.accounts.evaluation;
    e.pending_orders = e.pending_orders.saturating_sub(1);
    emit!(EvalEntryOrderCancelled {
        evaluation: e.key(),
        order,
        by: ctx.accounts.caller.key(),
        expired: lapsed,
        ts: now,
    });
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn tighten_only_is_toward_the_price_on_both_sides() {
        assert!(is_tighter(Direction::Long, 100, 101));
        assert!(!is_tighter(Direction::Long, 100, 100));
        assert!(!is_tighter(Direction::Long, 100, 99));
        assert!(is_tighter(Direction::Short, 100, 99));
        assert!(!is_tighter(Direction::Short, 100, 100));
        assert!(!is_tighter(Direction::Short, 100, 101));
    }

    #[test]
    fn entry_kinds_are_the_four_cases_inclusively() {
        use Direction::{Long, Short};
        // A buy limit fills at or below; a sell limit at or above.
        assert!(EntryKind::Limit.is_met(Long, 100, 100));
        assert!(EntryKind::Limit.is_met(Long, 100, 99));
        assert!(!EntryKind::Limit.is_met(Long, 100, 101));
        assert!(EntryKind::Limit.is_met(Short, 100, 100));
        assert!(EntryKind::Limit.is_met(Short, 100, 101));
        assert!(!EntryKind::Limit.is_met(Short, 100, 99));
        // A buy stop fills at or above; a sell stop at or below.
        assert!(EntryKind::Stop.is_met(Long, 100, 100));
        assert!(EntryKind::Stop.is_met(Long, 100, 101));
        assert!(!EntryKind::Stop.is_met(Long, 100, 99));
        assert!(EntryKind::Stop.is_met(Short, 100, 100));
        assert!(EntryKind::Stop.is_met(Short, 100, 99));
        assert!(!EntryKind::Stop.is_met(Short, 100, 101));
    }

    proptest::proptest! {
        /// **Tightening only ever reduces risk.** For any stop on the losing side of the price,
        /// any tighter stop that is still on that side is strictly closer to the price — so the
        /// risk the trade was judged on can only fall, which is why a move needs no re-pricing.
        #[test]
        fn a_tighter_stop_is_always_closer_to_the_price(
            spot in 1i64..1_000_000_000_000,
            old_gap in 1i64..1_000_000_000,
            new_gap in 1i64..1_000_000_000,
            long in proptest::bool::ANY,
        ) {
            let (d, old, new) = if long {
                (Direction::Long, spot - old_gap, spot - new_gap)
            } else {
                (Direction::Short, spot + old_gap, spot + new_gap)
            };
            if is_tighter(d, old, new) && TriggerKind::StopLoss.is_placeable(d, new, spot) {
                proptest::prop_assert!(spot.abs_diff(new) < spot.abs_diff(old));
            }
        }

        /// A buy limit is a sell stop's condition and a sell limit a buy stop's — and away from
        /// the trigger, for one direction, exactly one of limit and stop is met.
        #[test]
        fn entry_kinds_mirror_each_other(
            trigger in 1i64..1_000_000_000_000,
            price in 1i64..1_000_000_000_000,
        ) {
            use Direction::{Long, Short};
            proptest::prop_assert_eq!(
                EntryKind::Limit.is_met(Long, trigger, price),
                EntryKind::Stop.is_met(Short, trigger, price)
            );
            proptest::prop_assert_eq!(
                EntryKind::Limit.is_met(Short, trigger, price),
                EntryKind::Stop.is_met(Long, trigger, price)
            );
            if price != trigger {
                for d in [Long, Short] {
                    proptest::prop_assert_ne!(
                        EntryKind::Limit.is_met(d, trigger, price),
                        EntryKind::Stop.is_met(d, trigger, price)
                    );
                }
            } else {
                for d in [Long, Short] {
                    proptest::prop_assert!(EntryKind::Limit.is_met(d, trigger, price));
                    proptest::prop_assert!(EntryKind::Stop.is_met(d, trigger, price));
                }
            }
        }

        /// A limit is always satisfied at its own price, and never one unit worse.
        #[test]
        fn a_limit_is_met_at_its_price_and_not_one_unit_worse(
            limit in 2i64..1_000_000_000_000,
        ) {
            proptest::prop_assert!(within_limit(Direction::Long, limit, limit));
            proptest::prop_assert!(within_limit(Direction::Short, limit, limit));
            proptest::prop_assert!(!within_limit(Direction::Long, limit, limit + 1));
            proptest::prop_assert!(!within_limit(Direction::Short, limit, limit - 1));
        }
    }

    #[test]
    fn a_limit_bounds_the_execution_price_not_only_the_trigger() {
        assert!(within_limit(Direction::Long, 100, 100));
        assert!(!within_limit(Direction::Long, 100, 101));
        assert!(within_limit(Direction::Short, 100, 100));
        assert!(!within_limit(Direction::Short, 100, 99));
    }
}
