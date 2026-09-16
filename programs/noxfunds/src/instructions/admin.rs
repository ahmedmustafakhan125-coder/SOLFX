//! Configuration. One account, written once, admin-updatable where the plan says so.

use anchor_lang::prelude::*;

use crate::constants::{CONFIG_SEED, DEFAULT_PROTOCOL_FEE_BPS};
use crate::errors::NoxError;
use crate::events::ConfigInitialized;
use crate::state::NoxConfig;

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + NoxConfig::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Box<Account<'info, NoxConfig>>,

    pub system_program: Program<'info, System>,
}

pub fn initialize_config(
    ctx: Context<InitializeConfig>,
    guardian: Pubkey,
    treasury: Pubkey,
    usdc_mint: Pubkey,
) -> Result<()> {
    let cfg = &mut ctx.accounts.config;
    cfg.admin = ctx.accounts.admin.key();
    cfg.guardian = guardian;
    cfg.treasury = treasury;
    // Fixed at the address of the venue this program was built against. A mandate cannot be
    // pointed at another program that happens to share an instruction layout.
    cfg.solfx_program = solfx_core::ID;
    cfg.usdc_mint = usdc_mint;
    cfg.protocol_fee_bps = DEFAULT_PROTOCOL_FEE_BPS;
    cfg.paused = false;
    cfg.bump = ctx.bumps.config;

    emit!(ConfigInitialized {
        admin: cfg.admin,
        treasury: cfg.treasury,
        protocol_fee_bps: cfg.protocol_fee_bps,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct SetPaused<'info> {
    pub authority: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,
}

/// Pause or unpause. The guardian may pause; only the admin may unpause.
///
/// Asymmetric on purpose, copying SolFX: a key that can stop the protocol in an emergency is
/// worth spreading around, and a key that can restart it is not.
pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
    let who = ctx.accounts.authority.key();
    let cfg = &mut ctx.accounts.config;
    let allowed = if paused {
        who == cfg.admin || who == cfg.guardian
    } else {
        who == cfg.admin
    };
    require!(allowed, NoxError::NotTheAdmin);
    cfg.paused = paused;
    Ok(())
}
