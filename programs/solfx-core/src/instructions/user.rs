//! Trader account lifecycle and collateral movement.
//!
//! # Invariant I1 lives here
//!
//! `CollateralVault.amount == Σ(UserAccount.free) + Σ(Position.collateral)`
//! (`ARCHITECTURE.md` § 12.3). In Phase 2 there are no positions, so the vault balance must
//! equal the sum of free collateral. These two instructions are the only ones that move
//! USDC across the protocol boundary, so they are the only ones that can break it — every
//! token transfer here is paired with the matching balance update in the same instruction.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

use crate::constants::{COLLATERAL_VAULT_SEED, PROTOCOL_SEED, USER_SEED};
use crate::errors::SolfxError;
use crate::events::{CollateralDeposited, CollateralWithdrawn, UserAccountInitialized};
use crate::state::{Protocol, UserAccount};

#[derive(Accounts)]
pub struct InitializeUserAccount<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        init,
        payer = authority,
        space = 8 + UserAccount::INIT_SPACE,
        seeds = [USER_SEED, authority.key().as_ref()],
        bump,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,

    pub system_program: Program<'info, System>,
}

/// Create a trader's account and bind their referrer **permanently**.
///
/// The referrer is written once here and there is no instruction anywhere in the protocol
/// that changes it. That is the whole point: every introducing broker in retail FX has the
/// same complaint — the broker controls the ledger, and clients get reassigned. An
/// immutable binding plus a public rebate event is the structural fix, and it is the moat
/// (§ 8.5), because the smart contract is replicable in months and a network of IBs who
/// trust the ledger is not.
///
/// Self-referral is rejected. It would otherwise be a permanent fee rebate to oneself.
pub fn initialize_user_account(
    ctx: Context<InitializeUserAccount>,
    referrer: Pubkey,
) -> Result<()> {
    let authority = ctx.accounts.authority.key();
    require!(referrer != authority, SolfxError::InvalidParameter);

    let clock = Clock::get()?;
    let user = &mut ctx.accounts.user_account;

    user.authority = authority;
    user.free_collateral = 0;
    user.open_positions = 0;
    user.referrer = referrer;
    user.thirty_day_volume = 0;
    user.volume_window_start_ts = clock.unix_timestamp;
    user.total_deposits = 0;
    user.total_withdrawals = 0;
    user.created_at = clock.unix_timestamp;
    user.bump = ctx.bumps.user_account;

    emit!(UserAccountInitialized {
        user_account: user.key(),
        authority,
        referrer,
        ts: clock.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct MoveCollateral<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [USER_SEED, authority.key().as_ref()],
        bump = user_account.bump,
        has_one = authority @ SolfxError::AuthorityMismatch,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,

    /// Checked against `protocol.usdc_mint` in the handler. Fixing the mint at
    /// initialisation and comparing here means a token account for some other mint cannot be
    /// substituted in to credit collateral that the vault does not hold.
    #[account(mut)]
    pub collateral_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        seeds = [COLLATERAL_VAULT_SEED],
        bump = protocol.collateral_vault_bump,
        constraint = collateral_vault.mint == protocol.usdc_mint @ SolfxError::WrongCollateralMint,
    )]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = user_token_account.mint == protocol.usdc_mint @ SolfxError::WrongCollateralMint,
        constraint = user_token_account.owner == authority.key() @ SolfxError::AuthorityMismatch,
    )]
    pub user_token_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

/// Move USDC from the trader's wallet into the protocol's collateral vault.
///
/// Deposits are permitted while the protocol is paused. A pause exists to stop new risk
/// being taken, and adding collateral to an existing position reduces risk — blocking it
/// would push positions toward liquidation during exactly the incident the pause was called
/// for.
pub fn deposit_collateral(ctx: Context<MoveCollateral>, amount: u64) -> Result<()> {
    require!(amount > 0, SolfxError::ZeroAmount);
    require!(
        ctx.accounts.collateral_mint.key() == ctx.accounts.protocol.usdc_mint,
        SolfxError::WrongCollateralMint
    );

    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            Transfer {
                from: ctx.accounts.user_token_account.to_account_info(),
                to: ctx.accounts.collateral_vault.to_account_info(),
                authority: ctx.accounts.authority.to_account_info(),
            },
        ),
        amount,
    )?;

    let user = &mut ctx.accounts.user_account;
    user.credit(amount)?;
    user.total_deposits = user
        .total_deposits
        .checked_add(amount)
        .ok_or(SolfxError::MathOverflow)?;

    let protocol = &mut ctx.accounts.protocol;
    protocol.record_deposit(amount)?;

    emit!(CollateralDeposited {
        user_account: user.key(),
        authority: user.authority,
        amount,
        free_collateral_after: user.free_collateral,
        total_user_collateral_after: protocol.total_user_collateral,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

/// Move USDC from free collateral back to the trader's wallet.
///
/// # Why there is no margin check here
///
/// Margin is isolated (ADR-004): a position's collateral is moved *out* of `free_collateral`
/// and into its own `Position` account when it opens. So free collateral is genuinely
/// unencumbered, and `free_collateral >= amount` is the complete condition — § 5.4's
/// "blocked if it would breach margin" is satisfied by construction rather than by a check.
/// This is a property of isolated margin specifically and would not survive the move to
/// cross margin in v2.
///
/// # Why this works while paused
///
/// Withdrawing free collateral is the one operation that must never be blocked. "Your
/// collateral never leaves your own account" is the load-bearing claim of the whole product,
/// and a pause that could hold user funds would make it false.
pub fn withdraw_collateral(ctx: Context<MoveCollateral>, amount: u64) -> Result<()> {
    require!(amount > 0, SolfxError::ZeroAmount);
    require!(
        ctx.accounts.collateral_mint.key() == ctx.accounts.protocol.usdc_mint,
        SolfxError::WrongCollateralMint
    );

    let user = &mut ctx.accounts.user_account;
    user.debit(amount)?;
    user.total_withdrawals = user
        .total_withdrawals
        .checked_add(amount)
        .ok_or(SolfxError::MathOverflow)?;

    let protocol = &mut ctx.accounts.protocol;
    protocol.record_withdrawal(amount)?;

    let signer_seeds: &[&[&[u8]]] = &[&[PROTOCOL_SEED, &[protocol.bump]]];
    token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            Transfer {
                from: ctx.accounts.collateral_vault.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                authority: protocol.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
    )?;

    emit!(CollateralWithdrawn {
        user_account: ctx.accounts.user_account.key(),
        authority: ctx.accounts.user_account.authority,
        amount,
        free_collateral_after: ctx.accounts.user_account.free_collateral,
        total_user_collateral_after: ctx.accounts.protocol.total_user_collateral,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
