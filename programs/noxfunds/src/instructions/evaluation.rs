//! Stage 3 — the evaluation: how a trader earns a record before anyone risks capital on them.
//!
//! # The claim this module exists to make true
//!
//! A simulated fill here is not an approximation of a SolFX fill. It is computed by SolFX's own
//! functions, called rather than copied, in the order `open_position` and `close_position` call
//! them:
//!
//! | Step | Function |
//! |---|---|
//! | Price and every oracle gate | `solfx_core::oracle::load_validated_price` |
//! | Execution price (spread, confidence, skew) | `solfx_core::…::execution_price_for` |
//! | Notional, PnL, quote conversion | `solfx_math::pnl` |
//! | Fees | `solfx_math::fees::{fee_amount, fee_rate_for_volume}` |
//! | Carry at close | `solfx_math::funding::accrued_carry` |
//! | Whether a stop has fired | `solfx_core::state::TriggerKind::is_met`, on the oracle price |
//!
//! So on the same price update and the same market state, a virtual fill and a real one agree
//! to the unit — `tests/stage3.rs` opens both and compares.
//!
//! # What is deliberately different, stated rather than hidden
//!
//! - **The simulation does not move the market.** A virtual trade does not change open
//!   interest, so it does not shift the skew for anyone after it. It is priced exactly as a real
//!   trade would be in that state; it just leaves no footprint.
//! - **The whole balance is the margin.** There is no per-position collateral, so the leverage
//!   cap is checked against the simulated balance, and there is no simulated liquidation — the
//!   6% drawdown rule ends an evaluation long before a liquidation price could be reached.
//! - **Fees are the entry tier's.** A real account earns a volume discount; simulated volume
//!   earns none, so every evaluation pays what a new account pays.
//! - **Single-leg markets only.** The equity crank must be able to mark every open position or
//!   the drawdown rule is blind to it, and it reads one price per position — the same limit the
//!   funded mandate's crank has. Opening on a market that needs a second leg fails at
//!   `load_validated_price`, before anything is recorded.
//!
//! # No token moves in the simulation
//!
//! The only token account an evaluation touches is its own stake escrow, in `start`,
//! `claim_stage_pass` and `forfeit_stake`. The trading instructions take no token account at
//! all, which is what leaves SolFX's vault invariants untouched by construction.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_core::instructions::trader::execution_price_for;
use solfx_core::oracle::{load_validated_price, MarketPrice};
use solfx_core::state::{Direction, Market, TriggerKind};
use solfx_math::{fees, fixed, funding, pnl, pricing, Side, TradeAction};

use crate::constants::{
    BPS, CONFIG_SEED, EVAL_CONSISTENCY_BPS, EVAL_MAX_ACCOUNT, EVAL_MAX_DAILY_LOSS_BPS,
    EVAL_MAX_DRAWDOWN_BPS, EVAL_MAX_OPEN, EVAL_MAX_RISK_BPS, EVAL_MIN_ACCOUNT,
    EVAL_MIN_AVG_HOLD_SECS, EVAL_MIN_DAYS, EVAL_MIN_HOLD_SECS, EVAL_MIN_TRADES, EVAL_SEED,
    EVAL_STAKE, EVAL_TARGET_BPS, EVAL_VAULT_SEED, TRADER_SEED, VPOS_SEED,
};
use crate::errors::NoxError;
use crate::events::{
    EvaluationEquityObserved, EvaluationFailed, EvaluationStarted, EvaluationTradeClosed,
    EvaluationTradeOpened, StagePassed, StakeForfeited, StakeRefunded,
};
use crate::state::{
    EvalRule, Evaluation, EvaluationState, NoxConfig, TraderProfile, VirtualPosition,
};

const SECS_PER_DAY: i64 = 86_400;

fn utc_day(ts: i64) -> i64 {
    ts.div_euclid(SECS_PER_DAY)
}

/// Start a new UTC day if this action is the first of one. The day opens at `equity_now`.
fn roll_day(e: &mut Evaluation, now: i64, equity_now: u64) {
    let today = utc_day(now);
    if today != e.day {
        e.day = today;
        e.day_start_equity = equity_now;
        e.day_pnl = 0;
    }
}

/// Mark a failure — only from `Active`, so the event stream never records one twice.
fn fail(e: &mut Evaluation, key: Pubkey, rule: EvalRule, equity: u64, now: i64) {
    if e.state != EvaluationState::Active {
        return;
    }
    e.state = EvaluationState::Failed;
    e.failed_rule = rule;
    emit!(EvaluationFailed {
        evaluation: key,
        trader: e.trader,
        stage: e.stage,
        rule: rule as u8,
        equity,
        ts: now,
    });
}

/// Update the peak, check both loss limits against `equity`, fail the evaluation if either broke.
fn judge(e: &mut Evaluation, key: Pubkey, equity: u64, now: i64) {
    if equity > e.peak_equity {
        e.peak_equity = equity;
    }
    if e.drawdown_bps(equity) > u64::from(EVAL_MAX_DRAWDOWN_BPS) {
        fail(e, key, EvalRule::Drawdown, equity, now);
    } else if e.daily_loss_bps(equity) > u64::from(EVAL_MAX_DAILY_LOSS_BPS) {
        fail(e, key, EvalRule::DailyLoss, equity, now);
    }
}

// --- pricing, through SolFX's own functions ----------------------------------------------

/// What closing a position right now would realise, computed exactly as `close_position`
/// computes it: the adverse-side execution price, PnL converted to USDC, the close fee at the
/// entry-tier rate, and carry accrued since entry.
pub(crate) struct Realised {
    pub exit_price: i64,
    /// PnL minus the close fee minus carry. The open fee was taken at open.
    pub net: i64,
}

pub(crate) fn realise(
    pos: &VirtualPosition,
    market: &Market,
    price: &MarketPrice,
) -> Result<Realised> {
    let side = Side::resolve(pos.direction.into(), TradeAction::Close);
    let exit_price = execution_price_for(market, price, pos.size_base, side)?;
    let rate = price.quote_conversion_rate.unwrap_or(0);

    let pnl_quote = pnl::upnl_in_quote(
        pos.size_base,
        pos.entry_price,
        exit_price,
        pos.direction.into(),
    )
    .map_err(NoxError::from)?;
    let pnl_usdc = pnl::convert_pnl_to_collateral(pnl_quote, market.quote_conversion(), rate)
        .map_err(NoxError::from)?;

    let exit_notional = pnl::convert_cost_to_collateral(
        pnl::notional_in_quote(pos.size_base, exit_price).map_err(NoxError::from)?,
        market.quote_conversion(),
        rate,
    )
    .map_err(NoxError::from)?;
    let close_fee = fees::fee_amount(
        exit_notional,
        market.close_fee_rate.min(fees::fee_rate_for_volume(0)),
    )
    .map_err(NoxError::from)?;
    let carry = funding::accrued_carry(
        pos.entry_notional,
        market.cum_borrow_index,
        pos.cum_borrow_entry,
    )
    .map_err(NoxError::from)?;

    let net = i128::from(pnl_usdc)
        .checked_sub(i128::from(close_fee))
        .and_then(|v| v.checked_sub(i128::from(carry)))
        .ok_or(NoxError::MathOverflow)?;
    Ok(Realised {
        exit_price,
        net: i64::try_from(net).map_err(|_| NoxError::MathOverflow)?,
    })
}

/// Fold a closed trade into the evaluation's record and balance.
fn book_close(
    e: &mut Evaluation,
    key: Pubkey,
    pos: &VirtualPosition,
    pos_key: Pubkey,
    r: &Realised,
    now: i64,
    stop_out: bool,
) -> Result<()> {
    let held = now.saturating_sub(pos.opened_at);

    // The trade's whole result includes the fee it paid to open — the same correction the
    // funded record needed, so a round trip that lost only fees reads as a loss, not a wash.
    let result = i128::from(r.net)
        .checked_sub(i128::from(pos.open_fee))
        .ok_or(NoxError::MathOverflow)?;

    let balance_before = Evaluation::floor_equity(i128::from(e.balance));
    roll_day(e, now, balance_before);

    e.balance = i64::try_from(
        i128::from(e.balance)
            .checked_add(i128::from(r.net))
            .ok_or(NoxError::MathOverflow)?,
    )
    .map_err(|_| NoxError::MathOverflow)?;

    e.trades = e.trades.saturating_add(1);
    let magnitude = u64::try_from(result.unsigned_abs()).map_err(|_| NoxError::MathOverflow)?;
    if result > 0 {
        e.wins = e.wins.saturating_add(1);
        e.gross_profit = e.gross_profit.saturating_add(magnitude);
    } else {
        e.losses = e.losses.saturating_add(1);
        e.gross_loss = e.gross_loss.saturating_add(magnitude);
    }
    // Stop-outs are exempt from the hold rules and excluded from the average (§3.2.1): a stop
    // that fires fast is the discipline the rules demand, not scalping.
    if !stop_out {
        e.voluntary_closes = e.voluntary_closes.saturating_add(1);
        e.voluntary_hold_secs = e
            .voluntary_hold_secs
            .saturating_add(u64::try_from(held.max(0)).unwrap_or(0));
    }

    let day_pnl = i128::from(e.day_pnl)
        .checked_add(result)
        .ok_or(NoxError::MathOverflow)?;
    e.day_pnl = i64::try_from(day_pnl).map_err(|_| NoxError::MathOverflow)?;
    e.best_day_pnl = e.best_day_pnl.max(e.day_pnl);
    e.open_positions = e.open_positions.saturating_sub(1);

    // Flat, the balance *is* the equity — so the loss limits can be judged exactly, here, rather
    // than waiting for a crank.
    if e.open_positions == 0 {
        let equity = Evaluation::floor_equity(i128::from(e.balance));
        e.last_equity = equity;
        e.last_observed_at = now;
        judge(e, key, equity, now);
    }

    emit!(EvaluationTradeClosed {
        evaluation: key,
        position: pos_key,
        exit_price: r.exit_price,
        result: i64::try_from(result).map_err(|_| NoxError::MathOverflow)?,
        stop_out,
        held_secs: held,
        balance: e.balance,
        ts: now,
    });
    Ok(())
}

// --- start --------------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(seq: u8)]
pub struct StartEvaluation<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    /// Required: passing is recorded against a real profile, and an evaluation run by a wallet
    /// with no profile would earn a record nobody could find.
    #[account(
        seeds = [TRADER_SEED, trader.key().as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == trader.key() @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    #[account(
        init,
        payer = trader,
        space = 8 + Evaluation::INIT_SPACE,
        seeds = [EVAL_SEED, trader.key().as_ref(), &[seq]],
        bump,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(address = config.usdc_mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, token::mint = usdc_mint, token::authority = trader)]
    pub trader_token: Box<Account<'info, TokenAccount>>,

    /// The stake escrow, owned by the evaluation PDA. Nobody else can move it: refunded to the
    /// trader on a pass, forfeited to the treasury on a failure, and there is no third door.
    #[account(
        init,
        payer = trader,
        seeds = [EVAL_VAULT_SEED, evaluation.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = evaluation,
    )]
    pub stake_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn start_evaluation(ctx: Context<StartEvaluation>, seq: u8, account_size: u64) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    require!(
        (EVAL_MIN_ACCOUNT..=EVAL_MAX_ACCOUNT).contains(&account_size),
        NoxError::InvalidAccountSize
    );
    require!(
        ctx.accounts.trader_token.amount >= EVAL_STAKE,
        NoxError::InsufficientPrincipal
    );

    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.trader_token.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.stake_vault.to_account_info(),
                authority: ctx.accounts.trader.to_account_info(),
            },
        ),
        EVAL_STAKE,
        ctx.accounts.usdc_mint.decimals,
    )?;

    let now = Clock::get()?.unix_timestamp;
    let e = &mut ctx.accounts.evaluation;
    e.trader = ctx.accounts.trader.key();
    e.seq = seq;
    e.bump = ctx.bumps.evaluation;
    e.vault_bump = ctx.bumps.stake_vault;
    e.started_at = now;
    reset_stage(e, 1, account_size, now)?;

    emit!(EvaluationStarted {
        evaluation: e.key(),
        trader: e.trader,
        seq,
        account_size,
        stake: EVAL_STAKE,
        ts: now,
    });
    Ok(())
}

/// A fresh stage: full balance, clean record. Phase 2 is a separate run, not a continuation.
fn reset_stage(e: &mut Evaluation, stage: u8, account_size: u64, now: i64) -> Result<()> {
    e.stage = stage;
    e.state = EvaluationState::Active;
    e.failed_rule = EvalRule::None;
    e.account_size = account_size;
    e.balance = i64::try_from(account_size).map_err(|_| NoxError::MathOverflow)?;
    e.peak_equity = account_size;
    e.day = utc_day(now);
    e.day_start_equity = account_size;
    e.day_pnl = 0;
    e.best_day_pnl = 0;
    e.trades = 0;
    e.wins = 0;
    e.losses = 0;
    e.gross_profit = 0;
    e.gross_loss = 0;
    e.voluntary_closes = 0;
    e.voluntary_hold_secs = 0;
    e.trading_days = 0;
    e.last_trade_day = i64::MIN;
    e.open_positions = 0;
    e.last_equity = account_size;
    e.last_observed_at = now;
    Ok(())
}

// --- open ---------------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(market_index: u16, nonce: u8)]
pub struct EvalOpenPosition<'info> {
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
        space = 8 + VirtualPosition::INIT_SPACE,
        seeds = [VPOS_SEED, evaluation.key().as_ref(), &market_index.to_le_bytes(), &[nonce]],
        bump,
    )]
    pub virtual_position: Box<Account<'info, VirtualPosition>>,

    /// SolFX's own market account. `Account<Market>` checks it is owned by `solfx-core`, which
    /// is the only program that can create one, so it cannot be forged.
    #[account(constraint = market.market_index == market_index @ NoxError::MarketNotPermitted)]
    pub market: Box<Account<'info, Market>>,

    /// Pyth's price update. `load_validated_price` checks it is the market's own feed and
    /// applies every staleness and confidence gate a real fill applies.
    pub price_update: Box<Account<'info, PriceUpdateV2>>,

    pub system_program: Program<'info, System>,
}

pub fn eval_open_position(
    ctx: Context<EvalOpenPosition>,
    market_index: u16,
    nonce: u8,
    direction: Direction,
    size_base: u64,
    stop_loss_price: i64,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;

    let market = &ctx.accounts.market;
    let e = &ctx.accounts.evaluation;
    require!(
        e.state == EvaluationState::Active,
        NoxError::EvaluationNotActive
    );
    require!(
        e.open_positions < EVAL_MAX_OPEN,
        NoxError::TooManyOpenPositions
    );
    require!(market.status.allows_open(), NoxError::MarketNotOpen);
    require!(size_base > 0, NoxError::ZeroAmount);
    require!(
        size_base >= market.min_position_size && size_base <= market.max_position_size,
        NoxError::PositionSizeOutOfBounds
    );

    let price = load_validated_price(market, &ctx.accounts.price_update, None, None, &clock)?;

    // --- the stop: mandatory, and on the losing side of the market -------------------------
    require!(stop_loss_price > 0, NoxError::StopLossRequired);
    let spot = price.spot.price;
    let correct_side = match direction {
        Direction::Long => stop_loss_price < spot,
        Direction::Short => stop_loss_price > spot,
    };
    require!(correct_side, NoxError::StopOnWrongSide);

    // --- the fill, exactly as `open_position` computes it ----------------------------------
    let side = Side::resolve(direction.into(), TradeAction::Open);
    let entry_price = execution_price_for(market, &price, size_base, side)?;
    let rate = price.quote_conversion_rate.unwrap_or(0);
    let notional = pnl::convert_cost_to_collateral(
        pnl::notional_in_quote(size_base, entry_price).map_err(NoxError::from)?,
        market.quote_conversion(),
        rate,
    )
    .map_err(NoxError::from)?;
    pricing::validate_notional(notional).map_err(|_| NoxError::PositionSizeOutOfBounds)?;
    let fee = fees::fee_amount(
        notional,
        market.open_fee_rate.min(fees::fee_rate_for_volume(0)),
    )
    .map_err(NoxError::from)?;

    // --- the rules ---------------------------------------------------------------------------
    let balance = Evaluation::floor_equity(i128::from(e.balance));
    require!(balance > 0, NoxError::DrawdownExceeded);
    // Once a limit is broken, no new risk. The crank formally fails the evaluation; this just
    // refuses to let a breached account keep trading until it does.
    require!(
        e.drawdown_bps(balance) <= u64::from(EVAL_MAX_DRAWDOWN_BPS),
        NoxError::DrawdownExceeded
    );

    let leverage =
        solfx_math::margin::effective_leverage(notional, balance).map_err(NoxError::from)?;
    require!(
        leverage <= u64::from(market.effective_max_leverage()),
        NoxError::EvaluationLeverageTooHigh
    );

    // Risk at the stop: `size × |spot − stop|` in USDC, as bps of the balance, rounded up — the
    // same measure the funded path uses, so an evaluation trains the rule a mandate enforces.
    let distance = spot.abs_diff(stop_loss_price);
    let risk = pnl::notional_in_collateral(
        size_base,
        i64::try_from(distance).map_err(|_| NoxError::MathOverflow)?,
        market.quote_conversion(),
        rate,
    )
    .map_err(NoxError::from)?;
    let risk_used_bps = fixed::to_u64(
        fixed::mul_div_ceil(u128::from(risk), u128::from(BPS), u128::from(balance))
            .map_err(NoxError::from)?,
    )
    .map_err(NoxError::from)?;
    require!(
        risk_used_bps <= u64::from(EVAL_MAX_RISK_BPS),
        NoxError::RiskPerTradeExceeded
    );

    // --- book it ---------------------------------------------------------------------------
    let e_key = ctx.accounts.evaluation.key();
    let e = &mut ctx.accounts.evaluation;
    roll_day(e, now, balance);
    require!(
        e.daily_loss_bps(balance) <= u64::from(EVAL_MAX_DAILY_LOSS_BPS),
        NoxError::DailyLossExceeded
    );
    e.balance = e
        .balance
        .checked_sub(i64::try_from(fee).map_err(|_| NoxError::MathOverflow)?)
        .ok_or(NoxError::MathOverflow)?;
    let today = utc_day(now);
    if e.last_trade_day != today {
        e.last_trade_day = today;
        e.trading_days = e.trading_days.saturating_add(1);
    }
    e.open_positions = e.open_positions.saturating_add(1);

    let cum_borrow_entry = ctx.accounts.market.cum_borrow_index;
    let p = &mut ctx.accounts.virtual_position;
    p.evaluation = e_key;
    p.market_index = market_index;
    p.nonce = nonce;
    p.direction = direction;
    p.size_base = size_base;
    p.entry_price = entry_price;
    p.entry_notional = notional;
    p.stop_price = stop_loss_price;
    p.open_fee = fee;
    p.cum_borrow_entry = cum_borrow_entry;
    p.opened_at = now;
    p.bump = ctx.bumps.virtual_position;

    emit!(EvaluationTradeOpened {
        evaluation: e_key,
        position: p.key(),
        market_index,
        direction: direction as u8,
        size_base,
        entry_price,
        notional,
        fee,
        stop_price: stop_loss_price,
        risk_used_bps,
        ts: now,
    });
    Ok(())
}

// --- close, voluntarily -------------------------------------------------------------------

#[derive(Accounts)]
pub struct EvalClosePosition<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(
        mut,
        seeds = [EVAL_SEED, trader.key().as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
        has_one = trader @ NoxError::NotTheTrader,
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
}

/// Close a simulated position by choice. Subject to the ten-minute minimum hold.
///
/// Allowed on a failed evaluation too, so a trader is never stuck with positions whose rent they
/// cannot recover — but a failed evaluation stays failed.
pub fn eval_close_position(ctx: Context<EvalClosePosition>) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    require!(
        ctx.accounts.market.status.allows_close(),
        NoxError::MarketNotOpen
    );
    let pos = &ctx.accounts.virtual_position;
    require!(
        now.saturating_sub(pos.opened_at) >= EVAL_MIN_HOLD_SECS,
        NoxError::MinimumHoldNotMet
    );

    let price = load_validated_price(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        None,
        None,
        &clock,
    )?;
    let r = realise(pos, &ctx.accounts.market, &price)?;
    let pos_key = pos.key();
    let pos = (**pos).clone();
    let key = ctx.accounts.evaluation.key();
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

// --- stop-out, by anyone ------------------------------------------------------------------

#[derive(Accounts)]
pub struct EvalTriggerStop<'info> {
    /// Anyone. A stop that only its owner can fire is a stop the owner can choose not to fire.
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
}

/// Fire a simulated stop. **Permissionless**, and judged exactly as SolFX judges a real stop:
/// on the oracle price, inclusively, and filled at the execution price of the moment.
///
/// Exempt from the minimum hold and excluded from the average hold (§3.2.1): the rulebook
/// requires a stop, so it must not punish one for firing.
pub fn eval_trigger_stop(ctx: Context<EvalTriggerStop>) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    require!(
        ctx.accounts.market.status.allows_close(),
        NoxError::MarketNotOpen
    );
    let price = load_validated_price(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        None,
        None,
        &clock,
    )?;
    let pos = &ctx.accounts.virtual_position;
    require!(
        TriggerKind::StopLoss.is_met(pos.direction, pos.stop_price, price.spot.price),
        NoxError::StopNotTriggered
    );

    let r = realise(pos, &ctx.accounts.market, &price)?;
    let pos_key = pos.key();
    let pos = (**pos).clone();
    let key = ctx.accounts.evaluation.key();
    book_close(
        &mut ctx.accounts.evaluation,
        key,
        &pos,
        pos_key,
        &r,
        now,
        true,
    )
}

// --- the equity crank ---------------------------------------------------------------------

#[derive(Accounts)]
pub struct EvalObserveEquity<'info> {
    pub observer: Signer<'info>,

    #[account(
        mut,
        seeds = [EVAL_SEED, evaluation.trader.as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,
}

/// Mark every open simulated position and judge both loss limits. **Permissionless.**
///
/// `remaining_accounts` carries (virtual position, market, price update) triples — one per open
/// position, each exactly once. A position left out would hide a loss; a position supplied twice
/// would double a gain. Both are refused, the lesson the funded crank learned the hard way.
pub fn eval_observe_equity(ctx: Context<EvalObserveEquity>) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let rem = ctx.remaining_accounts;
    let key = ctx.accounts.evaluation.key();

    require!(rem.len().is_multiple_of(3), NoxError::IncompleteObservation);
    let mut seen = [Pubkey::default(); EVAL_MAX_OPEN as usize];
    let mut count: usize = 0;
    let mut equity = i128::from(ctx.accounts.evaluation.balance);

    for triple in rem.chunks_exact(3) {
        let [pos_info, market_info, price_info] = triple else {
            return Err(NoxError::IncompleteObservation.into());
        };
        let pos: Account<VirtualPosition> = Account::try_from(pos_info)?;
        let market: Account<Market> = Account::try_from(market_info)?;
        let price_update: Account<PriceUpdateV2> = Account::try_from(price_info)?;

        require!(pos.evaluation == key, NoxError::PositionNotInEvaluation);
        require!(
            pos.market_index == market.market_index,
            NoxError::MarketNotPermitted
        );
        require!(
            !seen.iter().take(count).any(|k| *k == pos.key()),
            NoxError::IncompleteObservation
        );
        let slot = seen.get_mut(count).ok_or(NoxError::IncompleteObservation)?;
        *slot = pos.key();
        count = count.checked_add(1).ok_or(NoxError::MathOverflow)?;

        let price = load_validated_price(&market, &price_update, None, None, &clock)?;
        let r = realise(&pos, &market, &price)?;
        equity = equity
            .checked_add(i128::from(r.net))
            .ok_or(NoxError::MathOverflow)?;
    }

    // And none left out.
    require!(
        count == usize::from(ctx.accounts.evaluation.open_positions),
        NoxError::IncompleteObservation
    );

    let equity = Evaluation::floor_equity(equity);
    let e = &mut ctx.accounts.evaluation;
    roll_day(e, now, equity);
    e.last_equity = equity;
    e.last_observed_at = now;
    let drawdown_bps = e.drawdown_bps(equity);
    let daily_loss_bps = e.daily_loss_bps(equity);
    judge(e, key, equity, now);

    emit!(EvaluationEquityObserved {
        evaluation: key,
        equity,
        peak_equity: e.peak_equity,
        drawdown_bps,
        daily_loss_bps,
        observer: ctx.accounts.observer.key(),
        ts: now,
    });
    Ok(())
}

// --- passing a stage ------------------------------------------------------------------------

#[derive(Accounts)]
pub struct ClaimStagePass<'info> {
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

    #[account(address = config.usdc_mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, token::mint = usdc_mint, token::authority = trader)]
    pub trader_token: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [EVAL_VAULT_SEED, evaluation.key().as_ref()],
        bump = evaluation.vault_bump,
    )]
    pub stake_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

/// Verify every rule of the current stage and advance. Phase 1 passed starts Phase 2 afresh;
/// Phase 2 passed ends the evaluation and refunds the stake in full.
///
/// Flat only: with a position open the balance is not the equity, and "passed" must mean the
/// target was banked, not merely shown on a mark.
pub fn claim_stage_pass(ctx: Context<ClaimStagePass>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let key = ctx.accounts.evaluation.key();
    let e = &ctx.accounts.evaluation;

    require!(
        e.state == EvaluationState::Active,
        NoxError::EvaluationNotActive
    );
    require!(e.open_positions == 0, NoxError::PositionsStillOpen);

    let equity = Evaluation::floor_equity(i128::from(e.balance));
    // Loss limits first: a stage cannot be passed by an account that is past either of them.
    require!(
        e.drawdown_bps(equity) <= u64::from(EVAL_MAX_DRAWDOWN_BPS),
        NoxError::DrawdownExceeded
    );

    let stage_index = usize::from(e.stage.saturating_sub(1));
    let target_bps = *EVAL_TARGET_BPS
        .get(stage_index)
        .ok_or(NoxError::EvaluationNotActive)?;
    // The target is a threshold to reach, so it is rounded up: rounding down would let a stage
    // pass fractionally short of its target.
    let target = fixed::to_u64(
        fixed::mul_div_ceil(
            u128::from(e.account_size),
            u128::from(target_bps),
            u128::from(BPS),
        )
        .map_err(NoxError::from)?,
    )
    .map_err(NoxError::from)?;
    let goal = e
        .account_size
        .checked_add(target)
        .ok_or(NoxError::MathOverflow)?;

    require!(equity >= goal, NoxError::EvaluationIncomplete);
    require!(e.trades >= EVAL_MIN_TRADES, NoxError::EvaluationIncomplete);
    require!(
        e.trading_days >= EVAL_MIN_DAYS,
        NoxError::EvaluationIncomplete
    );
    if e.voluntary_closes > 0 {
        let avg = e
            .voluntary_hold_secs
            .checked_div(u64::from(e.voluntary_closes))
            .ok_or(NoxError::MathOverflow)?;
        require!(
            avg >= EVAL_MIN_AVG_HOLD_SECS,
            NoxError::EvaluationIncomplete
        );
    }
    // No single day may carry more than half the target — one lucky session is not a record.
    let cap = fixed::mul_div_floor(
        u128::from(target),
        u128::from(EVAL_CONSISTENCY_BPS),
        u128::from(BPS),
    )
    .map_err(NoxError::from)?;
    require!(
        u128::try_from(e.best_day_pnl.max(0)).map_err(|_| NoxError::MathOverflow)? <= cap,
        NoxError::ConsistencyRuleViolated
    );

    let stage = e.stage;
    let trades = e.trades;
    let days = e.trading_days;
    let trader = e.trader;
    emit!(StagePassed {
        evaluation: key,
        trader,
        stage,
        equity,
        trades,
        trading_days: days,
        ts: now,
    });

    if stage == 1 {
        let size = e.account_size;
        reset_stage(&mut ctx.accounts.evaluation, 2, size, now)?;
        return Ok(());
    }

    ctx.accounts.evaluation.state = EvaluationState::Passed;
    let amount = ctx.accounts.stake_vault.amount;
    move_stake(
        &ctx.accounts.evaluation,
        &ctx.accounts.stake_vault,
        &ctx.accounts.usdc_mint,
        ctx.accounts.trader_token.to_account_info(),
        &ctx.accounts.token_program,
        amount,
    )?;
    emit!(StakeRefunded {
        evaluation: key,
        trader,
        amount,
        ts: now,
    });
    Ok(())
}

/// Move the stake out of escrow, signed by the evaluation PDA. Its own frame, so the
/// `CpiContext` never shares one with the caller's state.
#[inline(never)]
fn move_stake<'info>(
    evaluation: &Account<'info, Evaluation>,
    vault: &Account<'info, TokenAccount>,
    mint: &Account<'info, Mint>,
    to: AccountInfo<'info>,
    token_program: &Program<'info, Token>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let trader = evaluation.trader;
    let seq = [evaluation.seq];
    let bump = [evaluation.bump];
    let seeds: &[&[&[u8]]] = &[&[EVAL_SEED, trader.as_ref(), &seq, &bump]];
    token::transfer_checked(
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from: vault.to_account_info(),
                mint: mint.to_account_info(),
                to,
                authority: evaluation.to_account_info(),
            },
            seeds,
        ),
        amount,
        mint.decimals,
    )
}

// --- failure: forfeit and abandon -------------------------------------------------------------

#[derive(Accounts)]
pub struct ForfeitStake<'info> {
    /// Anyone. The treasury should not depend on the failed trader's cooperation.
    pub caller: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        seeds = [EVAL_SEED, evaluation.trader.as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,

    #[account(address = config.usdc_mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        seeds = [EVAL_VAULT_SEED, evaluation.key().as_ref()],
        bump = evaluation.vault_bump,
    )]
    pub stake_vault: Box<Account<'info, TokenAccount>>,

    /// The treasury's USDC account, pinned to the configured treasury so a caller cannot send
    /// the stake anywhere else.
    #[account(mut, token::mint = usdc_mint, token::authority = config.treasury)]
    pub treasury_token: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn forfeit_stake(ctx: Context<ForfeitStake>) -> Result<()> {
    require!(
        ctx.accounts.evaluation.state == EvaluationState::Failed,
        NoxError::EvaluationNotFailed
    );
    let amount = ctx.accounts.stake_vault.amount;
    // Refused once empty, rather than succeeding with nothing: a repeat call would otherwise emit
    // a zero-amount forfeit every time, and the event stream is what an indexer believes.
    require!(amount > 0, NoxError::ZeroAmount);
    move_stake(
        &ctx.accounts.evaluation,
        &ctx.accounts.stake_vault,
        &ctx.accounts.usdc_mint,
        ctx.accounts.treasury_token.to_account_info(),
        &ctx.accounts.token_program,
        amount,
    )?;
    emit!(StakeForfeited {
        evaluation: ctx.accounts.evaluation.key(),
        treasury: ctx.accounts.config.treasury,
        amount,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct AbandonEvaluation<'info> {
    pub trader: Signer<'info>,

    #[account(
        mut,
        seeds = [EVAL_SEED, trader.key().as_ref(), &[evaluation.seq]],
        bump = evaluation.bump,
        has_one = trader @ NoxError::NotTheTrader,
    )]
    pub evaluation: Box<Account<'info, Evaluation>>,
}

/// Walk away. The stake is forfeit — there is no time limit to run out, so without this an
/// abandoned evaluation would hold its stake in escrow forever.
pub fn abandon_evaluation(ctx: Context<AbandonEvaluation>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let key = ctx.accounts.evaluation.key();
    let e = &mut ctx.accounts.evaluation;
    require!(
        e.state == EvaluationState::Active,
        NoxError::EvaluationNotActive
    );
    let equity = Evaluation::floor_equity(i128::from(e.balance));
    fail(e, key, EvalRule::Abandoned, equity, now);
    Ok(())
}
