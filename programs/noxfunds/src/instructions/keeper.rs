//! The detective half of the rulebook.
//!
//! # Why some rules cannot be checked at trade time
//!
//! Three enforcement sites exist, and they are not interchangeable. In the CPI, before the
//! fill, anything derivable from *this trade* is preventive — the transaction fails and
//! nothing happened. At close, hold time and realised PnL are the same. But **a trader who is
//! losing can simply stop trading.** No trade-time check ever fires again, and the mandate
//! sits in drawdown indefinitely.
//!
//! So equity has to be sampled independently, by anyone, without the trader's cooperation.
//! That is what this is.
//!
//! # The honest limitation
//!
//! Drawdown is **observed, not continuous**. A spike between two observations is not caught.
//! The crank cadence is therefore a published parameter rather than an implementation detail,
//! and every observation is emitted — not only the ones that trip something.
//!
//! # Why `flag_breach` is folded in
//!
//! Part 6 lists `observe_mandate_equity` and `flag_breach` separately. They are one
//! instruction here. A separate transition that re-derives the same equity is a second place
//! for the two to disagree about whether a mandate is breached, and the disagreement would
//! surface as a mandate that observes as broken and refuses to be flagged.

use anchor_lang::prelude::*;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_core::state::{Market, Position, UserAccount};

use crate::constants::{CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED};
use crate::errors::NoxError;
use crate::events::{EquityObserved, MandateBreached};
use crate::state::{Mandate, MandateState, NoxConfig};

#[derive(Accounts)]
pub struct ObserveMandateEquity<'info> {
    /// **Anyone.** Investor capital must never be hostage to an absent trader or an absent
    /// operator, so the instruction that can free it takes no privileged signer.
    pub observer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// CHECK: bound to this mandate by the address constraint; only its balance is read.
    #[account(seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()], bump = mandate.signer_bump)]
    pub mandate_signer: SystemAccount<'info>,

    #[account(address = mandate.solfx_user_account)]
    pub user_account: Box<Account<'info, UserAccount>>,
    // Open positions arrive in `remaining_accounts` as (position, market, price_update)
    // triples. Passing them as named optionals would fix the count at compile time; a mandate
    // may hold up to `max_concurrent_positions`, which is a per-mandate figure.
}

/// Observe a mandate's equity, update its high-water mark, and breach it if it has fallen too
/// far.
///
/// `remaining_accounts` carries **(position, market, price_update)** triples — one per open
/// position. A caller who supplies fewer than the mandate holds understates equity, which can
/// only ever breach a mandate early rather than let a broken one continue; the count is
/// checked against `open_positions` so that cannot happen silently.
pub fn observe_mandate_equity(ctx: Context<ObserveMandateEquity>) -> Result<()> {
    let clock = Clock::get()?;
    let rem = ctx.remaining_accounts;

    // Three accounts per open position. The length check is only the first gate — the real
    // one is below, where each triple must match a distinct open slot.
    require!(rem.len().is_multiple_of(3), NoxError::IncompleteObservation);
    let slots = ctx.accounts.mandate.slots;
    let mut matched: u16 = 0;

    // Free collateral sitting in SolFX, plus the equity of every open position marked on the
    // **adverse** side — the same number a liquidation would use, because it comes from the
    // same function.
    let mut equity = i128::from(ctx.accounts.user_account.free_collateral);

    for triple in rem.chunks_exact(3) {
        // Destructured rather than indexed. `chunks_exact(3)` guarantees the length, but the
        // workspace denies `indexing_slicing` and is right to: a slice pattern proves the
        // length to the compiler instead of to the reader.
        let [position_info, market_info, price_info] = triple else {
            return Err(NoxError::MathOverflow.into());
        };
        let position: Account<Position> = Account::try_from(position_info)?;
        let market: Account<Market> = Account::try_from(market_info)?;
        let price_update: Account<PriceUpdateV2> = Account::try_from(price_info)?;

        require!(
            position.user_account == ctx.accounts.user_account.key(),
            NoxError::NotTheTrader
        );

        // **Each open position exactly once.** An earlier version checked only the number of
        // triples, so one profitable position supplied twice satisfied it — inflating equity
        // and hiding a breach from the one rule that exists to catch it. Matching against the
        // slots, and refusing a slot already matched, closes that.
        let index = slots
            .iter()
            .position(|s| {
                s.open && s.market_index == position.market_index && s.nonce == position.nonce
            })
            .ok_or(NoxError::IncompleteObservation)?;
        let bit = 1u16
            .checked_shl(u32::try_from(index).map_err(|_| NoxError::MathOverflow)?)
            .ok_or(NoxError::MathOverflow)?;
        require!(matched & bit == 0, NoxError::IncompleteObservation);
        matched |= bit;
        require!(
            position.market_index == market.market_index,
            NoxError::MarketNotPermitted
        );

        let price =
            solfx_core::oracle::load_validated_price(&market, &price_update, None, None, &clock)?;
        let health = solfx_core::risk::assess(&position, &market, &price)?;
        equity = equity
            .checked_add(i128::from(health.equity))
            .ok_or(NoxError::MathOverflow)?;
    }

    // And none left out: omitting a losing position would overstate equity just as surely as
    // supplying a winner twice.
    let open = slots.iter().filter(|s| s.open).count();
    require!(
        usize::try_from(matched.count_ones()).map_err(|_| NoxError::MathOverflow)? == open,
        NoxError::IncompleteObservation
    );

    // A mandate underwater past its collateral reads as zero rather than negative: equity is
    // what could be recovered, and it cannot be less than nothing.
    let equity = u64::try_from(equity.max(0)).map_err(|_| NoxError::MathOverflow)?;

    let m = &mut ctx.accounts.mandate;
    if equity > m.peak_equity {
        m.peak_equity = equity;
    }
    m.last_equity = equity;
    m.last_observed_at = clock.unix_timestamp;

    let drawdown_bps = m.drawdown_bps(equity);

    emit!(EquityObserved {
        mandate: m.key(),
        equity,
        peak_equity: m.peak_equity,
        drawdown_bps,
        open_positions: m.open_positions,
        observer: ctx.accounts.observer.key(),
        ts: clock.unix_timestamp,
    });

    // The transition. Only from `Active`: a mandate already breached or winding down does not
    // get breached twice, and re-emitting would make the event stream lie about when it broke.
    if m.state == MandateState::Active && drawdown_bps > u64::from(m.max_drawdown_bps) {
        m.state = MandateState::Breached;
        emit!(MandateBreached {
            mandate: m.key(),
            trader: m.trader,
            rule: NoxError::DrawdownExceeded as u32,
            equity,
            peak_equity: m.peak_equity,
            drawdown_bps,
            ts: clock.unix_timestamp,
        });
    }

    Ok(())
}

// --- Stage 6: getting a mandate to settlement without anyone's cooperation -----------------
//
// Part 10's exit criterion: *a breached mandate winds down with **neither** the trader nor the
// operator cooperating.* Breach detection above gets a mandate to `Breached`. These three get it
// from there to flat, so the permissionless `claim_settlement` can pay the investor.

use anchor_spl::token::Token;
use solfx_core::constants::POSITION_SEED;
use solfx_core::state::Direction;

use crate::events::{PositionReconciled, PositionWoundDown};
use crate::instructions::trading::{close_via_cpi, leg};
use crate::SolfxCore;

#[derive(Accounts)]
pub struct WindDownPosition<'info> {
    /// **Anyone.** No relation to the mandate is required or checked.
    pub closer: Signer<'info>,

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
    /// Deserialized: its direction decides the slippage bound, and its market and nonce decide
    /// which slot to release. Bound to this mandate's SolFX account.
    #[account(mut, constraint = position.user_account == mandate.solfx_user_account @ NoxError::PositionNotTracked)]
    pub position: Box<Account<'info, Position>>,
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

    /// Validated by `solfx-core` against the market's feed id and its staleness gate, so an
    /// untrusted caller cannot supply a stale or foreign price to close at.
    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub token_program: Program<'info, Token>,
    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

/// Close one position of a breached or winding-down mandate, on anyone's signature.
///
/// # Why the caller does not choose the slippage bound
///
/// Accepting one would let an untrusted caller either refuse to close (a bound no fill can
/// meet) or be blamed for a bad one. The bound is derived here instead — the loosest the
/// venue accepts for the position's direction — exactly as `solfx-core` does for a firing stop,
/// where a bound "would mean a stop that silently fails to close in exactly the fast market it
/// was placed for". The fill itself is still the venue's oracle-derived execution price; the
/// caller controls only *when*, and the mandate is already stopped by the time they can act.
///
/// No minimum-hold check: that rule governs a trader's voluntary closes, and this is neither.
pub fn wind_down_position(ctx: Context<WindDownPosition>) -> Result<()> {
    require!(
        matches!(
            ctx.accounts.mandate.state,
            MandateState::Breached | MandateState::WindingDown
        ),
        NoxError::MandateNotWindingDown
    );

    let market_index = ctx.accounts.position.market_index;
    let nonce = ctx.accounts.position.nonce;
    // A long closes by selling, so its bound is a minimum; a short closes by buying.
    let unbounded = match ctx.accounts.position.direction {
        Direction::Long => 1,
        Direction::Short => i64::MAX,
    };

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    let a = &ctx.accounts;
    close_via_cpi(
        a.solfx_core_program.key(),
        solfx_core::cpi::accounts::ClosePosition {
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
        unbounded,
    )?;

    let m = &mut ctx.accounts.mandate;
    m.unbook(market_index, nonce)?;
    emit!(PositionWoundDown {
        mandate: mandate_key,
        market_index,
        nonce,
        open_positions: m.open_positions,
        closer: ctx.accounts.closer.key(),
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(market_index: u16, nonce: u8)]
pub struct ReconcilePosition<'info> {
    /// **Anyone.**
    pub caller: Signer<'info>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// CHECK: the address is derived below from this mandate's SolFX account, the market and
    /// the nonce, under `solfx-core`'s program id — so it can only be the position this slot
    /// describes. Only whether it still exists is read.
    #[account(
        seeds = [
            POSITION_SEED,
            mandate.solfx_user_account.as_ref(),
            &market_index.to_le_bytes(),
            &[nonce],
        ],
        seeds::program = solfx_core_program.key(),
        bump,
    )]
    pub position: UncheckedAccount<'info>,

    #[account(address = solfx_core::ID)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

/// Release a slot whose position closed without NOXFUNDS seeing it.
///
/// # The hole this closes
///
/// A stop fires through `solfx-core`'s own `execute_trigger_order`, which never enters this
/// program. The position is gone, but the mandate still counts it — and before this existed,
/// that one stop-out left a mandate permanently unable to settle or even be observed. Callable
/// in any state, because a stop-out can happen to an `Active` mandate too.
///
/// # Why this cannot be abused to hide a position
///
/// The position address is derived, not supplied, and the slot is released only if that
/// account no longer holds a SolFX position. A live position cannot be reconciled away.
///
/// Its bump is found at runtime rather than stored, because the account it describes may no
/// longer exist to store one. That costs roughly 1,500 CU and is paid only here.
pub fn reconcile_position(
    ctx: Context<ReconcilePosition>,
    market_index: u16,
    nonce: u8,
) -> Result<()> {
    let info = ctx.accounts.position.to_account_info();
    let gone = info.data_is_empty() || *info.owner != solfx_core::ID;
    require!(gone, NoxError::PositionStillOpen);

    let m = &mut ctx.accounts.mandate;
    let released = m.unbook(market_index, nonce)?;
    emit!(PositionReconciled {
        mandate: m.key(),
        market_index,
        nonce,
        notional_released: released,
        open_positions: m.open_positions,
        caller: ctx.accounts.caller.key(),
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct WindDownCancelStop<'info> {
    /// **Anyone.**
    pub closer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: validated by `solfx-core`, which also checks its authority is the mandate signer.
    #[account(mut)]
    pub trigger_order: UncheckedAccount<'info>,

    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

/// Cancel a resting stop on a mandate that has stopped trading, returning its rent to the
/// mandate signer. Without this an absent trader's orders would hold 2,039,280 lamports each
/// indefinitely.
pub fn wind_down_cancel_stop(ctx: Context<WindDownCancelStop>) -> Result<()> {
    require!(
        ctx.accounts.mandate.state != MandateState::Active,
        NoxError::MandateNotWindingDown
    );
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
    ))
}
