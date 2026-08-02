//! Value types shared across the math modules. Deliberately free of Anchor derives —
//! the on-chain crate wraps these rather than the other way round, so the maths stays
//! testable without the BPF toolchain.

/// Which way the trader is betting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Long,
    Short,
}

impl Direction {
    /// `+1` for long, `-1` for short. The sign that multiplies a price delta into PnL.
    #[must_use]
    pub const fn sign(self) -> i128 {
        match self {
            Self::Long => 1,
            Self::Short => -1,
        }
    }
}

/// Whether the trader is entering or leaving the position. Combined with [`Direction`]
/// this decides which side of the spread they cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeAction {
    Open,
    Close,
}

/// The side of the book the trader crosses. Always resolved so the spread is adverse
/// to them (`ARCHITECTURE.md` § 6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Trader is buying — they pay the ask, so the price is adjusted **up**.
    Buy,
    /// Trader is selling — they hit the bid, so the price is adjusted **down**.
    Sell,
}

impl Side {
    /// Opening a long or closing a short is a buy; the other two are sells.
    #[must_use]
    pub const fn resolve(direction: Direction, action: TradeAction) -> Self {
        match (direction, action) {
            (Direction::Long, TradeAction::Open) | (Direction::Short, TradeAction::Close) => {
                Self::Buy
            }
            (Direction::Short, TradeAction::Open) | (Direction::Long, TradeAction::Close) => {
                Self::Sell
            }
        }
    }
}

/// How to turn a PnL denominated in the market's quote currency into USDC (correction C-3).
///
/// Getting this wrong mis-prices every position by the FX rate — for USD/INR at ~88, by a
/// factor of 88. Nearly every listable market needs a variant other than [`Self::None`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteConversion {
    /// Quote currency is USD. PnL is already in collateral terms. The fast path, and the
    /// only variant Tier 1 markets use.
    None,
    /// Feed quotes **quote units per 1 USD** — the `USD/XXX` convention Pyth uses for
    /// almost everything (USD/JPY 157, USD/INR 88). Divide by the rate.
    QuotePerUsd,
    /// Feed quotes **USD per 1 quote unit** — the `XXX/USD` convention (EUR/USD 1.085).
    /// Multiply by the rate. Needed for markets quoted in EUR, GBP, AUD or NZD.
    UsdPerQuote,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_signs_are_opposite() {
        assert_eq!(Direction::Long.sign(), 1);
        assert_eq!(Direction::Short.sign(), -1);
        assert_eq!(Direction::Long.sign(), -Direction::Short.sign());
    }

    /// The trader must cross the adverse side on every one of the four transitions.
    #[test]
    fn side_resolution_is_always_adverse() {
        use Direction::{Long, Short};
        use TradeAction::{Close, Open};
        assert_eq!(Side::resolve(Long, Open), Side::Buy);
        assert_eq!(Side::resolve(Long, Close), Side::Sell);
        assert_eq!(Side::resolve(Short, Open), Side::Sell);
        assert_eq!(Side::resolve(Short, Close), Side::Buy);
    }

    /// Opening and closing the same position must cross opposite sides — otherwise a
    /// round trip could avoid the spread entirely.
    #[test]
    fn open_and_close_cross_opposite_sides() {
        for d in [Direction::Long, Direction::Short] {
            assert_ne!(
                Side::resolve(d, TradeAction::Open),
                Side::resolve(d, TradeAction::Close)
            );
        }
    }
}
