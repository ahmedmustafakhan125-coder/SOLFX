//! The position lifecycle (`ARCHITECTURE.md` § 5.4, § 6).
//!
//! Open, increase, decrease, close, and adjust margin. Every one of them:
//!
//! 1. reads a price through the single validated path ([`crate::oracle`]),
//! 2. prices the fill **adverse to the trader** (§ 6.6),
//! 3. converts everything into USDC (correction C-3),
//! 4. moves value between the four vaults conservatively ([`flows`]).
//!
//! # The property that matters most
//!
//! *Opening and immediately closing at an unchanged oracle price must always lose money.*
//! If it ever does not, there is free money in the protocol and bots will extract it until
//! the vault is empty. `solfx-math` property-tests it across the parameter space; the
//! integration suite proves it end to end on chain, on all three market shapes.

pub mod adjust_collateral;
pub mod close_position;
pub mod flows;
pub mod increase_position;
pub mod open_position;

pub use adjust_collateral::*;
pub use close_position::*;
pub use increase_position::*;
pub use open_position::*;

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Transfer};
use solfx_math::fees::FeeAllocation;

use crate::constants::{LP_POOL_SEED, PROTOCOL_SEED};
use crate::errors::SolfxError;
use crate::events::{FeeCollected, PoolSettlement};
use crate::state::{InsuranceFund, LpPool, Market, Protocol};
use flows::Flows;

/// The five accounts every settlement touches, plus the two PDAs that sign for them.
///
/// Passed as `AccountInfo` rather than typed accounts because the typed wrappers are
/// simultaneously borrowed mutably for their accounting fields; taking `to_account_info()`
/// up front is what lets both happen in one instruction.
pub struct VaultTransfer<'info> {
    pub token_program: AccountInfo<'info>,
    pub collateral_vault: AccountInfo<'info>,
    pub lp_vault: AccountInfo<'info>,
    pub insurance_vault: AccountInfo<'info>,
    pub fee_vault: AccountInfo<'info>,
    /// Signs for `collateral_vault`, `insurance_vault` and `fee_vault`.
    pub protocol: AccountInfo<'info>,
    pub protocol_bump: u8,
    /// Signs for `lp_vault`.
    pub lp_pool: AccountInfo<'info>,
    pub lp_pool_bump: u8,
}

impl<'info> VaultTransfer<'info> {
    fn move_out_of_collateral(&self, to: AccountInfo<'info>, amount: u64) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let seeds: &[&[&[u8]]] = &[&[PROTOCOL_SEED, &[self.protocol_bump]]];
        token::transfer(
            CpiContext::new_with_signer(
                self.token_program.key(),
                Transfer {
                    from: self.collateral_vault.clone(),
                    to,
                    authority: self.protocol.clone(),
                },
                seeds,
            ),
            amount,
        )
    }

    fn move_out_of_lp(&self, to: AccountInfo<'info>, amount: u64) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let seeds: &[&[&[u8]]] = &[&[LP_POOL_SEED, &[self.lp_pool_bump]]];
        token::transfer(
            CpiContext::new_with_signer(
                self.token_program.key(),
                Transfer {
                    from: self.lp_vault.clone(),
                    to,
                    authority: self.lp_pool.clone(),
                },
                seeds,
            ),
            amount,
        )
    }
}

/// Everything a settlement needs to know, gathered so the four call sites cannot disagree.
pub struct SettlementInput {
    pub fee: u64,
    /// Trader's realised PnL in USDC. Zero on open.
    pub pnl: i64,
    pub market_index: u16,
    pub user_account: Pubkey,
    pub referrer: Pubkey,
}

/// Execute a settlement: split the fee, net it against PnL, move the tokens, update every
/// accounting field, emit the events.
///
/// This is the only function in the program that moves USDC between vaults. One place to
/// audit, for the same reason [`crate::oracle`] is one place to read a price.
///
/// # Ordering
///
/// The pool is paid *before* it pays out, and its balance is checked against the payout
/// first. A pool that cannot cover a winning close must fail the transaction rather than
/// partially settle — Phase 4's insurance fund and ADL are what turn that failure into a
/// graceful degradation, and until they exist the honest behaviour is to stop.
#[allow(clippy::too_many_arguments)]
pub fn settle(
    input: SettlementInput,
    flows: Flows,
    alloc: FeeAllocation,
    transfer: &VaultTransfer<'_>,
    protocol: &mut Protocol,
    lp_pool: &mut LpPool,
    insurance: &mut InsuranceFund,
    market: &mut Market,
    lp_vault_balance: u64,
    now: i64,
) -> Result<()> {
    // --- token movements ---
    if flows.lp_delta >= 0 {
        let amount = u64::try_from(flows.lp_delta).map_err(|_| SolfxError::MathOverflow)?;
        transfer.move_out_of_collateral(transfer.lp_vault.clone(), amount)?;
    } else {
        let amount =
            u64::try_from(-i128::from(flows.lp_delta)).map_err(|_| SolfxError::MathOverflow)?;
        require!(
            lp_vault_balance >= amount,
            SolfxError::InsufficientPoolLiquidity
        );
        transfer.move_out_of_lp(transfer.collateral_vault.clone(), amount)?;
    }

    transfer.move_out_of_collateral(transfer.insurance_vault.clone(), flows.insurance_in)?;
    transfer.move_out_of_collateral(transfer.fee_vault.clone(), flows.fee_vault_in)?;

    // --- accounting, mirroring the movements exactly ---
    apply_signed(&mut lp_pool.aum, flows.lp_delta)?;
    insurance.balance = insurance
        .balance
        .checked_add(flows.insurance_in)
        .ok_or(SolfxError::MathOverflow)?;

    apply_signed(&mut protocol.total_user_collateral, flows.collateral_delta)?;
    protocol.total_referral_accrued = protocol
        .total_referral_accrued
        .checked_add(alloc.referral)
        .ok_or(SolfxError::MathOverflow)?;

    market.total_fees_collected = market
        .total_fees_collected
        .checked_add(input.fee)
        .ok_or(SolfxError::MathOverflow)?;

    if input.fee > 0 {
        emit!(FeeCollected {
            market_index: input.market_index,
            user_account: input.user_account,
            referrer: input.referrer,
            total: input.fee,
            lp: alloc.lp,
            treasury: alloc.treasury,
            insurance: alloc.insurance,
            referral: alloc.referral,
            ts: now,
        });
    }
    if input.pnl != 0 {
        emit!(PoolSettlement {
            market_index: input.market_index,
            amount: input.pnl,
            aum_after: lp_pool.aum,
            ts: now,
        });
    }

    Ok(())
}

/// Apply a signed delta to an unsigned balance, refusing to go negative.
///
/// An underflow here means the accounting has diverged from the token balances, which is a
/// broken invariant rather than an arithmetic edge case — so it surfaces as its own error.
fn apply_signed(balance: &mut u64, delta: i64) -> Result<()> {
    let next = i128::from(*balance)
        .checked_add(i128::from(delta))
        .ok_or(SolfxError::MathOverflow)?;
    require!(next >= 0, SolfxError::CollateralAccountingUnderflow);
    *balance = u64::try_from(next).map_err(|_| SolfxError::MathOverflow)?;
    Ok(())
}
