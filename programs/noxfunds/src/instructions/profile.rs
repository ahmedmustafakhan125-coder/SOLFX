//! The track record, and the tier it earns.
//!
//! # Why the record lives here and not on SolFX
//!
//! SolFX stores no per-account PnL, trade count, win count or drawdown. Those figures exist
//! only in its event stream, and a program cannot read events. Every statistic an investor
//! judges a trader on therefore has to be accumulated by the instruction that closes the
//! trade — which is why `funded_close_position` writes to this account and why it reads the
//! realised PnL out of the venue rather than recomputing it.
//!
//! # Why the tier can fall
//!
//! `recompute_tier` sets the tier to whatever the statistics currently say, in both
//! directions, and anyone may call it. A tier that only ever ratchets upward rewards getting
//! lucky once and then stopping; one that can fall is the thing that makes a marketplace
//! listing mean something a month later.

use anchor_lang::prelude::*;

use crate::constants::TRADER_SEED;
use crate::events::{TierChanged, TraderProfileCreated};
use crate::state::{TraderProfile, TraderTier};

#[derive(Accounts)]
pub struct InitializeTraderProfile<'info> {
    /// Pays the rent. Need not be the trader — an operator or an investor may open a profile
    /// for someone, because the profile confers nothing until trades land on it.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: the trader this record belongs to. It is a seed of the PDA, so the address is
    /// the binding; nothing is read from the account and it signs nothing.
    pub authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = payer,
        space = 8 + TraderProfile::INIT_SPACE,
        seeds = [TRADER_SEED, authority.key().as_ref()],
        bump,
    )]
    pub profile: Box<Account<'info, TraderProfile>>,

    pub system_program: Program<'info, System>,
}

pub fn initialize_trader_profile(ctx: Context<InitializeTraderProfile>) -> Result<()> {
    let ts = Clock::get()?.unix_timestamp;
    let p = &mut ctx.accounts.profile;
    p.authority = ctx.accounts.authority.key();
    // Bronze is the floor, not an achievement: it means a record exists and nothing has been
    // proven on it yet. Everything above it is earned by closed trades.
    p.tier = TraderTier::Bronze;
    p.created_at = ts;
    p.bump = ctx.bumps.profile;

    emit!(TraderProfileCreated {
        profile: p.key(),
        authority: p.authority,
        ts,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct RecomputeTier<'info> {
    /// **Anyone.** The computation is pure and the inputs are all on the account, so there is
    /// nothing a caller could bias by choosing when to run it — and an investor who suspects a
    /// listing is stale should not have to ask the trader to refresh it.
    pub caller: Signer<'info>,

    #[account(
        mut,
        seeds = [TRADER_SEED, profile.authority.as_ref()],
        bump = profile.bump,
    )]
    pub profile: Box<Account<'info, TraderProfile>>,
}

/// Set the tier to whatever the record currently earns, up or down.
pub fn recompute_tier(ctx: Context<RecomputeTier>) -> Result<()> {
    let p = &mut ctx.accounts.profile;
    let from = p.tier;
    let to = TraderTier::for_stats(p);
    if from == to {
        // Not an error. A crank that finds nothing to do should cost a caller a transaction
        // fee and nothing else, and emitting an unchanged `TierChanged` would make the event
        // stream unusable for exactly the audience it exists for.
        return Ok(());
    }
    p.tier = to;

    emit!(TierChanged {
        profile: p.key(),
        authority: p.authority,
        from: from as u8,
        to: to as u8,
        trades: p.trades,
        profit_factor_bps: p.profit_factor_bps(),
        win_rate_bps: p.win_rate_bps(),
        max_drawdown_bps: p.max_drawdown_bps,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
