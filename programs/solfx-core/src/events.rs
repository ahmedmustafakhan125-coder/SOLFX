//! Every state mutation emits an event (`ARCHITECTURE.md` § 13.2).
//!
//! These are not logging. They are the indexer's only data source (§ 9.1): the Geyser
//! pipeline reconstructs positions, PnL history and the IB rebate ledger from this stream.
//! An unemitted mutation is a hole in the audit trail, and the auditable rebate ledger is
//! the moat (§ 8.5).

use anchor_lang::prelude::*;

#[event]
pub struct ProtocolInitialized {
    pub admin: Pubkey,
    pub guardian: Pubkey,
    pub usdc_mint: Pubkey,
    pub lp_mint: Pubkey,
    pub ts: i64,
}

#[event]
pub struct MarketInitialized {
    pub market_index: u16,
    pub symbol: String,
    pub feed_kind: u8,
    pub pyth_feed_id: [u8; 32],
    pub is_synthetic: bool,
    pub needs_quote_conversion: bool,
    pub max_leverage: u16,
    pub ts: i64,
}

#[event]
pub struct MarketRiskParamsUpdated {
    pub market_index: u16,
    pub max_leverage: u16,
    pub imr_bps: u16,
    pub mmr_bps: u16,
    pub max_conf_bps: u16,
    pub max_deviation_bps: u16,
    pub max_staleness_seconds: u32,
    pub ts: i64,
}

#[event]
pub struct MarketFeeParamsUpdated {
    pub market_index: u16,
    pub open_fee_rate: u64,
    pub close_fee_rate: u64,
    pub base_spread_bps: u16,
    pub conf_spread_multiplier_bps: u16,
    pub ts: i64,
}

#[event]
pub struct MarketStatusChanged {
    pub market_index: u16,
    pub old_status: u8,
    pub new_status: u8,
    /// Who moved it: 0 = admin, 1 = guardian, 2 = automatic (circuit breaker).
    pub actor: u8,
    pub ts: i64,
}

#[event]
pub struct FeeSplitUpdated {
    pub lp_bps: u16,
    pub treasury_bps: u16,
    pub insurance_bps: u16,
    pub referral_bps: u16,
    pub ts: i64,
}

#[event]
pub struct AdminTransferInitiated {
    pub current_admin: Pubkey,
    pub pending_admin: Pubkey,
    pub ts: i64,
}

#[event]
pub struct AdminTransferAccepted {
    pub old_admin: Pubkey,
    pub new_admin: Pubkey,
    pub ts: i64,
}

#[event]
pub struct ProtocolPauseChanged {
    pub paused: bool,
    pub actor: Pubkey,
    pub ts: i64,
}

#[event]
pub struct UserAccountInitialized {
    pub user_account: Pubkey,
    pub authority: Pubkey,
    /// Bound once and never reassigned — the structural fix for the IB ledger complaint.
    pub referrer: Pubkey,
    pub ts: i64,
}

#[event]
pub struct CollateralDeposited {
    pub user_account: Pubkey,
    pub authority: Pubkey,
    pub amount: u64,
    pub free_collateral_after: u64,
    /// Protocol-wide total after this deposit. Lets the indexer check invariant I1 against
    /// the vault balance on every single transaction rather than only in tests.
    pub total_user_collateral_after: u64,
    pub ts: i64,
}

#[event]
pub struct CollateralWithdrawn {
    pub user_account: Pubkey,
    pub authority: Pubkey,
    pub amount: u64,
    pub free_collateral_after: u64,
    pub total_user_collateral_after: u64,
    pub ts: i64,
}

/// A validated oracle read landed on chain.
///
/// Emitted by `crank_market_price`. This is the record that a feed was reachable, fresh and
/// inside its confidence band at a given moment — the on-chain counterpart of the Phase 0b
/// probe, and the evidence that a listed market is actually priceable.
#[event]
pub struct MarketPriceUpdated {
    pub market_index: u16,
    pub price: i64,
    pub conf: u64,
    pub conf_bps: u64,
    pub ema_price: i64,
    pub deviation_bps: u64,
    pub publish_time: i64,
    pub ts: i64,
}

/// A circuit breaker fired (§ 7.2).
#[event]
pub struct CircuitBreakerTripped {
    pub market_index: u16,
    /// 0 = oracle deviation, 1 = oracle staleness.
    pub reason: u8,
    pub observed: u64,
    pub limit: u64,
    pub ts: i64,
}
