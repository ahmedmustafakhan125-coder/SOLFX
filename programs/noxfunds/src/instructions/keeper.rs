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

    // Three accounts per open position, and every open position must be present. Understating
    // equity would breach a healthy mandate; overstating it would hide a broken one. Neither
    // is acceptable, so the caller must bring them all.
    let expected = usize::from(ctx.accounts.mandate.open_positions)
        .checked_mul(3)
        .ok_or(NoxError::MathOverflow)?;
    require!(rem.len() == expected, NoxError::MathOverflow);

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
