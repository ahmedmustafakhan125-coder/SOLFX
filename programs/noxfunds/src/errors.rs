//! Every refusal names the rule it enforces.
//!
//! A trader whose transaction fails must be able to read *which* rule stopped them. A generic
//! `RuleViolation` would make the product feel arbitrary, which is the opposite of the pitch —
//! the whole claim is that the rules are legible and checked before the fill.

use anchor_lang::prelude::*;

#[error_code]
pub enum NoxError {
    #[msg("Protocol is paused")]
    ProtocolPaused,
    #[msg("Only the admin may do this")]
    NotTheAdmin,
    #[msg("Signer is not this mandate's trader")]
    NotTheTrader,
    #[msg("Mandate is not Active")]
    MandateNotActive,

    #[msg("Trade notional exceeds the mandate's per-trade ceiling")]
    TradeExceedsMandate,
    #[msg("Risk at the stop exceeds the mandate's per-trade risk limit")]
    RiskPerTradeExceeded,
    #[msg("Every funded trade must carry a stop-loss")]
    StopLossRequired,
    #[msg("Stop-loss is further from entry than the mandate allows")]
    StopTooFar,
    #[msg("Stop-loss is on the wrong side of the entry price")]
    StopOnWrongSide,
    #[msg("This market is not in the mandate's permitted set")]
    MarketNotPermitted,
    #[msg("Mandate already holds its maximum number of open positions")]
    TooManyOpenPositions,
    #[msg("Trade would exceed the mandate's total open notional")]
    TotalNotionalExceeded,
    #[msg("Mandate has breached its maximum drawdown")]
    DrawdownExceeded,

    #[msg("Mandate rules are not internally consistent")]
    InvalidMandateRules,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Arithmetic overflow")]
    MathOverflow,
}

/// Map `solfx-math`'s errors onto this program's. Total and explicit, so adding a variant
/// upstream without deciding how it surfaces is a compile error — the same discipline
/// `solfx-core`'s `errors.rs` applies.
impl From<solfx_math::MathError> for NoxError {
    fn from(_: solfx_math::MathError) -> Self {
        NoxError::MathOverflow
    }
}
