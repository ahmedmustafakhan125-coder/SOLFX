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
use solfx_core::state::{Market, Position, PriceSource, UserAccount};

use anchor_spl::token::TokenAccount;

use crate::constants::{
    BPS, CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED, MANDATE_VAULT_SEED, TRADER_SEED,
};
use crate::errors::NoxError;
use crate::events::{EquityObserved, MandateBreached, TradeRecorded};
use crate::state::{Mandate, MandateState, NoxConfig, TraderProfile, MAX_SLOTS};
use crate::venue::read_user_account;

const SECS_PER_DAY: i64 = 86_400;

#[derive(Accounts)]
pub struct ObserveMandateEquity<'info> {
    /// **Anyone.** Investor capital must never be hostage to an absent trader or an absent
    /// operator, so the instruction that can free it takes no privileged signer.
    pub observer: Signer<'info>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// CHECK: bound to the mandate. It may not exist yet — a mandate is observable from the moment
    /// it is funded, before anyone has created its SolFX account — and is read through
    /// `venue::read_user_account`, which refuses anything that is not a SolFX `UserAccount`.
    #[account(address = mandate.solfx_user_account)]
    pub user_account: UncheckedAccount<'info>,

    /// The trader's record, so the worst drawdown ever *observed* lands on it.
    ///
    /// Required rather than optional, and this is the load-bearing reason the crank exists at
    /// all: a trader holding a losing position and refusing to close it would otherwise never
    /// record the drawdown, because nothing but a close writes to the profile. Marking it here
    /// means the gaming vector that matters most costs the trader their tier whether they close
    /// or not.
    #[account(
        mut,
        seeds = [TRADER_SEED, mandate.trader.as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == mandate.trader @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    /// The mandate's own vault. **Principal not yet moved into SolFX is still the investor's
    /// equity**, and leaving it out made a freshly funded mandate read as a 100% drawdown that any
    /// stranger could breach — permanently, with the 100% written onto the trader's record
    /// (internal review R-2).
    ///
    /// Bound by its seeds and stored bump alone. That identifies the one token account
    /// `fund_mandate` or `accept_offer` created, with the configured mint and the mandate signer
    /// as authority; a token account's mint cannot change, and only that signer — which this
    /// program never uses for `SetAuthority` — could change its authority. So `config` and
    /// `mandate_signer`, which were here only to restate those facts, are no longer passed: the
    /// crank must fit every open position in one transaction, and each account costs 32 bytes.
    #[account(seeds = [MANDATE_VAULT_SEED, mandate.key().as_ref()], bump = mandate.vault_bump)]
    pub mandate_vault: Box<Account<'info, TokenAccount>>,
    // Open positions arrive in `remaining_accounts`, one group each. Passing them as named
    // optionals would fix the count at compile time; a mandate may hold up to
    // `max_concurrent_positions`, which is a per-mandate figure.
}

/// Observe a mandate's equity, update its high-water mark, and breach it if it has fallen too
/// far.
///
/// `remaining_accounts` carries one group per open position: **(position, market,
/// price_update)**, then the secondary update if the market is synthetic and the conversion
/// update if it is not USD-quoted. The group's length is read from the market account, never
/// chosen by the caller, and every leg is checked against the market's own feed ids by
/// `load_validated_price`. A caller who supplies fewer positions than the mandate holds is
/// refused rather than allowed to understate or overstate equity — see the slot matching below.
pub fn observe_mandate_equity(ctx: Context<ObserveMandateEquity>) -> Result<()> {
    let clock = Clock::get()?;
    let rem = ctx.remaining_accounts;

    // One group per open position: the position, its market and its primary price — then the
    // secondary leg if the market is synthetic, and the quote-conversion leg if it is not quoted
    // in USD. The market decides how many legs follow, exactly as `load_validated_price` does.
    //
    // It was fixed triples, and `load_validated_price` was called with no extra legs. That made
    // any mandate holding a synthetic or non-USD-quoted position — EUR/JPY and USD/INR are both
    // listed on devnet — impossible to observe: the read failed with a missing-leg error, so the
    // drawdown rule could not be judged for as long as that position stayed open.
    let slots = ctx.accounts.mandate.slots;
    let mut matched: u16 = 0;

    // The vault, plus free collateral sitting in SolFX, plus the equity of every open position
    // marked on the **adverse** side — the same number a liquidation would use, because it comes
    // from the same function. No SolFX account yet means no collateral and no positions there.
    let user_key = ctx.accounts.user_account.key();
    let free = read_user_account(&ctx.accounts.user_account)?.map_or(0, |v| v.free_collateral);
    let mut equity = i128::from(free)
        .checked_add(i128::from(ctx.accounts.mandate_vault.amount))
        .ok_or(NoxError::MathOverflow)?;

    // An iterator rather than indexing: the workspace denies `indexing_slicing`, and a group's
    // length is only known once its market has been read.
    let mut accounts = rem.iter();
    while let Some(position_info) = accounts.next() {
        let market_info = accounts.next().ok_or(NoxError::IncompleteObservation)?;
        let price_info = accounts.next().ok_or(NoxError::IncompleteObservation)?;
        let position: Account<Position> = Account::try_from(position_info)?;
        let market: Account<Market> = Account::try_from(market_info)?;
        let price_update: Account<PriceUpdateV2> = Account::try_from(price_info)?;
        let secondary: Option<Account<PriceUpdateV2>> =
            if matches!(market.price_source, PriceSource::Synthetic { .. }) {
                let info = accounts.next().ok_or(NoxError::IncompleteObservation)?;
                Some(Account::try_from(info)?)
            } else {
                None
            };
        let quote: Option<Account<PriceUpdateV2>> = if market.needs_quote_conversion() {
            let info = accounts.next().ok_or(NoxError::IncompleteObservation)?;
            Some(Account::try_from(info)?)
        } else {
            None
        };

        require!(position.user_account == user_key, NoxError::NotTheTrader);

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

        let price = solfx_core::oracle::load_validated_price(
            &market,
            &price_update,
            secondary.as_deref(),
            quote.as_deref(),
            &clock,
        )?;
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

    // A new UTC day opens at the last equity seen before it — the most recent figure known to
    // belong to an earlier day — or at the high-water mark if nothing has been observed yet.
    // Rolled before `last_equity` is overwritten below, or the baseline would be this very
    // observation and a loss taken before it would never count.
    let today = clock.unix_timestamp.div_euclid(SECS_PER_DAY);
    if today != m.day {
        m.day = today;
        m.day_start_equity = if m.last_equity > 0 {
            m.last_equity
        } else {
            m.peak_equity
        };
    }

    if equity > m.peak_equity {
        m.peak_equity = equity;
    }
    m.last_equity = equity;
    m.last_observed_at = clock.unix_timestamp;

    let drawdown_bps = m.drawdown_bps(equity);
    let daily_loss_bps = daily_loss_bps(m.day_start_equity, equity);

    emit!(EquityObserved {
        mandate: m.key(),
        equity,
        peak_equity: m.peak_equity,
        drawdown_bps,
        open_positions: m.open_positions,
        observer: ctx.accounts.observer.key(),
        ts: clock.unix_timestamp,
    });

    // The worst drawdown the record has ever seen, across every mandate this trader has held.
    // Saturating at `u16::MAX` rather than wrapping: `drawdown_bps` returns `u64::MAX` on an
    // arithmetic failure it treats as "breached", and that must not come back as a small number.
    let observed = u16::try_from(drawdown_bps).unwrap_or(u16::MAX);
    let profile = &mut ctx.accounts.trader_profile;
    profile.max_drawdown_bps = profile.max_drawdown_bps.max(observed);

    let m = &mut ctx.accounts.mandate;

    // The transition. Only from `Active`: a mandate already breached or winding down does not
    // get breached twice, and re-emitting would make the event stream lie about when it broke.
    //
    // Two rules, drawdown first. `max_daily_loss_bps` used to be validated at funding and never
    // read again — an investor who set a 3% daily limit had no such protection (internal review
    // R-6). The event's `rule` names which one broke.
    let broke = if drawdown_bps > u64::from(m.max_drawdown_bps) {
        Some(NoxError::DrawdownExceeded)
    } else if daily_loss_bps > u64::from(m.max_daily_loss_bps) {
        Some(NoxError::DailyLossExceeded)
    } else {
        None
    };
    if let (MandateState::Active, Some(rule)) = (m.state, broke) {
        m.state = MandateState::Breached;
        emit!(MandateBreached {
            mandate: m.key(),
            trader: m.trader,
            rule: rule as u32,
            equity,
            peak_equity: m.peak_equity,
            drawdown_bps,
            ts: clock.unix_timestamp,
        });
    }

    Ok(())
}

/// Loss since the day began, in bps of the day's opening equity. Ceiling: this is a limit the
/// mandate must stay under, so rounding down would miss a breach by a unit.
fn daily_loss_bps(day_start: u64, equity: u64) -> u64 {
    if day_start == 0 || equity >= day_start {
        return 0;
    }
    solfx_math::fixed::mul_div_ceil(
        u128::from(day_start.saturating_sub(equity)),
        u128::from(BPS),
        u128::from(day_start),
    )
    .and_then(solfx_math::fixed::to_u64)
    .unwrap_or(u64::MAX)
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
    /// Deserialized and reloaded after the CPI, exactly as on the trader's own close: the change
    /// in free collateral across the close is what the trade made.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: Box<Account<'info, UserAccount>>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub market: UncheckedAccount<'info>,
    /// Deserialized: its direction decides the slippage bound, and its market and nonce decide
    /// which slot to release. Bound to this mandate's SolFX account.
    #[account(mut, constraint = position.user_account == mandate.solfx_user_account @ NoxError::PositionNotTracked)]
    pub position: Box<Account<'info, Position>>,
    /// The trader's record. A forced close is still the trader's trade, and it was the one kind
    /// the record never saw (internal review R-3).
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
    let free_before = ctx.accounts.user_account.free_collateral;
    let margin = ctx.accounts.position.collateral;
    let open_fee = ctx.accounts.position.open_fee_paid;
    let held = Clock::get()?
        .slot
        .saturating_sub(ctx.accounts.position.opened_at_slot);
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

    // The same measurement as `funded_close_position`: what came back, less the margin it
    // released and the fee it paid to open.
    ctx.accounts.user_account.reload()?;
    let credit = ctx
        .accounts
        .user_account
        .free_collateral
        .checked_sub(free_before)
        .ok_or(NoxError::MathOverflow)?;
    let realized = i64::try_from(
        i128::from(credit)
            .checked_sub(i128::from(margin))
            .and_then(|d| d.checked_sub(i128::from(open_fee)))
            .ok_or(NoxError::MathOverflow)?,
    )
    .map_err(|_| NoxError::MathOverflow)?;

    let profile = &mut ctx.accounts.trader_profile;
    profile.record_trade(realized, held)?;

    let ts = Clock::get()?.unix_timestamp;
    let m = &mut ctx.accounts.mandate;
    m.unbook(market_index, nonce)?;
    m.release_close(margin, open_fee, credit, realized)?;
    emit!(TradeRecorded {
        profile: profile.key(),
        mandate: mandate_key,
        trader: m.trader,
        market_index,
        nonce,
        realized_pnl: realized,
        hold_slots: held,
        trades: profile.trades,
        wins: profile.wins,
        losses: profile.losses,
        gross_profit: profile.gross_profit,
        gross_loss: profile.gross_loss,
        ts,
    });
    emit!(PositionWoundDown {
        mandate: mandate_key,
        market_index,
        nonce,
        open_positions: m.open_positions,
        closer: ctx.accounts.closer.key(),
        ts,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ReconcilePosition<'info> {
    /// **Anyone.**
    pub caller: Signer<'info>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// SolFX's own account of this mandate: how much free collateral it holds, and how many
    /// positions it still has open. Those two numbers are what makes the result recoverable.
    #[account(address = mandate.solfx_user_account)]
    pub user_account: Box<Account<'info, UserAccount>>,

    #[account(
        mut,
        seeds = [TRADER_SEED, mandate.trader.as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == mandate.trader @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,
    // `remaining_accounts`: the position account of **every** open slot, in slot order.
}

/// Account for positions that closed where NOXFUNDS could not see them — a stop, a take-profit,
/// a liquidation — and put their result on the trader's record.
///
/// # What it used to do, and why that was not enough
///
/// It released the slot and recorded nothing. Every funded trade carries a stop, so the ordinary
/// way a losing trade ends was the one way it never reached the record: a trader who let losers
/// stop out showed 100% wins and an unbounded profit factor, and tier — which sets how much
/// capital they may take — is decided on those figures (internal review R-3).
///
/// # How the result is recovered after the position account is gone
///
/// `last_free_collateral` moves only by NOXFUNDS' own effects, so `free_collateral` minus it is
/// exactly what external closes have credited. `booked_margin_fees` minus what the still-open
/// positions hold is exactly what the closed ones cost to open. The difference is their net
/// result, to the unit — the same figure a voluntary close records.
///
/// # When two closed together
///
/// SolFX's own `open_positions` says how many are gone. If it is one, the attribution is exact.
/// If it is more, the combined result is exact but how it divides between them is not knowable,
/// and a hedged pair — a long's take-profit and a short's stop firing on the same move — could
/// otherwise net a loss away. So the split is resolved **against the trader**, the rule every
/// rounding in this codebase follows: a combined loss is recorded in full, a combined profit is
/// not credited, and each such trade is counted in `ambiguous_trades` for anyone to see.
///
/// # Why this cannot hide a live position
///
/// Every open slot's position is supplied, at an address derived here from the slot, and SolFX's
/// own count of open positions must equal the number found alive. Nothing can be omitted, and
/// nothing can be passed in another's place.
pub fn reconcile_position(ctx: Context<ReconcilePosition>) -> Result<()> {
    let user_key = ctx.accounts.user_account.key();
    let free_now = ctx.accounts.user_account.free_collateral;
    let solfx_open = ctx.accounts.user_account.open_positions;
    let slots = ctx.accounts.mandate.slots;

    let mut supplied = ctx.remaining_accounts.iter();
    let mut gone = [(0u16, 0u8); MAX_SLOTS];
    let mut gone_count: usize = 0;
    let mut live_count: u16 = 0;
    let mut live_booked: u64 = 0;

    for slot in slots.iter().filter(|s| s.open) {
        let info = supplied.next().ok_or(NoxError::IncompleteObservation)?;
        // Found at runtime rather than stored: the account may no longer exist to hold a bump.
        let (expected, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                user_key.as_ref(),
                &slot.market_index.to_le_bytes(),
                &[slot.nonce],
            ],
            &solfx_core::ID,
        );
        require_keys_eq!(info.key(), expected, NoxError::IncompleteObservation);

        if info.data_is_empty() || *info.owner != solfx_core::ID {
            let entry = gone
                .get_mut(gone_count)
                .ok_or(NoxError::IncompleteObservation)?;
            *entry = (slot.market_index, slot.nonce);
            gone_count = gone_count.checked_add(1).ok_or(NoxError::MathOverflow)?;
        } else {
            // The derived address already binds it to this account, market and nonce.
            let p: Account<Position> = Account::try_from(info)?;
            live_booked = live_booked
                .checked_add(
                    p.collateral
                        .checked_add(p.open_fee_paid)
                        .ok_or(NoxError::MathOverflow)?,
                )
                .ok_or(NoxError::MathOverflow)?;
            live_count = live_count.checked_add(1).ok_or(NoxError::MathOverflow)?;
        }
    }
    require!(supplied.next().is_none(), NoxError::IncompleteObservation);
    require!(gone_count > 0, NoxError::PositionStillOpen);
    // SolFX and NOXFUNDS must agree on what is still open, or the attribution below is unsound.
    require!(solfx_open == live_count, NoxError::IncompleteObservation);

    let m = &mut ctx.accounts.mandate;
    let credit = i128::from(free_now)
        .checked_sub(i128::from(m.last_free_collateral))
        .ok_or(NoxError::MathOverflow)?;
    let gone_booked = m
        .booked_margin_fees
        .checked_sub(live_booked)
        .ok_or(NoxError::MathOverflow)?;
    let total = i64::try_from(
        credit
            .checked_sub(i128::from(gone_booked))
            .ok_or(NoxError::MathOverflow)?,
    )
    .map_err(|_| NoxError::MathOverflow)?;

    // One recorded result per closed position. Exact when there is one; resolved against the
    // trader when there are several — see the doc comment.
    let mut results = [0i64; MAX_SLOTS];
    if gone_count == 1 || total <= 0 {
        if let Some(first) = results.first_mut() {
            *first = total;
        }
    }
    let ambiguous = gone_count > 1;

    let profile = &mut ctx.accounts.trader_profile;
    let count = u32::try_from(gone_count).map_err(|_| NoxError::MathOverflow)?;
    profile.untimed_trades = profile.untimed_trades.saturating_add(count);
    if ambiguous {
        profile.ambiguous_trades = profile.ambiguous_trades.saturating_add(count);
    }

    let ts = Clock::get()?.unix_timestamp;
    let mandate_key = m.key();
    let caller = ctx.accounts.caller.key();
    for (&(market_index, nonce), &result) in gone.iter().zip(results.iter()).take(gone_count) {
        profile.record_trade(result, 0)?;
        let released = m.unbook(market_index, nonce)?;
        emit!(TradeRecorded {
            profile: profile.key(),
            mandate: mandate_key,
            trader: m.trader,
            market_index,
            nonce,
            realized_pnl: result,
            hold_slots: 0,
            trades: profile.trades,
            wins: profile.wins,
            losses: profile.losses,
            gross_profit: profile.gross_profit,
            gross_loss: profile.gross_loss,
            ts,
        });
        emit!(PositionReconciled {
            mandate: mandate_key,
            market_index,
            nonce,
            notional_released: released,
            open_positions: m.open_positions,
            caller,
            ts,
        });
    }

    // Every external close is now accounted for — SolFX's count proved there are no others — so
    // this is the one place re-reading free collateral is exact rather than a way to absorb one.
    m.booked_margin_fees = live_booked;
    m.last_free_collateral = i64::try_from(free_now).map_err(|_| NoxError::MathOverflow)?;
    // The mandate's money is real whatever the attribution, so it counts at its true total.
    m.realized_pnl = m
        .realized_pnl
        .checked_add(total)
        .ok_or(NoxError::MathOverflow)?;
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

    /// Deserialized for its kind and position; see `funded_cancel_stop`.
    #[account(mut)]
    pub trigger_order: Box<Account<'info, solfx_core::state::TriggerOrder>>,

    /// CHECK: the position the order protects, bound by the order's own record of it.
    #[account(address = trigger_order.position)]
    pub position: UncheckedAccount<'info>,

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
    // The same rule as the trader's own cancel: a stopped mandate's open positions keep their
    // stops until someone closes them.
    crate::instructions::trading::require_stop_unprotected(
        &ctx.accounts.trigger_order,
        &ctx.accounts.position,
    )?;
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
