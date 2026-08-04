use anchor_lang::prelude::*;
use solfx_math::fees::FeeSplitBps;

use crate::constants::PROTOCOL_SEED;
use crate::errors::SolfxError;
use crate::events::{AdminTransferAccepted, AdminTransferInitiated, FeeSplitUpdated};
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
