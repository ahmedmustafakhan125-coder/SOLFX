//! Liquidity provision (`ARCHITECTURE.md` § 5.4 LP, § 8.1 streams 4–5, threat T6).
//!
//! `add_liquidity` shipped in Phase 3 because the pool is the counterparty to every trade
//! (ADR-001) and a winning position is paid out of `lp_vault` — without a way to fund it,
//! Phase 3 could only test *losing* trades. Withdrawal waited for Phase 5 so it could arrive
//! with its defences rather than without them.
//!
//! # Exit is three instructions, not one
//!
//! ```text
//! request_remove_liquidity  ->  [cooldown]  ->  remove_liquidity
//!                            \
//!                             `-> cancel_remove_liquidity
//! ```
//!
//! **The cooldown is the defence, and the reason is worth being precise about.** Threat T6 is
//! the JIT attack: deposit ahead of a trader loss the pool is about to collect, withdraw
//! ahead of a gain it is about to pay, capture the edge without carrying the risk.
//!
//! The exit fee does not stop that — 0.05% is trivially outrun by a large enough known move.
//! What stops it is that **redemption is priced at NAV when it settles, not when it was
//! requested.** The attacker must hold through the whole cooldown, exposed to everything that
//! happens in it. The attack becomes "carry a day of risk", which is the risk they were
//! trying to avoid.
//!
//! Everything else follows from that: the request records a *time*, never a price.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount, Transfer};

use crate::constants::{
    FEE_VAULT_SEED, LP_MINT_SEED, LP_POOL_SEED, LP_VAULT_SEED, LP_WITHDRAW_SEED, PROTOCOL_SEED,
};
use crate::errors::IntoProgramResult;
use crate::errors::SolfxError;
use crate::events::{LiquidityAdded, LiquidityRemoved, WithdrawalCancelled, WithdrawalRequested};
use crate::state::{LpPool, LpWithdrawRequest, Protocol};

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
    let shares = solfx_math::lp::shares_for_deposit(amount, pool.aum, pool.lp_token_supply)
        .or_program_err()?;
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
    pool.total_deposited = pool
        .total_deposited
        .checked_add(amount)
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

// --- exit ---------------------------------------------------------------------------------

#[derive(Accounts)]
pub struct RequestRemoveLiquidity<'info> {
    #[account(mut)]
    pub provider: Signer<'info>,

    #[account(seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(mut, seeds = [LP_POOL_SEED], bump = lp_pool.bump)]
    pub lp_pool: Box<Account<'info, LpPool>>,

    /// One request per provider. A second one while the first is outstanding fails on
    /// `init` — which is the right behaviour: two requests would need two unlock clocks and
    /// let a provider keep a settled request permanently available by rolling it forward.
    #[account(
        init,
        payer = provider,
        space = 8 + LpWithdrawRequest::INIT_SPACE,
        seeds = [LP_WITHDRAW_SEED, provider.key().as_ref()],
        bump,
    )]
    pub withdraw_request: Box<Account<'info, LpWithdrawRequest>>,

    #[account(
        constraint = provider_lp_account.mint == lp_pool.lp_mint @ SolfxError::WrongCollateralMint,
        constraint = provider_lp_account.owner == provider.key() @ SolfxError::AuthorityMismatch,
    )]
    pub provider_lp_account: Box<Account<'info, TokenAccount>>,

    pub system_program: Program<'info, System>,
}

/// Start the cooldown clock.
///
/// Records **when** the request may settle and nothing about what it is worth. Pricing
/// happens in `remove_liquidity`, at whatever NAV the pool has then — see the module note.
///
/// Works while the protocol is paused. A pause stops new risk being taken; queueing an exit
/// takes none, and blocking it would mean a pause could trap LP capital.
pub fn request_remove_liquidity(
    ctx: Context<RequestRemoveLiquidity>,
    lp_amount: u64,
) -> Result<()> {
    require!(lp_amount > 0, SolfxError::ZeroAmount);
    require!(
        ctx.accounts.provider_lp_account.amount >= lp_amount,
        SolfxError::InsufficientLpShares
    );

    let clock = Clock::get()?;
    let pool = &mut ctx.accounts.lp_pool;

    let unlock_at = clock
        .unix_timestamp
        .checked_add(i64::from(pool.withdrawal_cooldown_seconds))
        .ok_or(SolfxError::MathOverflow)?;

    pool.pending_withdrawal_shares = pool
        .pending_withdrawal_shares
        .checked_add(lp_amount)
        .ok_or(SolfxError::MathOverflow)?;

    let request = &mut ctx.accounts.withdraw_request;
    request.authority = ctx.accounts.provider.key();
    request.shares = lp_amount;
    request.requested_at = clock.unix_timestamp;
    request.unlock_at = unlock_at;
    request.bump = ctx.bumps.withdraw_request;

    emit!(WithdrawalRequested {
        provider: ctx.accounts.provider.key(),
        shares: lp_amount,
        unlock_at,
        pending_shares_after: pool.pending_withdrawal_shares,
        ts: clock.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct CancelRemoveLiquidity<'info> {
    #[account(mut)]
    pub provider: Signer<'info>,

    #[account(mut, seeds = [LP_POOL_SEED], bump = lp_pool.bump)]
    pub lp_pool: Box<Account<'info, LpPool>>,

    #[account(
        mut,
        close = provider,
        seeds = [LP_WITHDRAW_SEED, provider.key().as_ref()],
        bump = withdraw_request.bump,
        has_one = authority @ SolfxError::AuthorityMismatch,
    )]
    pub withdraw_request: Box<Account<'info, LpWithdrawRequest>>,

    /// CHECK: matched against the request's recorded authority by `has_one`.
    #[account(address = provider.key() @ SolfxError::AuthorityMismatch)]
    pub authority: UncheckedAccount<'info>,
}

/// Abandon a pending exit and reclaim the request account's rent.
///
/// No penalty. Cancelling means staying in the pool and continuing to carry its risk, which
/// is the outcome the cooldown is trying to encourage — charging for it would push LPs to
/// follow through on exits they had changed their mind about.
pub fn cancel_remove_liquidity(ctx: Context<CancelRemoveLiquidity>) -> Result<()> {
    let shares = ctx.accounts.withdraw_request.shares;
    let pool = &mut ctx.accounts.lp_pool;
    pool.pending_withdrawal_shares = pool.pending_withdrawal_shares.saturating_sub(shares);

    emit!(WithdrawalCancelled {
        provider: ctx.accounts.provider.key(),
        shares,
        pending_shares_after: pool.pending_withdrawal_shares,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct RemoveLiquidity<'info> {
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

    #[account(mut, seeds = [FEE_VAULT_SEED], bump = protocol.fee_vault_bump)]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        close = provider,
        seeds = [LP_WITHDRAW_SEED, provider.key().as_ref()],
        bump = withdraw_request.bump,
        constraint = withdraw_request.authority == provider.key() @ SolfxError::AuthorityMismatch,
    )]
    pub withdraw_request: Box<Account<'info, LpWithdrawRequest>>,

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

/// Settle a matured exit: burn the shares, pay the USDC, keep the fees.
///
/// # Priced now, not at request time
///
/// `usdc = shares × aum / supply` at **today's** NAV. That is the anti-JIT property: the
/// provider carried the pool's risk for the whole cooldown, and is paid what the pool is
/// worth at the end of it rather than what it was worth when they lost their nerve.
///
/// # Two fees, going to two different places
///
/// * **Exit fee** stays in the pool, accruing to the LPs who stayed. § 8.1 lists it as
///   treasury revenue; `solfx_math::lp::exit_fee` documents why this protocol pays it to the
///   LPs being defended instead.
/// * **Performance fee** goes to the treasury. 10% of the gain above the high-water mark, and
///   nothing while the pool is under its previous peak.
///
/// # Withdrawals are not blocked by a pause
///
/// The same reasoning as trader collateral: a pause exists to stop new risk being taken, and
/// a protocol that could hold LP capital indefinitely is one nobody should provide liquidity
/// to. The cooldown already governs the timing.
pub fn remove_liquidity(ctx: Context<RemoveLiquidity>, min_usdc_out: u64) -> Result<()> {
    let clock = Clock::get()?;
    let request = &ctx.accounts.withdraw_request;

    require!(
        clock.unix_timestamp >= request.unlock_at,
        SolfxError::WithdrawalCooldownActive
    );

    let shares = request.shares;
    require!(
        ctx.accounts.provider_lp_account.amount >= shares,
        SolfxError::InsufficientLpShares
    );

    let pool = &ctx.accounts.lp_pool;
    require!(
        shares <= pool.lp_token_supply,
        SolfxError::InsufficientLpShares
    );

    let gross =
        solfx_math::lp::usdc_for_shares(shares, pool.aum, pool.lp_token_supply).or_program_err()?;

    let nav_before =
        solfx_math::lp::nav_per_share(pool.aum, pool.lp_token_supply).or_program_err()?;
    let performance_fee = solfx_math::lp::performance_fee(
        shares,
        nav_before,
        pool.high_water_mark_per_share,
        pool.performance_fee_bps,
    )
    .or_program_err()?;

    // The exit fee is waived when this redemption empties the pool.
    //
    // It exists to compensate the LPs who *stayed*. If there are none, there is nobody to
    // compensate — and leaving it behind would strand USDC in a pool with no shares against
    // it, which nobody could ever claim. Invariant I8 (`supply > 0 ⟺ aum > 0`) is exactly
    // the assertion that catches that, and it caught this.
    //
    // Not a JIT hole: escaping the fee requires being the only provider left, and the
    // cooldown — not the fee — is what defends against the attack anyway.
    let drains_the_pool = shares == pool.lp_token_supply;
    let exit_fee = if drains_the_pool {
        0
    } else {
        solfx_math::lp::exit_fee(gross, pool.exit_fee_bps).or_program_err()?
    };

    let total_fees = performance_fee
        .checked_add(exit_fee)
        .ok_or(SolfxError::MathOverflow)?;
    let payout = gross
        .checked_sub(total_fees)
        .ok_or(SolfxError::MathOverflow)?;

    require!(payout >= min_usdc_out, SolfxError::SlippageExceeded);
    // Saturating rather than checked: the comparison only needs to fail when the vault
    // cannot cover the outflow, and an overflowing sum is by definition more than it holds.
    require!(
        ctx.accounts.lp_vault.amount >= payout.saturating_add(performance_fee),
        SolfxError::InsufficientPoolLiquidity
    );

    // Burn first. A burn that fails must not leave USDC already paid out.
    token::burn(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            Burn {
                mint: ctx.accounts.lp_mint.to_account_info(),
                from: ctx.accounts.provider_lp_account.to_account_info(),
                authority: ctx.accounts.provider.to_account_info(),
            },
        ),
        shares,
    )?;

    let pool_seeds: &[&[&[u8]]] = &[&[LP_POOL_SEED, &[ctx.accounts.lp_pool.bump]]];

    token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            Transfer {
                from: ctx.accounts.lp_vault.to_account_info(),
                to: ctx.accounts.provider_token_account.to_account_info(),
                authority: ctx.accounts.lp_pool.to_account_info(),
            },
            pool_seeds,
        ),
        payout,
    )?;

    // The performance fee leaves the pool for the treasury. The exit fee does not move: it
    // stays in `lp_vault`, and simply not deducting it from `aum` is what credits it to the
    // remaining LPs.
    if performance_fee > 0 {
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                Transfer {
                    from: ctx.accounts.lp_vault.to_account_info(),
                    to: ctx.accounts.fee_vault.to_account_info(),
                    authority: ctx.accounts.lp_pool.to_account_info(),
                },
                pool_seeds,
            ),
            performance_fee,
        )?;
    }

    let pool = &mut ctx.accounts.lp_pool;
    pool.aum = pool
        .aum
        .checked_sub(payout)
        .ok_or(SolfxError::MathOverflow)?
        .checked_sub(performance_fee)
        .ok_or(SolfxError::MathOverflow)?;
    pool.lp_token_supply = pool
        .lp_token_supply
        .checked_sub(shares)
        .ok_or(SolfxError::MathOverflow)?;
    pool.pending_withdrawal_shares = pool.pending_withdrawal_shares.saturating_sub(shares);
    pool.total_performance_fees = pool
        .total_performance_fees
        .checked_add(performance_fee)
        .ok_or(SolfxError::MathOverflow)?;
    pool.total_exit_fees = pool
        .total_exit_fees
        .checked_add(exit_fee)
        .ok_or(SolfxError::MathOverflow)?;
    // Both the payout and the performance fee leave the pool; only the payout leaves the
    // protocol, but I7 measures the pool's own outflow so it counts what left `lp_vault`.
    pool.total_withdrawn = pool
        .total_withdrawn
        .checked_add(payout)
        .ok_or(SolfxError::MathOverflow)?;

    // Raise the high-water mark if the pool is at a new peak.
    //
    // Measured *after* the redemption, because the exit fee has just raised NAV per share for
    // everyone left and the mark should reflect what the pool is actually worth now.
    let nav_after =
        solfx_math::lp::nav_per_share(pool.aum, pool.lp_token_supply).or_program_err()?;
    if nav_after > pool.high_water_mark_per_share {
        pool.high_water_mark_per_share = nav_after;
    }

    emit!(LiquidityRemoved {
        provider: ctx.accounts.provider.key(),
        shares,
        gross,
        exit_fee,
        performance_fee,
        payout,
        aum_after: pool.aum,
        supply_after: pool.lp_token_supply,
        nav_per_share_after: nav_after,
        ts: clock.unix_timestamp,
    });
    Ok(())
}
