use anchor_lang::prelude::*;
use solfx_math::funding;

use crate::constants::{MARKET_SEED, PROTOCOL_SEED};
use crate::errors::{IntoProgramResult, SolfxError};
use crate::events::FundingUpdated;
use crate::state::{Direction, Market, Protocol};

#[derive(Accounts)]
pub struct CrankFunding<'info> {
    /// Permissionless. Funding that only the operator can advance is funding the operator
    /// can choose not to advance when it would cost them.
    pub keeper: Signer<'info>,

    #[account(seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,
}

/// Advance a market's funding and carry indices (`ARCHITECTURE.md` § 6.7).
///
/// # Why an index rather than a sweep
///
/// A Solana instruction cannot iterate every open position to charge funding — there is no
/// bound on how many there are. Instead the market accumulates monotonically, each position
/// snapshots the index when it opens, and the difference is what it owes. Accrual becomes
/// O(1) per position and the crank is O(1) per market.
///
/// # No price is read
///
/// Deliberately. Both rates derive from **open interest and configured interest rates**, not
/// from the mark — so the crank cannot be blocked by a stale or wide oracle, and it keeps
/// working through exactly the conditions where funding matters most. It is also why this
/// instruction is cheap enough to run every hour on every market.
///
/// # Carry and funding are different mechanisms
///
/// * **Carry** is interest on borrowed notional, charged in both directions, and it is
///   revenue: it flows to LPs and the treasury. Its index is unsigned and never decreases.
/// * **Funding** is a transfer *between traders* to correct book skew. It nets to zero and
///   must never touch LP capital (invariant I3). Its indices are signed and move opposite
///   ways.
pub fn crank_funding(ctx: Context<CrankFunding>) -> Result<()> {
    let clock = Clock::get()?;
    let market = &mut ctx.accounts.market;

    require!(
        market.status != crate::state::MarketStatus::Initialized,
        SolfxError::MarketNotActive
    );

    let elapsed = clock
        .unix_timestamp
        .checked_sub(market.last_funding_update_ts)
        .ok_or(SolfxError::MathOverflow)?;

    // A crank in the same second is a no-op rather than an error: keepers run redundantly by
    // design (§ 9.2 wants at least three instances), and racing ones must not fail each
    // other's transactions.
    if elapsed <= 0 {
        return Ok(());
    }

    // --- carry: the interest differential plus our markup, per direction ---
    //
    // The long and short rates are not mirror images. The differential flips sign between
    // them, but the markup is added to both, so both sides pay it — that is the revenue.
    let markup =
        u64::try_from(market.carry_rate_per_hour.max(0)).map_err(|_| SolfxError::MathOverflow)?;
    let long_carry_rate = funding::carry_cost_rate_per_hour(
        market.rate_base_annual,
        market.rate_quote_annual,
        markup,
        Direction::Long.into(),
    )
    .or_program_err()?;

    // One index for carry, advanced at the long-side rate.
    //
    // A single index cannot represent two different rates, and splitting it into two would
    // double the storage and the snapshot bookkeeping on every position. The long side is
    // the conservative choice when the differential is a cost to longs; when it favours
    // them, `advance_carry_index` clamps at zero rather than running backwards, so a short
    // is never charged less than the markup. Phase 5 revisits this if the differential ever
    // dominates the markup on a listed market.
    market.cum_borrow_index =
        funding::advance_carry_index(market.cum_borrow_index, long_carry_rate, elapsed)
            .or_program_err()?;

    // --- funding: skew correction between traders ---
    let rate = funding::funding_rate_per_hour(
        market.base_oi_long,
        market.base_oi_short,
        market.funding_rate_k,
        market.funding_rate_cap_per_hour,
    )
    .or_program_err()?;

    let update =
        funding::funding_index_update(market.base_oi_long, market.base_oi_short, rate, elapsed)
            .or_program_err()?;

    market.cum_funding_long = market
        .cum_funding_long
        .checked_add(update.long_delta)
        .ok_or(SolfxError::MathOverflow)?;
    market.cum_funding_short = market
        .cum_funding_short
        .checked_add(update.short_delta)
        .ok_or(SolfxError::MathOverflow)?;

    market.last_funding_update_ts = clock.unix_timestamp;

    emit!(FundingUpdated {
        market_index: market.market_index,
        funding_rate_per_hour: rate,
        carry_rate_per_hour: long_carry_rate,
        cum_funding_long: market.cum_funding_long,
        cum_funding_short: market.cum_funding_short,
        cum_borrow_index: market.cum_borrow_index,
        skew_ratio: funding::skew_ratio(market.base_oi_long, market.base_oi_short)
            .or_program_err()?,
        elapsed_seconds: elapsed,
        ts: clock.unix_timestamp,
    });

    Ok(())
}
