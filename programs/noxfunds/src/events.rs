//! Events are the indexer's only input.
//!
//! NOXFUNDS ranks traders on what these carry, and Part 9 promises every displayed statistic
//! is re-derivable from them by a third party. Adding a field is fine; changing or removing
//! one is a breaking change.

use anchor_lang::prelude::*;

#[event]
pub struct ConfigInitialized {
    pub admin: Pubkey,
    pub treasury: Pubkey,
    pub protocol_fee_bps: u16,
    pub ts: i64,
}

#[event]
pub struct MandateFunded {
    pub mandate: Pubkey,
    pub investor: Pubkey,
    pub trader: Pubkey,
    pub principal: u64,
    pub max_trade_notional: u64,
    pub max_drawdown_bps: u16,
    pub allowed_markets: u128,
    pub trader_split_bps: u16,
    pub ts: i64,
}

/// Emitted after both CPIs land, carrying **the margins the trade passed by**.
///
/// Not just that it was allowed — by how much. An investor watching a trader run at 99% of
/// every limit is reading something a pass/fail flag cannot tell them.
#[event]
pub struct FundedTradeOpened {
    pub mandate: Pubkey,
    pub trader: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub direction: u8,
    pub size_base: u64,
    pub notional: u64,
    pub collateral: u64,
    pub stop_loss_price: i64,
    /// Notional as bps of the mandate's per-trade ceiling. 10_000 = exactly at the limit.
    pub notional_used_bps: u64,
    /// Risk at the stop as bps of equity — against `max_risk_per_trade_bps`.
    pub risk_used_bps: u64,
    pub open_positions: u8,
    pub ts: i64,
}

#[event]
pub struct FundedTradeClosed {
    pub mandate: Pubkey,
    pub trader: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub open_positions: u8,
    pub ts: i64,
}

#[event]
pub struct StopCancelled {
    pub mandate: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub order_id: u8,
    pub ts: i64,
}

/// Emitted on every observation, breach or not.
///
/// The crank cadence *is* the honesty of the drawdown rule — equity moves between
/// observations and a spike in the gap is not caught — so each observation is published
/// rather than only the ones that trip something.
#[event]
pub struct EquityObserved {
    pub mandate: Pubkey,
    pub equity: u64,
    pub peak_equity: u64,
    pub drawdown_bps: u64,
    pub open_positions: u8,
    pub observer: Pubkey,
    pub ts: i64,
}

/// A mandate has broken a rule that can only be detected by observation.
///
/// Distinct from a wind-down on purpose: this records *why* it ended, permanently, and that
/// is exactly what an investor choosing a trader is reading.
#[event]
pub struct MandateBreached {
    pub mandate: Pubkey,
    pub trader: Pubkey,
    /// The rule that broke, as its `NoxError` discriminant.
    pub rule: u32,
    pub equity: u64,
    pub peak_equity: u64,
    pub drawdown_bps: u64,
    pub ts: i64,
}

/// The investor asked to end the mandate. New trades stop; open positions may still be closed.
#[event]
pub struct SettlementRequested {
    pub mandate: Pubkey,
    pub investor: Pubkey,
    pub ts: i64,
}

/// Money moved and the mandate is finished. Every figure in the §5.1 worked example is here,
/// so a third party can check the split from the event alone.
#[event]
pub struct MandateSettled {
    pub mandate: Pubkey,
    pub investor: Pubkey,
    pub trader: Pubkey,
    pub principal: u64,
    pub final_equity: u64,
    pub gross_profit: u64,
    pub protocol_fee: u64,
    pub trader_share: u64,
    pub investor_share: u64,
    /// Whether this ended in `Breached` rather than a voluntary wind-down.
    pub was_breached: bool,
    pub settled_by: Pubkey,
    pub ts: i64,
}

/// A third party closed a position on a stopped mandate.
#[event]
pub struct PositionWoundDown {
    pub mandate: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub open_positions: u8,
    pub closer: Pubkey,
    pub ts: i64,
}

/// A position that closed outside NOXFUNDS — a stop-out — was released from the mandate's book.
#[event]
pub struct PositionReconciled {
    pub mandate: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub notional_released: u64,
    pub open_positions: u8,
    pub caller: Pubkey,
    pub ts: i64,
}

/// A trader's record opened. The marketplace's first row for them.
#[event]
pub struct TraderProfileCreated {
    pub profile: Pubkey,
    pub authority: Pubkey,
    pub ts: i64,
}

/// One closed trade, as it landed on the record.
///
/// `realized_pnl` is the **delta in the trader's SolFX free collateral across the close, less
/// the margin the position released** — which is the venue's own arithmetic, net of fees, not a
/// figure this program recomputed. Emitted per trade so any third party can rebuild
/// `gross_profit`, `gross_loss`, win rate and profit factor from the event stream and check
/// them against the account.
#[event]
pub struct TradeRecorded {
    pub profile: Pubkey,
    pub mandate: Pubkey,
    pub trader: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub realized_pnl: i64,
    pub hold_slots: u64,
    pub trades: u32,
    pub wins: u32,
    pub losses: u32,
    pub gross_profit: u64,
    pub gross_loss: u64,
    pub ts: i64,
}

/// A tier moved. Emitted in both directions — a demotion is the more interesting event, and
/// suppressing it would let the marketplace show a peak tier the trader no longer holds.
#[event]
pub struct TierChanged {
    pub profile: Pubkey,
    pub authority: Pubkey,
    pub from: u8,
    pub to: u8,
    pub trades: u32,
    pub profit_factor_bps: u64,
    pub win_rate_bps: u64,
    pub max_drawdown_bps: u16,
    pub ts: i64,
}
