//! Configuration. One account, written once, admin-updatable where the plan says so.

use anchor_lang::prelude::*;

use crate::constants::{CONFIG_SEED, DEFAULT_PROTOCOL_FEE_BPS};
use crate::errors::NoxError;
use crate::events::ConfigInitialized;
use crate::program::Noxfunds;
use crate::state::NoxConfig;

/// One-time setup, and **only the program's upgrade authority may perform it**.
///
/// # Why the signer is checked against the loader rather than against a stored admin
///
/// `config` is a singleton PDA, so whoever creates it first owns it forever: they become
/// `admin`, and they choose `treasury` — where the 5% performance fee on every mandate lands.
/// With a plain `Signer` here, a freshly deployed program is an open invitation to anyone
/// watching the chain for new deployments, and there is no stored admin to check against,
/// because storing the admin is exactly what this instruction does.
///
/// The way out of that loop is the one account that already names an authority before this
/// program has run at all: the loader's `ProgramData`, which records who may upgrade it. This
/// is the pattern Anchor documents on `Account<ProgramData>` for precisely this problem. Two
/// constraints, and both are necessary:
///
/// - `program` must point at `program_data`. Without it a caller could pass *any* upgradeable
///   program's data account — one whose upgrade authority they hold — and satisfy the second.
/// - `program_data` must name the signer as its upgrade authority.
///
/// A program deployed **immutable** has no upgrade authority, so nobody can initialise it.
/// That is the correct failure: initialise before finalising, never after.
///
/// # Field order does not make the guard run earlier
///
/// Anchor 1.1.2's generated `try_accounts` creates every `init` account first and evaluates the
/// other constraints afterwards, whatever order the fields are declared in (read from
/// `anchor-syn` `generate_constraints`). So a refused caller's `config` is created and then
/// rolled back. What protects the configuration is that the whole transaction reverts, not
/// that the check comes first — moving these fields above `config` would change nothing.
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

    /// This program's own account. Its only job is to name the real `program_data`.
    #[account(
        constraint = program.programdata_address()? == Some(program_data.key())
            @ NoxError::NotTheUpgradeAuthority,
    )]
    pub program: Program<'info, Noxfunds>,

    /// The loader's record of who may upgrade this program. Anchor checks it is owned by the
    /// upgradeable loader and is genuinely a `ProgramData` account.
    #[account(
        constraint = program_data.upgrade_authority_address == Some(admin.key())
            @ NoxError::NotTheUpgradeAuthority,
    )]
    pub program_data: Box<Account<'info, ProgramData>>,

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
