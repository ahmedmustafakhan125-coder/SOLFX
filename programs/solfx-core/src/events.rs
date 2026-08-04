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

// --- positions (Phase 3) ------------------------------------------------------------------

/// A position was opened.
///
/// `exec_price` is what the trader actually filled at — oracle mid plus the adverse spread —
/// while `oracle_price` is the mid it was derived from. Emitting both is what lets a trader
/// verify the spread they were charged rather than take it on trust, which is the whole
/// difference between this and a broker's fill report.
#[event]
pub struct PositionOpened {
    pub position: Pubkey,
    pub user_account: Pubkey,
    pub market_index: u16,
    pub direction: u8,
    pub size_base: u64,
    pub oracle_price: i64,
    pub exec_price: i64,
    pub spread_bps: u64,
    pub notional_collateral: u64,
    pub collateral: u64,
    pub fee: u64,
    pub referrer: Pubkey,
    pub ts: i64,
}

#[event]
pub struct PositionIncreased {
    pub position: Pubkey,
    pub market_index: u16,
    pub size_added: u64,
    pub collateral_added: u64,
    pub exec_price: i64,
    pub new_size_base: u64,
    pub new_entry_price: i64,
    pub fee: u64,
    pub ts: i64,
}

/// A position was reduced or closed.
///
/// `realized_pnl` is signed and already converted into USDC (correction C-3), so an indexer
/// never has to know the market's quote currency to total a trader's P&L.
#[event]
pub struct PositionDecreased {
    pub position: Pubkey,
    pub user_account: Pubkey,
    pub market_index: u16,
    pub size_closed: u64,
    pub remaining_size: u64,
    pub oracle_price: i64,
    pub exec_price: i64,
    pub realized_pnl: i64,
    pub fee: u64,
    pub collateral_returned: u64,
    pub fully_closed: bool,
    pub ts: i64,
}

#[event]
pub struct PositionCollateralChanged {
    pub position: Pubkey,
    pub market_index: u16,
    /// Positive when collateral was added, negative when removed.
    pub delta: i64,
    pub collateral_after: u64,
    pub ts: i64,
}

/// A fee was collected and split (§ 8.3).
///
/// Emitted alongside every trade. The `referral` field is the per-trade accrual the IB
/// programme claims against in Phase 6 — a public, verifiable record of what was earned,
/// which is the structural answer to "the broker controls the ledger" (§ 8.5).
#[event]
pub struct FeeCollected {
    pub market_index: u16,
    pub user_account: Pubkey,
    pub referrer: Pubkey,
    pub total: u64,
    pub lp: u64,
    pub treasury: u64,
    pub insurance: u64,
    pub referral: u64,
    pub ts: i64,
}

/// The LP vault paid or received on a settled position.
///
/// This is the B-book made visible: the pool is the counterparty, it earns when traders lose
/// and pays when they win, and every one of those transfers is a public event (§ 1.3).
#[event]
pub struct PoolSettlement {
    pub market_index: u16,
    /// Positive when the pool paid a winning trader, negative when it received.
    pub amount: i64,
    pub aum_after: u64,
    pub ts: i64,
}

#[event]
pub struct LiquidityAdded {
    pub provider: Pubkey,
    pub usdc_in: u64,
    pub lp_tokens_out: u64,
    pub aum_after: u64,
    pub ts: i64,
}

/// A position closed owing more than its collateral covered.
///
/// Phase 3 has no liquidation engine, so this is reachable by simply letting a position run.
/// The trader's loss is capped at what they posted and the shortfall lands on the pool.
/// Phase 4 inserts the insurance fund and ADL ahead of that (§ 6.9); this event is what those
/// mechanisms will reconcile against.
#[event]
pub struct BadDebtIncurred {
    pub market_index: u16,
    pub position: Pubkey,
    pub amount: u64,
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
