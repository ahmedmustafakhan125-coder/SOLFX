//! Liquidity provision — **the deposit half only**.
//!
//! # Why this is here and not in Phase 5
//!
//! The pool is the counterparty to every trade (ADR-001). A winning position is paid out of
//! `lp_vault`, so without a way to fund it Phase 3 could only ever test *losing* trades —
//! half the lifecycle, and the half that does not touch the pool's solvency path.
//!
//! So `add_liquidity` lands here. **Withdrawal does not**, and the split is deliberate rather
//! than convenient: `request_remove_liquidity` / `remove_liquidity` / `cancel` carry the
//! 24-hour cooldown and exit fee that defend against the JIT attack (threat T6), where an LP
//! deposits ahead of a known trader loss and withdraws ahead of a known gain. Shipping the
//! withdrawal path without its defences would be worse than not shipping it, so Phase 5 does
//! the whole thing at once.
//!
//! Until then liquidity is one-way. That is a safe direction to be incomplete in.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount, Transfer};

use crate::constants::{LP_MINT_SEED, LP_POOL_SEED, LP_VAULT_SEED, PROTOCOL_SEED};
use crate::errors::SolfxError;
use crate::events::LiquidityAdded;
use crate::state::{LpPool, Protocol};

#[derive(Accounts)]
pub struct AddLiquidity<'info> {
    #[account(mut)]
    pub provider: Signer<'info>,

    #[account(seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(mut, seeds = [LP_POOL_SEED], bump = lp_pool.bump)]
    pub lp_pool: Box<Account<'info, LpPool>>,

    #[account(mut, seeds = [LP_VAULT_SEED], bump = lp_pool.lp_vault_bump)]
    pub lp_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut, seeds = [LP_MINT_SEED], bump = lp_pool.lp_mint_bump)]
    pub lp_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        constraint = provider_token_account.mint == protocol.usdc_mint @ SolfxError::WrongCollateralMint,
        constraint = provider_token_account.owner == provider.key() @ SolfxError::AuthorityMismatch,
    )]
    pub provider_token_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = provider_lp_account.mint == lp_pool.lp_mint @ SolfxError::WrongCollateralMint,
        constraint = provider_lp_account.owner == provider.key() @ SolfxError::AuthorityMismatch,
    )]
    pub provider_lp_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

/// Deposit USDC and receive `slpUSD` pro rata.
///
/// # Share pricing
///
/// The first deposit mints 1:1, fixing the initial NAV per share at one dollar. Afterwards
/// `shares = amount × supply / aum`, **floored** — a depositor never receives more of the
/// pool than they paid for, so rounding accrues to the existing LPs rather than diluting
/// them. A deposit too small to mint a whole share is rejected rather than silently taken.
///
/// `aum` is used rather than the vault's token balance because the two are the same thing by
/// invariant I2, and reading the accounting figure is what makes a divergence *visible*
/// instead of self-correcting.
pub fn add_liquidity(ctx: Context<AddLiquidity>, amount: u64, min_lp_out: u64) -> Result<()> {
    require!(amount > 0, SolfxError::ZeroAmount);
    require!(!ctx.accounts.protocol.paused, SolfxError::ProtocolPaused);

    let pool = &ctx.accounts.lp_pool;
    let shares = if pool.lp_token_supply == 0 || pool.aum == 0 {
        amount
    } else {
        u64::try_from(
            u128::from(amount)
                .checked_mul(u128::from(pool.lp_token_supply))
                .ok_or(SolfxError::MathOverflow)?
                .checked_div(u128::from(pool.aum))
                .ok_or(SolfxError::DivideByZero)?,
        )
        .map_err(|_| SolfxError::MathOverflow)?
    };
    require!(shares > 0, SolfxError::ZeroLpShares);
    require!(shares >= min_lp_out, SolfxError::SlippageExceeded);

    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            Transfer {
                from: ctx.accounts.provider_token_account.to_account_info(),
                to: ctx.accounts.lp_vault.to_account_info(),
                authority: ctx.accounts.provider.to_account_info(),
            },
        ),
        amount,
    )?;

    let seeds: &[&[&[u8]]] = &[&[LP_POOL_SEED, &[ctx.accounts.lp_pool.bump]]];
    token::mint_to(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            MintTo {
                mint: ctx.accounts.lp_mint.to_account_info(),
                to: ctx.accounts.provider_lp_account.to_account_info(),
                authority: ctx.accounts.lp_pool.to_account_info(),
            },
            seeds,
        ),
        shares,
    )?;

    let pool = &mut ctx.accounts.lp_pool;
    pool.aum = pool
        .aum
        .checked_add(amount)
        .ok_or(SolfxError::MathOverflow)?;
    pool.lp_token_supply = pool
        .lp_token_supply
        .checked_add(shares)
        .ok_or(SolfxError::MathOverflow)?;

    emit!(LiquidityAdded {
        provider: ctx.accounts.provider.key(),
        usdc_in: amount,
        lp_tokens_out: shares,
        aum_after: pool.aum,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
