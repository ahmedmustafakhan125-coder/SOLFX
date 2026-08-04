//! The guardian can only ever **restrict**. It cannot move funds, change a risk parameter,
//! or unpause the protocol.
//!
//! That asymmetry is the point (ADR-008). Halting is time-critical — an oracle dislocation
//! does not wait for a 3-of-5 multisig to assemble — so the key that halts must be hot.
//! A hot key is a key that will eventually be compromised, so the blast radius of holding it
//! is capped at "trading stops", which is recoverable. Unpausing requires the admin, so a
//! stolen guardian key cannot restart a protocol the real operators deliberately stopped.

use anchor_lang::prelude::*;

use crate::constants::{MARKET_SEED, PROTOCOL_SEED};
use crate::errors::SolfxError;
use crate::events::{MarketStatusChanged, ProtocolPauseChanged};
use crate::state::{Market, MarketStatus, Protocol};

#[derive(Accounts)]
pub struct GuardianPause<'info> {
    pub guardian: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        has_one = guardian @ SolfxError::NotGuardian,
    )]
    pub protocol: Box<Account<'info, Protocol>>,
}

/// Global kill switch.
///
/// Blocks opens. **Withdrawals of free collateral and liquidations stay live**, and that is
/// deliberate: freezing a user's own money is exactly the failure mode the non-custodial
/// claim exists to rule out, and a pause that also stopped liquidations would convert an
/// incident into bad debt.
pub fn emergency_pause(ctx: Context<GuardianPause>) -> Result<()> {
    ctx.accounts.protocol.paused = true;

    emit!(ProtocolPauseChanged {
        paused: true,
        actor: ctx.accounts.guardian.key(),
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct GuardianHaltMarket<'info> {
    pub guardian: Signer<'info>,

    #[account(
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        has_one = guardian @ SolfxError::NotGuardian,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,
}

/// Halt one market. Positions freeze; liquidations of already-underwater positions continue.
pub fn halt_market(ctx: Context<GuardianHaltMarket>) -> Result<()> {
    let market = &mut ctx.accounts.market;
    let old_status = market.status;
    market.status = MarketStatus::Halted;

    emit!(MarketStatusChanged {
        market_index: market.market_index,
        old_status: old_status as u8,
        new_status: MarketStatus::Halted as u8,
        actor: 1, // guardian
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct AdminUnpause<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        has_one = admin @ SolfxError::NotAdmin,
    )]
    pub protocol: Box<Account<'info, Protocol>>,
}

/// Resume trading. **Admin only** — see the module note on why the guardian cannot do this.
pub fn unpause(ctx: Context<AdminUnpause>) -> Result<()> {
    ctx.accounts.protocol.paused = false;

    emit!(ProtocolPauseChanged {
        paused: false,
        actor: ctx.accounts.admin.key(),
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
