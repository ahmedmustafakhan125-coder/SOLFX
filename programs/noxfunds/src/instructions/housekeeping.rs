//! Housekeeping: recovering a settled mandate's SOL, and rotating the keys that run the protocol.
//!
//! # The SOL a mandate signer was left holding
//!
//! The mandate signer pays the rent for its SolFX account and for every stop, and is refunded as
//! they close. What is left after settlement used to be unrecoverable: the signer is a PDA with no
//! key, and nothing swept it — about 0.02 SOL per mandate, stated in `NOXFUNDS.md` §10 rather
//! than fixed. `sweep_mandate_signer` returns it to the trader, whose dashboard is what funds it
//! (`prepareMandateIxs` in `app/src/lib/nox.ts`). It is a System Program transfer signed with the
//! signer's seeds — the documented way to move lamports out of a dataless PDA.
//!
//! # Rotating admin, guardian and treasury
//!
//! These could only change by a program upgrade (`NOXFUNDS.md` §11). The guardian and the
//! treasury are now set by the admin directly. The admin itself moves in **two steps** — the
//! current admin proposes, the new one accepts by signing — so a mistyped address cannot hand the
//! protocol to a key nobody holds. The same split SolFX uses for its own admin.

use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Transfer};

use crate::constants::{CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED};
use crate::errors::NoxError;
use crate::events::{ConfigKeyChanged, MandateSignerSwept};
use crate::state::{Mandate, MandateState, NoxConfig};

// --- sweeping a settled mandate's signer --------------------------------------------------------

#[derive(Accounts)]
pub struct SweepMandateSigner<'info> {
    /// Anyone. Recovering the trader's own SOL should not wait on the trader.
    pub caller: Signer<'info>,

    #[account(
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
        constraint = mandate.state == MandateState::Settled @ NoxError::MandateNotSettled,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: receives the lamports. Constrained to the mandate's trader, so a caller cannot
    /// redirect them.
    #[account(mut, address = mandate.trader @ NoxError::NotTheTrader)]
    pub trader: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Send everything a settled mandate's signer holds to its trader. **Permissionless.**
///
/// Only once `Settled`: until then the signer pays for stops, and emptying it would leave a live
/// position unable to carry one. A settled mandate never trades again, so draining it to zero is
/// safe — the runtime reclaims an empty system account, and anything sent to it later can be
/// swept again.
pub fn sweep_mandate_signer(ctx: Context<SweepMandateSigner>) -> Result<()> {
    let amount = ctx.accounts.mandate_signer.lamports();
    require!(amount > 0, NoxError::ZeroAmount);

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];
    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.key(),
            Transfer {
                from: ctx.accounts.mandate_signer.to_account_info(),
                to: ctx.accounts.trader.to_account_info(),
            },
            seeds,
        ),
        amount,
    )?;

    emit!(MandateSignerSwept {
        mandate: mandate_key,
        trader: ctx.accounts.trader.key(),
        lamports: amount,
        caller: ctx.accounts.caller.key(),
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

// --- rotating the protocol's keys ---------------------------------------------------------------

/// Which key a `ConfigKeyChanged` event is about.
pub mod key {
    pub const GUARDIAN: u8 = 0;
    pub const TREASURY: u8 = 1;
    pub const ADMIN_PROPOSED: u8 = 2;
    pub const ADMIN_ACCEPTED: u8 = 3;
}

#[derive(Accounts)]
pub struct AdminConfig<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = admin @ NoxError::NotTheAdmin,
    )]
    pub config: Box<Account<'info, NoxConfig>>,
}

/// Replace the guardian — the key that may pause, and may not move funds or unpause.
pub fn set_guardian(ctx: Context<AdminConfig>, guardian: Pubkey) -> Result<()> {
    require!(guardian != Pubkey::default(), NoxError::InvalidKey);
    let cfg = &mut ctx.accounts.config;
    let old = cfg.guardian;
    cfg.guardian = guardian;
    emit_change(key::GUARDIAN, old, guardian)
}

/// Replace the treasury — where performance fees and forfeited stakes go from now on.
pub fn set_treasury(ctx: Context<AdminConfig>, treasury: Pubkey) -> Result<()> {
    require!(treasury != Pubkey::default(), NoxError::InvalidKey);
    let cfg = &mut ctx.accounts.config;
    let old = cfg.treasury;
    cfg.treasury = treasury;
    emit_change(key::TREASURY, old, treasury)
}

/// Step one of handing over the admin: name the successor. Nothing changes until they accept,
/// and proposing again replaces the proposal — the way to withdraw a mistaken one.
pub fn propose_admin(ctx: Context<AdminConfig>, new_admin: Pubkey) -> Result<()> {
    let cfg = &mut ctx.accounts.config;
    let old = cfg.pending_admin;
    cfg.pending_admin = new_admin;
    emit_change(key::ADMIN_PROPOSED, old, new_admin)
}

#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    /// The proposed successor, proving they hold the key by signing.
    pub new_admin: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.pending_admin == new_admin.key() @ NoxError::NotThePendingAdmin,
        constraint = config.pending_admin != Pubkey::default() @ NoxError::NotThePendingAdmin,
    )]
    pub config: Box<Account<'info, NoxConfig>>,
}

/// Step two: the proposed admin takes over, and the proposal is cleared.
pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
    let cfg = &mut ctx.accounts.config;
    let old = cfg.admin;
    cfg.admin = cfg.pending_admin;
    cfg.pending_admin = Pubkey::default();
    emit_change(key::ADMIN_ACCEPTED, old, cfg.admin)
}

fn emit_change(key: u8, old: Pubkey, new: Pubkey) -> Result<()> {
    emit!(ConfigKeyChanged {
        key,
        old,
        new,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
