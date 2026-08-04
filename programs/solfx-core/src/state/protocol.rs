use anchor_lang::prelude::*;
use solfx_math::fees::FeeSplitBps;

use crate::errors::SolfxError;

/// Global configuration. Singleton at `["protocol"]`.
///
/// Holds the three authorities, the global kill switch, the fee split, and the running
/// collateral total that invariant I1 is checked against.
#[account]
#[derive(InitSpace)]
pub struct Protocol {
    /// Squads multisig on mainnet, a single keypair on devnet (ADR-008).
    pub admin: Pubkey,
    /// Set by `transfer_admin`, cleared by `accept_admin`. Two-step so a typo in a new
    /// authority cannot orphan the protocol.
    pub pending_admin: Pubkey,
    /// Pause-only key. Can restrict, never move funds. Held hot precisely because it is
    /// the one key that is safe to hold hot.
    pub guardian: Pubkey,
    /// Collateral mint. Fixed at initialisation and never mutable — changing it would
    /// orphan every deposit already in the vault.
    pub usdc_mint: Pubkey,

    /// Next market index to assign. Also the count of markets ever created.
    pub num_markets: u16,
    /// Global kill switch. Blocks opens; withdrawals of free collateral and liquidations
    /// stay live, because freezing a user's own money is the thing we promise not to do.
    pub paused: bool,

    // --- fee split, in bps, must total 10_000 (§ 8.3) ---
    pub fee_split_lp_bps: u16,
    pub fee_split_treasury_bps: u16,
    pub fee_split_insurance_bps: u16,
    pub fee_split_referral_bps: u16,

    /// Every USDC unit the vault holds on behalf of users: free collateral plus (from
    /// Phase 3) position margin.
    ///
    /// Invariant I1 (`ARCHITECTURE.md` § 12.3) is
    /// `CollateralVault.amount == Σ(UserAccount.free) + Σ(Position.collateral)`.
    /// This field is the third leg of that comparison: the sums are computed by walking
    /// accounts off-chain, while this is maintained incrementally on-chain. Checking all
    /// three against each other catches drift that checking only two would hide.
    pub total_user_collateral: u64,

    /// Cumulative deposits and withdrawals, for invariant I7.
    pub total_deposits: u64,
    pub total_withdrawals: u64,

    pub bump: u8,
    pub collateral_vault_bump: u8,
    pub fee_vault_bump: u8,

    // --- appended in Phase 3 -----------------------------------------------------------
    // Paid for out of `_reserved`, which shrank from 128 bytes to 120. Every field above
    // keeps its byte offset and the account's total size is unchanged (§ 5.6).
    /// Referral share accrued across all trades, sitting in the fee vault.
    ///
    /// The IB programme (Phase 6, `solfx-referral`) claims against this. Tracked here rather
    /// than inferred from events because a rebate ledger an IB has to reconstruct from logs
    /// is exactly the ledger they cannot audit — which is the complaint the whole programme
    /// exists to answer (§ 8.5).
    pub total_referral_accrued: u64,
    /// Lifetime referral paid out. Phase 6 increments this on claim.
    pub total_referral_claimed: u64,
    /// Cumulative shortfall where a position closed owing more than its collateral.
    ///
    /// Phase 3 has no liquidation engine, so a position can run past the point its margin
    /// covers. The trader's loss is capped at what they posted and the remainder is recorded
    /// here rather than silently absorbed — § 6.9's waterfall (insurance, then ADL, then LP
    /// NAV) is Phase 4's job, and it needs a number to reconcile against.
    pub total_bad_debt: u64,
    /// Cumulative USDC paid out to liquidators.
    ///
    /// This is the only value that leaves the protocol other than a trader withdrawal, so
    /// invariant I7 — *every USDC the program holds is accounted for* — needs it as a term.
    pub total_liquidator_paid: u64,

    /// Forward-compatible padding (§ 5.6). New fields consume these bytes; nothing already
    /// on chain shifts. Append only — never reorder, never retype.
    pub _reserved: [u8; 104],
}

impl Protocol {
    pub fn fee_split(&self) -> FeeSplitBps {
        FeeSplitBps {
            lp: self.fee_split_lp_bps,
            treasury: self.fee_split_treasury_bps,
            insurance: self.fee_split_insurance_bps,
            referral: self.fee_split_referral_bps,
        }
    }

    pub fn set_fee_split(&mut self, split: FeeSplitBps) -> Result<()> {
        split.validate().map_err(SolfxError::from)?;
        self.fee_split_lp_bps = split.lp;
        self.fee_split_treasury_bps = split.treasury;
        self.fee_split_insurance_bps = split.insurance;
        self.fee_split_referral_bps = split.referral;
        Ok(())
    }

    /// Credit a deposit to the running totals.
    pub fn record_deposit(&mut self, amount: u64) -> Result<()> {
        self.total_user_collateral = self
            .total_user_collateral
            .checked_add(amount)
            .ok_or(SolfxError::MathOverflow)?;
        self.total_deposits = self
            .total_deposits
            .checked_add(amount)
            .ok_or(SolfxError::MathOverflow)?;
        Ok(())
    }

    /// Debit a withdrawal from the running totals.
    ///
    /// A `checked_sub` failure here means the vault accounting has diverged from the token
    /// balance, so it surfaces as its own error rather than as a generic overflow.
    pub fn record_withdrawal(&mut self, amount: u64) -> Result<()> {
        self.total_user_collateral = self
            .total_user_collateral
            .checked_sub(amount)
            .ok_or(SolfxError::CollateralAccountingUnderflow)?;
        self.total_withdrawals = self
            .total_withdrawals
            .checked_add(amount)
            .ok_or(SolfxError::MathOverflow)?;
        Ok(())
    }
}
