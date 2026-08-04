//! Every failure mode is named (`ARCHITECTURE.md` § 13.2).
//!
//! Named errors are not cosmetic here. § 7.2's circuit breakers each need to be
//! distinguishable by the frontend, because "market conditions — spread too wide" and
//! "insufficient collateral" call for completely different user actions, and rendering both
//! as a generic failure is how a protocol earns a reputation for eating orders (ADR-006).

use anchor_lang::prelude::*;
use solfx_math::MathError;

#[error_code]
pub enum SolfxError {
    // --- arithmetic ---
    #[msg("Arithmetic overflow")]
    MathOverflow,
    #[msg("Division by zero")]
    DivideByZero,
    #[msg("Parameter out of range")]
    InvalidParameter,

    // --- oracle (§ 7.1) ---
    #[msg("Oracle price must be positive")]
    InvalidOraclePrice,
    #[msg("Oracle confidence exceeds this market's ceiling")]
    OracleConfidenceTooWide,
    #[msg("Oracle price is stale")]
    OracleStale,
    #[msg("Oracle price is dated in the future")]
    OracleFromFuture,
    #[msg("Price deviates too far from the reference")]
    OracleDeviationTooLarge,
    #[msg("Price update does not match this market's configured feed")]
    WrongOracleFeed,
    #[msg("A synthetic market requires a second price update account")]
    MissingSecondaryPriceUpdate,
    #[msg("This market requires a quote-conversion price update account")]
    MissingQuoteConversionPriceUpdate,
    #[msg("A price update account was supplied that this market does not use")]
    UnexpectedPriceUpdate,

    // --- authority ---
    #[msg("Signer is not the protocol admin")]
    NotAdmin,
    #[msg("Signer is not the pending admin")]
    NotPendingAdmin,
    #[msg("Signer is not the guardian")]
    NotGuardian,
    #[msg("No admin transfer is pending")]
    NoPendingAdmin,
    #[msg("Account authority mismatch")]
    AuthorityMismatch,

    // --- protocol state ---
    #[msg("Protocol is paused")]
    ProtocolPaused,
    #[msg("Collateral mint must have 6 decimals to match QUOTE_PRECISION")]
    InvalidCollateralMintDecimals,
    #[msg("Collateral mint does not match the protocol's configured mint")]
    WrongCollateralMint,
    #[msg("Fee splits must total exactly 10000 bps")]
    InvalidFeeSplit,
    #[msg("Collateral accounting underflowed: vault total is below zero")]
    CollateralAccountingUnderflow,
    #[msg("Maximum market count reached")]
    TooManyMarkets,

    // --- market configuration ---
    #[msg("Leverage must be between 1 and the protocol maximum")]
    InvalidLeverage,
    #[msg("Maintenance margin must be positive and below initial margin")]
    InvalidMarginRatios,
    #[msg("Liquidation fee must be positive and below the maintenance margin ratio")]
    InvalidLiquidationFee,
    #[msg("Staleness window must be positive and within the protocol ceiling")]
    InvalidStaleness,
    #[msg("Confidence limit must be positive and within the protocol ceiling")]
    InvalidConfidenceLimit,
    #[msg("Deviation limit must be positive")]
    InvalidDeviationLimit,
    #[msg("Position size bounds are inconsistent")]
    InvalidPositionSizeBounds,
    #[msg("Open interest caps must be positive")]
    InvalidOiCap,
    #[msg("Session calendar is invalid")]
    InvalidSession,
    #[msg("Market symbol is empty or too long")]
    InvalidSymbol,
    #[msg("Market index does not match the protocol's next index")]
    InvalidMarketIndex,
    #[msg("Feed id must not be all zeroes")]
    InvalidFeedId,
    #[msg("A synthetic market requires a secondary feed id")]
    MissingSecondaryFeedId,
    #[msg("A non-USD-quoted market requires a quote-conversion feed id")]
    MissingQuoteConversionFeedId,
    #[msg("FeedKind::ContinuousIndex has no reachable feed; blocked pending Q1b")]
    ContinuousIndexUnavailable,

    // --- market state ---
    #[msg("Market is not active")]
    MarketNotActive,
    #[msg("Market does not permit opening positions right now")]
    MarketClosedForOpens,
    #[msg("Market is halted")]
    MarketHalted,
    #[msg("Invalid market status transition")]
    InvalidStatusTransition,

    // --- collateral ---
    #[msg("Insufficient free collateral")]
    InsufficientCollateral,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Account still has open positions")]
    HasOpenPositions,

    // --- positions (Phase 3) ---
    #[msg("Position size is outside this market's permitted bounds")]
    PositionSizeOutOfBounds,
    #[msg("Notional is below the protocol minimum")]
    NotionalTooSmall,
    #[msg("Leverage exceeds this market's maximum")]
    LeverageTooHigh,
    #[msg("Collateral is below the initial margin requirement")]
    InsufficientMargin,
    #[msg("Fill price is worse than the slippage bound")]
    SlippageExceeded,
    #[msg("Position has not been held for the minimum number of slots")]
    MinHoldTimeNotMet,
    #[msg("Cannot reduce a position by more than its size")]
    ReductionExceedsSize,
    #[msg("Position still holds size and cannot be closed this way")]
    PositionNotEmpty,
    #[msg("Position does not belong to this market")]
    PositionMarketMismatch,
    #[msg("Open interest cap reached on this side")]
    OpenInterestCapReached,
    #[msg("Open interest accounting underflowed")]
    OpenInterestUnderflow,
    #[msg("Removing this collateral would breach the maintenance margin")]
    WouldBreachMaintenanceMargin,
    #[msg("Position is already liquidatable and cannot be modified")]
    PositionLiquidatable,

    // --- liquidity ---
    #[msg("The liquidity pool cannot cover this payout")]
    InsufficientPoolLiquidity,
    #[msg("LP share calculation produced zero shares")]
    ZeroLpShares,
}

/// Map the maths crate's errors onto program errors.
///
/// `solfx-math` deliberately does not depend on Anchor — that is what keeps it testable
/// without the BPF toolchain — so this is the one place the two error spaces meet. The
/// mapping is total and explicit: adding a `MathError` variant without deciding how it
/// surfaces to a user is a compile error.
impl From<MathError> for SolfxError {
    fn from(e: MathError) -> Self {
        match e {
            MathError::Overflow => Self::MathOverflow,
            MathError::DivideByZero => Self::DivideByZero,
            MathError::InvalidPrice => Self::InvalidOraclePrice,
            MathError::ConfidenceTooWide => Self::OracleConfidenceTooWide,
            MathError::NotionalTooSmall => Self::NotionalTooSmall,
            MathError::InvalidParameter => Self::InvalidParameter,
            MathError::PriceTooStale => Self::OracleStale,
            MathError::PriceFromFuture => Self::OracleFromFuture,
            MathError::DeviationTooLarge => Self::OracleDeviationTooLarge,
        }
    }
}

/// Lift a `MathResult` into an Anchor `Result`.
pub trait IntoProgramResult<T> {
    fn or_program_err(self) -> Result<T>;
}

impl<T> IntoProgramResult<T> for core::result::Result<T, MathError> {
    fn or_program_err(self) -> Result<T> {
        self.map_err(|e| error!(SolfxError::from(e)))
    }
}
