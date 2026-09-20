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

/// A take-profit was added to an open funded position.
///
/// Carries the position key rather than `(market_index, nonce)`: the position is an account the
/// instruction touched, so the field cannot disagree with what happened. An index taken as an
/// argument and never checked can.
#[event]
pub struct TakeProfitPlaced {
    pub mandate: Pubkey,
    pub position: Pubkey,
    pub order_id: u8,
    pub trigger_price: i64,
    pub size_base: u64,
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

// --- the marketplace ---------------------------------------------------------------------
//
// These are the marketplace's only output. A third party indexing NOXFUNDS must be able to
// rebuild the whole order flow — who advertised, who offered what to whom, what was accepted and
// what was withdrawn — from these alone, without reading any account and without trusting any
// server we run.

/// A trader advertised for capital, or edited the terms of an existing advertisement.
#[event]
pub struct ListingPosted {
    pub trader: Pubkey,
    pub listing: Pubkey,
    pub min_principal: u64,
    pub max_principal: u64,
    pub wanted_markets: u128,
    pub wanted_split_bps: u16,
    pub timestamp: i64,
}

/// A trader withdrew their advertisement. The account remains; only the flag changed.
#[event]
pub struct ListingClosed {
    pub trader: Pubkey,
    pub listing: Pubkey,
    pub timestamp: i64,
}

/// An investor escrowed capital and published terms to one trader.
#[event]
pub struct OfferPosted {
    pub offer: Pubkey,
    pub investor: Pubkey,
    pub trader: Pubkey,
    pub seq: u8,
    pub principal: u64,
    pub trader_split_bps: u16,
    pub expires_at: i64,
    pub timestamp: i64,
}

/// An investor took their capital back before any trader accepted.
#[event]
pub struct OfferRevoked {
    pub offer: Pubkey,
    pub investor: Pubkey,
    pub trader: Pubkey,
    pub principal: u64,
    pub timestamp: i64,
}

/// A trader accepted, and the offer became a funded mandate in the same transaction.
///
/// `MandateFunded` is emitted alongside this, so a consumer watching only for funded mandates
/// does not have to know the marketplace exists.
#[event]
pub struct OfferAccepted {
    pub offer: Pubkey,
    pub mandate: Pubkey,
    pub investor: Pubkey,
    pub trader: Pubkey,
    pub principal: u64,
    pub timestamp: i64,
}

/// A trader said no to an offer. The reason is on the offer, in `reply`.
#[event]
pub struct OfferDeclined {
    pub offer: Pubkey,
    pub investor: Pubkey,
    pub trader: Pubkey,
    pub timestamp: i64,
}

/// An investor advertised capital, or edited the advertisement.
#[event]
pub struct InvestorListingPosted {
    pub investor: Pubkey,
    pub listing: Pubkey,
    pub min_principal: u64,
    pub max_principal: u64,
    pub allowed_markets: u128,
    pub offered_split_bps: u16,
    pub timestamp: i64,
}

/// An investor stopped advertising. No new requests can reach them until they reopen.
#[event]
pub struct InvestorListingClosed {
    pub investor: Pubkey,
    pub listing: Pubkey,
    pub timestamp: i64,
}

/// A trader asked an investor for capital.
#[event]
pub struct RequestPosted {
    pub request: Pubkey,
    pub trader: Pubkey,
    pub investor: Pubkey,
    pub wanted_principal: u64,
    pub wanted_split_bps: u16,
    pub timestamp: i64,
}

/// A request was withdrawn by the trader or dismissed by the investor. `closed_by` says which.
#[event]
pub struct RequestClosed {
    pub request: Pubkey,
    pub trader: Pubkey,
    pub investor: Pubkey,
    pub closed_by: Pubkey,
    pub timestamp: i64,
}

// --- the evaluation (Stage 3) -----------------------------------------------------------------

#[event]
pub struct EvaluationStarted {
    pub evaluation: Pubkey,
    pub trader: Pubkey,
    pub seq: u8,
    pub account_size: u64,
    pub stake: u64,
    pub ts: i64,
}

/// A simulated fill. `entry_price` and `fee` are exactly what a real SolFX fill would have been
/// on the same price update and market state.
#[event]
pub struct EvaluationTradeOpened {
    pub evaluation: Pubkey,
    pub position: Pubkey,
    pub market_index: u16,
    pub direction: u8,
    pub size_base: u64,
    pub entry_price: i64,
    pub notional: u64,
    pub fee: u64,
    pub stop_price: i64,
    pub risk_used_bps: u64,
    pub ts: i64,
}

#[event]
pub struct EvaluationTradeClosed {
    pub evaluation: Pubkey,
    pub position: Pubkey,
    pub exit_price: i64,
    /// Net of the open fee, the close fee and carry — the trade's whole result.
    pub result: i64,
    /// A stop-out rather than a voluntary close: exempt from the hold rules.
    pub stop_out: bool,
    pub held_secs: i64,
    pub balance: i64,
    pub ts: i64,
}

#[event]
pub struct EvaluationEquityObserved {
    pub evaluation: Pubkey,
    pub equity: u64,
    pub peak_equity: u64,
    pub drawdown_bps: u64,
    pub daily_loss_bps: u64,
    pub observer: Pubkey,
    pub ts: i64,
}

#[event]
pub struct StagePassed {
    pub evaluation: Pubkey,
    pub trader: Pubkey,
    pub stage: u8,
    pub equity: u64,
    pub trades: u32,
    pub trading_days: u16,
    pub ts: i64,
}

/// Why it failed is `rule`, never a generic failure.
#[event]
pub struct EvaluationFailed {
    pub evaluation: Pubkey,
    pub trader: Pubkey,
    pub stage: u8,
    pub rule: u8,
    pub equity: u64,
    pub ts: i64,
}

#[event]
pub struct StakeRefunded {
    pub evaluation: Pubkey,
    pub trader: Pubkey,
    pub amount: u64,
    pub ts: i64,
}

#[event]
pub struct StakeForfeited {
    pub evaluation: Pubkey,
    pub treasury: Pubkey,
    pub amount: u64,
    pub ts: i64,
}
