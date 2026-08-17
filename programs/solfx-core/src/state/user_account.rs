use anchor_lang::prelude::*;

use crate::errors::SolfxError;

/// A trader's account. PDA at `["user", authority]`.
///
/// Collateral sits here, not in the program's general balance. The user keeps withdrawal
/// authority at all times, subject only to margin — that is the whole non-custodial claim,
/// and it is enforced by `withdraw_collateral` requiring the authority's signature and
/// nothing else.
#[account]
#[derive(InitSpace)]
pub struct UserAccount {
    /// The wallet that owns this account. Immutable.
    pub authority: Pubkey,

    /// USDC not committed to any position, at `QUOTE_PRECISION`.
    ///
    /// Margin is isolated (ADR-004): a position's collateral moves *out* of this balance
    /// and into its own `Position` account. So this balance is genuinely free, and
    /// withdrawing it can never breach an open position's margin.
    pub free_collateral: u64,

    /// Open positions. Guards against closing this account while a position still
    /// references it.
    pub open_positions: u16,

    /// The IB who introduced this trader, or `Pubkey::default()`.
    ///
    /// **Written once, at creation, and never again.** Every IB in retail FX has the same
    /// complaint — the broker reassigns their clients. Making this field immutable is the
    /// structural fix, and it is the reason the referral programme is worth building
    /// on-chain at all (§ 8.5).
    pub referrer: Pubkey,

    /// Rolling 30-day notional, for the volume fee tier (§ 8.2). Maintained from Phase 3.
    pub thirty_day_volume: u64,
    pub volume_window_start_ts: i64,

    /// Lifetime totals, for the portfolio view.
    pub total_deposits: u64,
    pub total_withdrawals: u64,

    pub created_at: i64,

    // --- appended in Phase 6 -----------------------------------------------------------
    /// Cumulative referral-pool share this trader's fees have produced, in USDC.
    ///
    /// **This is the entire interface to the referral programme.** It is a monotonic
    /// counter on an account the trading instructions already load, so recording a rebate
    /// costs no extra account, no CPI and no compute worth measuring — and the referral
    /// program derives everything it owes from the difference between this figure and what
    /// it has already credited.
    ///
    /// It also means every accrual is **independently verifiable**: anyone can read this
    /// number and recompute an IB's entitlement without trusting the referral program, the
    /// indexer, or us. That is the property § 8.5 says the moat is made of.
    pub referral_fees_generated: u64,
    /// Cumulative notional traded, in USDC. Feeds the IB volume tiers (§ 8.5).
    pub lifetime_volume: u64,

    pub bump: u8,
    pub _reserved: [u8; 48],
}

impl UserAccount {
    /// Record a trade's contribution to the referral pool and to lifetime volume.
    ///
    /// Called after every settlement. `referral_share` is zero when the trader has no
    /// referrer — that portion of the fee sweeps to the treasury instead (§ 8.3), and
    /// counting it here would promise an IB money nobody earmarked.
    pub fn record_trade(&mut self, notional: u64, referral_share: u64) -> Result<()> {
        self.lifetime_volume = self
            .lifetime_volume
            .checked_add(notional)
            .ok_or(SolfxError::MathOverflow)?;
        self.referral_fees_generated = self
            .referral_fees_generated
            .checked_add(referral_share)
            .ok_or(SolfxError::MathOverflow)?;
        Ok(())
    }

    pub fn credit(&mut self, amount: u64) -> Result<()> {
        self.free_collateral = self
            .free_collateral
            .checked_add(amount)
            .ok_or(SolfxError::MathOverflow)?;
        Ok(())
    }

    /// Debit free collateral, refusing to go negative.
    ///
    /// Surfaces as `InsufficientCollateral` rather than a generic overflow so the frontend
    /// can say what actually went wrong.
    pub fn debit(&mut self, amount: u64) -> Result<()> {
        self.free_collateral = self
            .free_collateral
            .checked_sub(amount)
            .ok_or(SolfxError::InsufficientCollateral)?;
        Ok(())
    }
}
