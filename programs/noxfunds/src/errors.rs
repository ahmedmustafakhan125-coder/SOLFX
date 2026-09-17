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
    #[msg("Position has not been held for the mandate's minimum")]
    MinimumHoldNotMet,
    #[msg("Mandate has not breached anything; there is nothing to flag")]
    NotBreached,
    #[msg("Only the investor who funded this mandate may do this")]
    NotTheInvestor,
    #[msg("Mandate still holds open positions; close them before settling")]
    PositionsStillOpen,
    #[msg("Mandate is not winding down or breached, so it cannot be settled")]
    MandateNotSettleable,
    #[msg("This mandate is not tracking a position at that market and nonce")]
    PositionNotTracked,
    #[msg("That position still exists on SolFX, so there is nothing to reconcile")]
    PositionStillOpen,
    #[msg("Equity observation must supply every open position exactly once")]
    IncompleteObservation,
    #[msg("Only a breached or winding-down mandate may be wound down by a third party")]
    MandateNotWindingDown,
    #[msg(
        "A position at this market and nonce closed without being reconciled; reconcile it first"
    )]
    SlotNotReconciled,

    #[msg("Mandate rules are not internally consistent")]
    InvalidMandateRules,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Arithmetic overflow")]
    MathOverflow,

    // --- tiers and the track record (Stage 4) --------------------------------------------
    #[msg("This mandate is larger than the trader's tier permits")]
    MandateExceedsTierLimit,
    #[msg("The trader already holds as many mandates as their tier permits")]
    TooManyActiveMandates,
    #[msg("This profile does not belong to the mandate's trader")]
    ProfileMismatch,

    // --- initialisation --------------------------------------------------------------------
    // Appended rather than grouped with the admin errors: the program is deployed and its IDL
    // published, so inserting a variant earlier would renumber every error after it.
    #[msg("Only the program's upgrade authority may initialise the configuration")]
    NotTheUpgradeAuthority,
}

/// Map `solfx-math`'s errors onto this program's. Total and explicit, so adding a variant
/// upstream without deciding how it surfaces is a compile error — the same discipline
/// `solfx-core`'s `errors.rs` applies.
impl From<solfx_math::MathError> for NoxError {
    fn from(_: solfx_math::MathError) -> Self {
        NoxError::MathOverflow
    }
}
