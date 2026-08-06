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

// --- risk engine (Phase 4) ----------------------------------------------------------------

/// The funding and carry indices advanced.
///
/// `funding_rate_per_hour` and `carry_rate_per_hour` are published separately because they
/// are different mechanisms: carry is revenue charged on borrowed notional, funding is a
/// transfer between traders that never touches LP capital (§ 6.7).
#[event]
pub struct FundingUpdated {
    pub market_index: u16,
    pub funding_rate_per_hour: i64,
    pub carry_rate_per_hour: i64,
    pub cum_funding_long: i128,
    pub cum_funding_short: i128,
    pub cum_borrow_index: u128,
    pub skew_ratio: i64,
    pub elapsed_seconds: i64,
    pub ts: i64,
}

/// The session cranker ran.
///
/// Emitted on every crank, not only on a transition, because "the feed was alive and the
/// calendar was open at time T" is the record that proves a market was *not* trading against
/// a frozen price. Absence of a transition is the thing worth being able to audit.
#[event]
pub struct SessionCranked {
    pub market_index: u16,
    pub status: u8,
    pub feed_live: bool,
    pub calendar_open: bool,
    pub seconds_to_close: i64,
    pub ts: i64,
}

/// A position was liquidated (§ 6.8).
#[event]
pub struct PositionLiquidated {
    pub position: Pubkey,
    pub user_account: Pubkey,
    pub liquidator: Pubkey,
    pub market_index: u16,
    pub size_base: u64,
    pub exit_price: i64,
    /// The § 6.8 equity that failed the test — excludes the close fee.
    pub equity: i64,
    pub maintenance_margin: u64,
    pub penalty: u64,
    pub liquidator_reward: u64,
    /// Whatever survived the penalty. A liquidation takes what it is owed, not the account.
    pub residual_to_owner: u64,
    pub bad_debt: u64,
    pub insurance_drawn: u64,
    /// Bad debt the insurance fund could not cover. ADL must clear this.
    pub uncovered_bad_debt: u64,
    pub ts: i64,
}

/// A profitable position was force-closed to socialise a shortfall (§ 6.9 step 2).
///
/// Traders must be told ADL exists, in plain language, **before** they open a position.
/// Every venue that hid it and then used it was destroyed on social media.
#[event]
pub struct PositionAutoDeleveraged {
    pub position: Pubkey,
    pub user_account: Pubkey,
    pub market_index: u16,
    pub size_base: u64,
    pub exit_price: i64,
    /// PnL the position would have realised.
    pub gross_pnl: i64,
    /// What was withheld to cover the shortfall.
    pub socialised: u64,
    pub paid_out: u64,
    pub remaining_adl_debt: u64,
    pub ts: i64,
}

/// A circuit breaker fired (§ 7.2).
#[event]
pub struct CircuitBreakerTripped {
    pub market_index: u16,
    /// 0 = oracle deviation, 1 = oracle staleness, 2 = oracle confidence.
    pub reason: u8,
    pub observed: u64,
    pub limit: u64,
    pub ts: i64,
}

// --- liquidity exit (Phase 5) -------------------------------------------------------------

/// An LP started the cooldown clock.
///
/// Deliberately carries no price. The request is a *time*, not a quote — pricing at request
/// would hand the JIT attacker exactly the option the cooldown exists to take away.
#[event]
pub struct WithdrawalRequested {
    pub provider: Pubkey,
    pub shares: u64,
    pub unlock_at: i64,
    pub pending_shares_after: u64,
    pub ts: i64,
}

#[event]
pub struct WithdrawalCancelled {
    pub provider: Pubkey,
    pub shares: u64,
    pub pending_shares_after: u64,
    pub ts: i64,
}

/// An LP exited. Both fees are published separately because they go to different places and
/// answer different questions: the exit fee to the LPs who stayed, the performance fee to the
/// treasury.
#[event]
pub struct LiquidityRemoved {
    pub provider: Pubkey,
    pub shares: u64,
    /// Value of the shares before fees.
    pub gross: u64,
    /// Stays in the pool, accruing to the remaining LPs.
    pub exit_fee: u64,
    /// Leaves for the treasury. Zero while the pool is below its high-water mark.
    pub performance_fee: u64,
    pub payout: u64,
    pub aum_after: u64,
    pub supply_after: u64,
    pub nav_per_share_after: u64,
    pub ts: i64,
}

/// Treasury fees were swept to a destination.
#[event]
pub struct TreasuryFeesWithdrawn {
    pub destination: Pubkey,
    pub amount: u64,
    pub ts: i64,
}
