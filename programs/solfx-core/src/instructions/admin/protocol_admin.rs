use anchor_lang::prelude::*;
use solfx_math::fees::FeeSplitBps;

use crate::constants::{FEE_VAULT_SEED, PROTOCOL_SEED};
use crate::errors::SolfxError;
use crate::events::{
    AdminTransferAccepted, AdminTransferInitiated, FeeSplitUpdated, TreasuryFeesWithdrawn,
};
use crate::state::Protocol;

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        has_one = admin @ SolfxError::NotAdmin,
    )]
    pub protocol: Box<Account<'info, Protocol>>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct FeeSplitParams {
    pub lp_bps: u16,
    pub treasury_bps: u16,
    pub insurance_bps: u16,
    pub referral_bps: u16,
}

pub fn update_fee_splits(ctx: Context<AdminOnly>, params: FeeSplitParams) -> Result<()> {
    let split = FeeSplitBps {
        lp: params.lp_bps,
        treasury: params.treasury_bps,
        insurance: params.insurance_bps,
        referral: params.referral_bps,
    };
    split.validate().map_err(|_| SolfxError::InvalidFeeSplit)?;
    require!(split.lp > split.treasury, SolfxError::InvalidFeeSplit);

    ctx.accounts.protocol.set_fee_split(split)?;

    emit!(FeeSplitUpdated {
        lp_bps: split.lp,
        treasury_bps: split.treasury,
        insurance_bps: split.insurance,
        referral_bps: split.referral,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

/// Step one of a two-step authority handover.
///
/// Two steps because a single-step transfer to a mistyped address is unrecoverable: the
/// protocol would be left with an admin key nobody holds, and every risk parameter frozen
/// at whatever it was. The new authority must prove it can sign before it takes effect.
///
/// Passing `Pubkey::default()` cancels a pending transfer.
pub fn transfer_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
    let protocol = &mut ctx.accounts.protocol;
    protocol.pending_admin = new_admin;

    emit!(AdminTransferInitiated {
        current_admin: protocol.admin,
        pending_admin: new_admin,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    pub pending_admin: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
    )]
    pub protocol: Box<Account<'info, Protocol>>,
}

/// Step two: the nominated authority claims the role.
pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
    let protocol = &mut ctx.accounts.protocol;

    require!(
        protocol.pending_admin != Pubkey::default(),
        SolfxError::NoPendingAdmin
    );
    require!(
        protocol.pending_admin == ctx.accounts.pending_admin.key(),
        SolfxError::NotPendingAdmin
    );

    let old_admin = protocol.admin;
    protocol.admin = protocol.pending_admin;
    protocol.pending_admin = Pubkey::default();

    emit!(AdminTransferAccepted {
        old_admin,
        new_admin: protocol.admin,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

/// Rotate the pause-only key. Admin-only: the guardian cannot replace itself, which keeps a
/// compromised hot key from making itself permanent.
pub fn set_guardian(ctx: Context<AdminOnly>, new_guardian: Pubkey) -> Result<()> {
    require!(
        new_guardian != Pubkey::default(),
        SolfxError::InvalidParameter
    );
    ctx.accounts.protocol.guardian = new_guardian;
    Ok(())
}

#[derive(Accounts)]
pub struct WithdrawTreasuryFees<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        has_one = admin @ SolfxError::NotAdmin,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(mut, seeds = [FEE_VAULT_SEED], bump = protocol.fee_vault_bump)]
    pub fee_vault: Box<Account<'info, anchor_spl::token::TokenAccount>>,

    #[account(
        mut,
        constraint = destination.mint == protocol.usdc_mint @ SolfxError::WrongCollateralMint,
    )]
    pub destination: Box<Account<'info, anchor_spl::token::TokenAccount>>,

    pub token_program: Program<'info, anchor_spl::token::Token>,
}

/// Move accumulated treasury fees out of the protocol.
///
/// # What this can and cannot reach
///
/// **Only `fee_vault`.** The collateral vault, the LP vault and the insurance vault are all
/// out of reach, because there is no instruction anywhere that lets the admin sign a transfer
/// from them — that is the structural form of the non-custodial claim, and it has to be true
/// by construction rather than by policy.
///
/// `fee_vault` holds the treasury's own share of fees, already split off at collection
/// (§ 8.3). Sweeping it moves no user, LP or insurance money.
pub fn withdraw_treasury_fees(ctx: Context<WithdrawTreasuryFees>, amount: u64) -> Result<()> {
    require!(amount > 0, SolfxError::ZeroAmount);
    require!(
        ctx.accounts.fee_vault.amount >= amount,
        SolfxError::InsufficientPoolLiquidity
    );

    let seeds: &[&[&[u8]]] = &[&[PROTOCOL_SEED, &[ctx.accounts.protocol.bump]]];
    anchor_spl::token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            anchor_spl::token::Transfer {
                from: ctx.accounts.fee_vault.to_account_info(),
                to: ctx.accounts.destination.to_account_info(),
                authority: ctx.accounts.protocol.to_account_info(),
            },
            seeds,
        ),
        amount,
    )?;

    let protocol = &mut ctx.accounts.protocol;
    protocol.total_treasury_withdrawn = protocol
        .total_treasury_withdrawn
        .checked_add(amount)
        .ok_or(SolfxError::MathOverflow)?;

    emit!(TreasuryFeesWithdrawn {
        destination: ctx.accounts.destination.key(),
        amount,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

/// Register (or clear) the referral programme's authority.
///
/// Admin-only, and the only knowledge core ever has of the IB programme. Setting
/// `Pubkey::default()` switches referral payouts off — which is also the launch state,
/// since a referral program that has not been deployed cannot be trusted with a signature.
pub fn set_referral_authority(ctx: Context<AdminOnly>, authority: Pubkey) -> Result<()> {
    ctx.accounts.protocol.referral_authority = authority;
    Ok(())
}

#[derive(Accounts)]
pub struct PayReferral<'info> {
    /// The referral programme's PDA, signing via CPI. Checked against the pubkey the admin
    /// registered — a signature from anyone else is not a rebate.
    pub referral_authority: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        constraint = protocol.referral_authority != Pubkey::default() @ SolfxError::ReferralDisabled,
        constraint = protocol.referral_authority == referral_authority.key() @ SolfxError::NotReferralAuthority,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(mut, seeds = [FEE_VAULT_SEED], bump = protocol.fee_vault_bump)]
    pub fee_vault: Box<Account<'info, anchor_spl::token::TokenAccount>>,

    #[account(
        mut,
        constraint = destination.mint == protocol.usdc_mint @ SolfxError::WrongCollateralMint,
    )]
    pub destination: Box<Account<'info, anchor_spl::token::TokenAccount>>,

    pub token_program: Program<'info, anchor_spl::token::Token>,
}

/// Release accrued referral money from the fee vault. **Called only by `solfx-referral`.**
///
/// # What this instruction deliberately does not know
///
/// Nothing about tiers, IB accounts, sub-broker overrides or who is owed what. Core holds
/// the money and enforces one arithmetic constraint; the referral programme owns the policy
/// and can be rewritten, re-audited or replaced without touching a line of the financial
/// core (§ 5.1).
///
/// # The constraint that matters
///
/// ```text
/// total_referral_claimed + amount <= total_referral_accrued
/// ```
///
/// The fee vault holds treasury money as well as referral money. Without this check a bug
/// — or a compromised referral program — could drain the treasury through a door built for
/// rebates. With it, the worst a broken referral programme can do is misallocate the
/// referral pool *among IBs*: bad, visible in the event stream, and bounded by what
/// referred traders actually generated.
pub fn pay_referral(ctx: Context<PayReferral>, amount: u64) -> Result<()> {
    require!(amount > 0, SolfxError::ZeroAmount);

    let protocol = &ctx.accounts.protocol;
    let claimed_after = protocol
        .total_referral_claimed
        .checked_add(amount)
        .ok_or(SolfxError::MathOverflow)?;
    require!(
        claimed_after <= protocol.total_referral_accrued,
        SolfxError::ReferralClaimExceedsAccrual
    );
    require!(
        ctx.accounts.fee_vault.amount >= amount,
        SolfxError::InsufficientPoolLiquidity
    );

    let seeds: &[&[&[u8]]] = &[&[PROTOCOL_SEED, &[protocol.bump]]];
    anchor_spl::token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            anchor_spl::token::Transfer {
                from: ctx.accounts.fee_vault.to_account_info(),
                to: ctx.accounts.destination.to_account_info(),
                authority: ctx.accounts.protocol.to_account_info(),
            },
            seeds,
        ),
        amount,
    )?;

    let protocol = &mut ctx.accounts.protocol;
    protocol.total_referral_claimed = claimed_after;

    emit!(crate::events::ReferralPaid {
        destination: ctx.accounts.destination.key(),
        amount,
        total_claimed: claimed_after,
        total_accrued: protocol.total_referral_accrued,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
