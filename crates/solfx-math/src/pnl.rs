//! Notional, unrealised PnL, and quote-currency conversion.
//!
//! See `ARCHITECTURE.md` § 6.2, § 6.4 and correction C-3.

use crate::constants::{
    NOTIONAL_DIVISOR, NOTIONAL_DIVISOR_I128, PRICE_PRECISION, PRICE_PRECISION_I128,
};
use crate::error::{MathError, MathResult};
use crate::fixed::{
    div_ceil, div_floor, mul_div_ceil, mul_div_floor_signed, positive_to_u128, to_i64, to_u64,
    u128_to_i128,
};
use crate::types::{Direction, QuoteConversion};

/// Position size × price, in the market's **quote currency**.
///
/// Rounded up: notional is the basis for fees and margin requirements, and both must
/// favour the protocol.
///
/// For a non-USD-quoted market this is *not* USDC — feed it through
/// [`convert_cost_to_collateral`] before comparing it against collateral.
pub fn notional_in_quote(size_base: u64, price: i64) -> MathResult<u64> {
    let n = mul_div_ceil(
        u128::from(size_base),
        positive_to_u128(price)?,
        NOTIONAL_DIVISOR,
    )?;
    to_u64(n)
}

/// Position size × price, converted into USDC.
pub fn notional_in_collateral(
    size_base: u64,
    price: i64,
    conversion: QuoteConversion,
    conversion_rate: i64,
) -> MathResult<u64> {
    let quote = notional_in_quote(size_base, price)?;
    convert_cost_to_collateral(quote, conversion, conversion_rate)
}

/// Unrealised PnL in the market's **quote currency**.
///
/// `sign × size_base × (exit − entry) / NOTIONAL_DIVISOR`, rounded toward −∞ so that
/// gains shrink and losses grow.
pub fn upnl_in_quote(
    size_base: u64,
    entry_price: i64,
    exit_price: i64,
    direction: Direction,
) -> MathResult<i64> {
    if entry_price <= 0 || exit_price <= 0 {
        return Err(MathError::InvalidPrice);
    }
    let delta = i128::from(exit_price)
        .checked_sub(i128::from(entry_price))
        .ok_or(MathError::Overflow)?;
    let signed_delta = delta
        .checked_mul(direction.sign())
        .ok_or(MathError::Overflow)?;
    let pnl = mul_div_floor_signed(i128::from(size_base), signed_delta, NOTIONAL_DIVISOR_I128)?;
    to_i64(pnl)
}

/// Unrealised PnL in USDC — the number that actually settles against collateral.
pub fn upnl_in_collateral(
    size_base: u64,
    entry_price: i64,
    exit_price: i64,
    direction: Direction,
    conversion: QuoteConversion,
    conversion_rate: i64,
) -> MathResult<i64> {
    let quote = upnl_in_quote(size_base, entry_price, exit_price, direction)?;
    convert_pnl_to_collateral(quote, conversion, conversion_rate)
}

/// Convert a **signed** quote-currency amount into USDC, rounded toward −∞.
///
/// Flooring is protocol-favourable in both signs: a gain shrinks, a loss grows.
pub fn convert_pnl_to_collateral(
    amount_quote: i64,
    conversion: QuoteConversion,
    conversion_rate: i64,
) -> MathResult<i64> {
    let converted = match conversion {
        QuoteConversion::None => i128::from(amount_quote),
        QuoteConversion::QuotePerUsd => {
            if conversion_rate <= 0 {
                return Err(MathError::InvalidPrice);
            }
            mul_div_floor_signed(
                i128::from(amount_quote),
                PRICE_PRECISION_I128,
                i128::from(conversion_rate),
            )?
        }
        QuoteConversion::UsdPerQuote => {
            if conversion_rate <= 0 {
                return Err(MathError::InvalidPrice);
            }
            mul_div_floor_signed(
                i128::from(amount_quote),
                i128::from(conversion_rate),
                PRICE_PRECISION_I128,
            )?
        }
    };
    to_i64(converted)
}

/// Convert an **unsigned cost** (notional, fee, margin requirement) into USDC, rounded up.
///
/// Costs round up; that is the opposite direction from [`convert_pnl_to_collateral`] and
/// deliberately so — both favour the protocol.
pub fn convert_cost_to_collateral(
    amount_quote: u64,
    conversion: QuoteConversion,
    conversion_rate: i64,
) -> MathResult<u64> {
    let converted = match conversion {
        QuoteConversion::None => u128::from(amount_quote),
        QuoteConversion::QuotePerUsd => mul_div_ceil(
            u128::from(amount_quote),
            PRICE_PRECISION,
            positive_to_u128(conversion_rate)?,
        )?,
        QuoteConversion::UsdPerQuote => mul_div_ceil(
            u128::from(amount_quote),
            positive_to_u128(conversion_rate)?,
            PRICE_PRECISION,
        )?,
    };
    to_u64(converted)
}

/// Size-weighted average entry price, for `increase_position`.
///
/// `(size_a × price_a + size_b × price_b) / (size_a + size_b)`, rounded so the trader's
/// average entry moves against them: **up** when adding to a long, **down** for a short.
/// A favourable-rounded entry price would be a free-money edge repeated on every top-up.
pub fn weighted_entry_price(
    size_a: u64,
    price_a: i64,
    size_b: u64,
    price_b: i64,
    direction: Direction,
) -> MathResult<i64> {
    let total_size = u128::from(size_a)
        .checked_add(u128::from(size_b))
        .ok_or(MathError::Overflow)?;
    if total_size == 0 {
        return Err(MathError::DivideByZero);
    }
    let weighted = u128::from(size_a)
        .checked_mul(positive_to_u128(price_a)?)
        .ok_or(MathError::Overflow)?
        .checked_add(
            u128::from(size_b)
                .checked_mul(positive_to_u128(price_b)?)
                .ok_or(MathError::Overflow)?,
        )
        .ok_or(MathError::Overflow)?;

    let avg = match direction {
        // Higher entry on a long means less profit — adverse, so round up.
        Direction::Long => div_ceil(weighted, total_size)?,
        // Lower entry on a short means less profit — adverse, so round down.
        Direction::Short => div_floor(weighted, total_size)?,
    };
    to_i64(u128_to_i128(avg)?)
}

/// Realise a fraction of a position's PnL on partial close.
///
/// `total_pnl × closed_size / total_size`, floored so the trader never realises more
/// than their proportional share.
pub fn proportional_pnl(total_pnl: i64, closed_size: u64, total_size: u64) -> MathResult<i64> {
    if total_size == 0 {
        return Err(MathError::DivideByZero);
    }
    if closed_size > total_size {
        return Err(MathError::InvalidParameter);
    }
    let p = mul_div_floor_signed(
        i128::from(total_pnl),
        i128::from(closed_size),
        i128::from(total_size),
    )?;
    to_i64(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::LOT_SIZE_BASE;

    const ONE_LOT: u64 = LOT_SIZE_BASE as u64; // 1e14 base units
    const EURUSD_ENTRY: i64 = 1_085_430_000; // 1.08543

    /// ARCHITECTURE.md § 6.2 worked check: 1 lot EUR/USD at 1.08543 = $108,543.00
    #[test]
    fn notional_matches_architecture_worked_example() {
        let n = notional_in_quote(ONE_LOT, EURUSD_ENTRY).unwrap();
        assert_eq!(n, 108_543_000_000);
    }

    /// ARCHITECTURE.md § 6.4 worked check: 1 lot, +10 pips = +$100.00
    #[test]
    fn upnl_matches_architecture_worked_example() {
        let exit = EURUSD_ENTRY + 10 * 100_000; // +10 pips
        let pnl = upnl_in_quote(ONE_LOT, EURUSD_ENTRY, exit, Direction::Long).unwrap();
        assert_eq!(pnl, 100_000_000); // $100.00
    }

    /// FOREX-EXPLAINED.md § 2: one standard lot moves $10 per pip.
    #[test]
    fn one_pip_on_one_standard_lot_is_ten_dollars() {
        let exit = EURUSD_ENTRY + 100_000; // 1 pip
        let pnl = upnl_in_quote(ONE_LOT, EURUSD_ENTRY, exit, Direction::Long).unwrap();
        assert_eq!(pnl, 10_000_000); // $10.00
    }

    #[test]
    fn short_pnl_mirrors_long() {
        let exit = EURUSD_ENTRY + 500_000; // +5 pips
        let long = upnl_in_quote(ONE_LOT, EURUSD_ENTRY, exit, Direction::Long).unwrap();
        let short = upnl_in_quote(ONE_LOT, EURUSD_ENTRY, exit, Direction::Short).unwrap();
        assert_eq!(long, -short);
    }

    /// Correction C-3, the case that mis-prices by 88x if the conversion is skipped.
    ///
    /// USD/INR at 88.5 -> 89.0. Long 1 lot (100,000 USD).
    /// Raw PnL is 50,000 INR; converted at 89 that is ~561.79 USD.
    #[test]
    fn usd_inr_pnl_converts_from_rupees_to_usdc() {
        let entry: i64 = 88_500_000_000; // 88.50 INR per USD
        let exit: i64 = 89_000_000_000; // 89.00

        let pnl_inr = upnl_in_quote(ONE_LOT, entry, exit, Direction::Long).unwrap();
        assert_eq!(pnl_inr, 50_000_000_000); // 50,000 INR

        let pnl_usd =
            convert_pnl_to_collateral(pnl_inr, QuoteConversion::QuotePerUsd, exit).unwrap();
        assert_eq!(pnl_usd, 561_797_752); // $561.797752

        // The bug this guards: booking the raw number as USDC over-credits by ~89x.
        assert!(pnl_inr / pnl_usd > 88);
    }

    #[test]
    fn usd_jpy_pnl_converts_from_yen_to_usdc() {
        let entry: i64 = 157_200_000_000; // 157.20 JPY per USD
        let exit: i64 = 157_300_000_000; // +10 pips (JPY pip = 0.01)

        let pnl_jpy = upnl_in_quote(ONE_LOT, entry, exit, Direction::Long).unwrap();
        assert_eq!(pnl_jpy, 10_000_000_000); // 10,000 JPY

        let pnl_usd =
            convert_pnl_to_collateral(pnl_jpy, QuoteConversion::QuotePerUsd, exit).unwrap();
        assert_eq!(pnl_usd, 63_572_790); // 10,000 JPY / 157.30 = $63.5728
    }

    /// The other conversion direction: a market quoted in EUR, converted via EUR/USD.
    #[test]
    fn usd_per_quote_multiplies_instead_of_dividing() {
        let eur_usd: i64 = 1_085_430_000; // 1.08543 USD per EUR
        let pnl_eur: i64 = 100_000_000; // 100 EUR
        let pnl_usd =
            convert_pnl_to_collateral(pnl_eur, QuoteConversion::UsdPerQuote, eur_usd).unwrap();
        assert_eq!(pnl_usd, 108_543_000); // $108.543
    }

    #[test]
    fn none_conversion_is_the_identity() {
        assert_eq!(
            convert_pnl_to_collateral(12_345, QuoteConversion::None, 0).unwrap(),
            12_345
        );
        assert_eq!(
            convert_cost_to_collateral(12_345, QuoteConversion::None, 0).unwrap(),
            12_345
        );
    }

    /// Costs round up, payouts round down. Same input, opposite direction.
    #[test]
    fn cost_and_pnl_conversions_round_in_opposite_directions() {
        let rate: i64 = 3_000_000_000; // 3.0
        let cost = convert_cost_to_collateral(10, QuoteConversion::QuotePerUsd, rate).unwrap();
        let pnl = convert_pnl_to_collateral(10, QuoteConversion::QuotePerUsd, rate).unwrap();
        assert_eq!(cost, 4); // ceil(10/3)
        assert_eq!(pnl, 3); // floor(10/3)
    }

    /// A loss must round further negative, never toward zero.
    #[test]
    fn negative_pnl_conversion_rounds_away_from_zero() {
        let rate: i64 = 3_000_000_000; // 3.0
        let pnl = convert_pnl_to_collateral(-10, QuoteConversion::QuotePerUsd, rate).unwrap();
        assert_eq!(pnl, -4); // floor(-3.33) = -4, not -3
    }

    #[test]
    fn zero_or_negative_prices_are_rejected() {
        assert_eq!(notional_in_quote(ONE_LOT, 0), Err(MathError::InvalidPrice));
        assert_eq!(notional_in_quote(ONE_LOT, -1), Err(MathError::InvalidPrice));
        assert_eq!(
            upnl_in_quote(ONE_LOT, 0, EURUSD_ENTRY, Direction::Long),
            Err(MathError::InvalidPrice)
        );
        assert_eq!(
            convert_pnl_to_collateral(1, QuoteConversion::QuotePerUsd, 0),
            Err(MathError::InvalidPrice)
        );
    }

    /// Both conversion directions must reject a bad rate, not just the first one.
    #[test]
    fn every_conversion_direction_rejects_a_nonpositive_rate() {
        for conv in [QuoteConversion::QuotePerUsd, QuoteConversion::UsdPerQuote] {
            assert_eq!(
                convert_pnl_to_collateral(1, conv, 0),
                Err(MathError::InvalidPrice)
            );
            assert_eq!(
                convert_pnl_to_collateral(1, conv, -1),
                Err(MathError::InvalidPrice)
            );
            assert_eq!(
                convert_cost_to_collateral(1, conv, 0),
                Err(MathError::InvalidPrice)
            );
            assert_eq!(
                convert_cost_to_collateral(1, conv, -1),
                Err(MathError::InvalidPrice)
            );
        }
    }

    /// The composed helpers must carry the conversion through, not just the primitives.
    #[test]
    fn composed_collateral_helpers_apply_the_conversion() {
        let rate: i64 = 88_500_000_000; // USD/INR
        let direct = notional_in_quote(ONE_LOT, rate).unwrap();
        let converted =
            notional_in_collateral(ONE_LOT, rate, QuoteConversion::QuotePerUsd, rate).unwrap();
        // 1 lot of USD is $100,000 however the quote currency is denominated.
        assert_eq!(converted, 100_000_000_000);
        assert!(direct > converted, "unconverted notional is in rupees");

        let pnl = upnl_in_collateral(
            ONE_LOT,
            rate,
            rate + 500_000_000, // +0.50 INR
            Direction::Long,
            QuoteConversion::QuotePerUsd,
            rate + 500_000_000,
        )
        .unwrap();
        assert_eq!(pnl, 561_797_752);
    }

    #[test]
    fn weighted_entry_rejects_two_empty_legs() {
        assert_eq!(
            weighted_entry_price(0, 1_000_000_000, 0, 1_000_000_000, Direction::Long),
            Err(MathError::DivideByZero)
        );
    }

    #[test]
    fn weighted_entry_sits_between_the_two_prices() {
        let a = 1_000_000_000_i64;
        let b = 2_000_000_000_i64;
        let avg = weighted_entry_price(ONE_LOT, a, ONE_LOT, b, Direction::Long).unwrap();
        assert_eq!(avg, 1_500_000_000);
    }

    /// Rounding on a top-up must be adverse: up for a long, down for a short.
    #[test]
    fn weighted_entry_rounds_against_the_trader() {
        // Two unequal sizes chosen so the true average is not an integer.
        let long =
            weighted_entry_price(3, 1_000_000_000, 1, 1_000_000_001, Direction::Long).unwrap();
        let short =
            weighted_entry_price(3, 1_000_000_000, 1, 1_000_000_001, Direction::Short).unwrap();
        assert_eq!(long, 1_000_000_001); // ceil
        assert_eq!(short, 1_000_000_000); // floor
        assert!(long >= short);
    }

    #[test]
    fn proportional_pnl_splits_and_never_over_realises() {
        assert_eq!(proportional_pnl(1_000, 1, 4).unwrap(), 250);
        // 1/3 of 100 floors to 33, not 34.
        assert_eq!(proportional_pnl(100, 1, 3).unwrap(), 33);
        // A loss floors further negative.
        assert_eq!(proportional_pnl(-100, 1, 3).unwrap(), -34);
    }

    #[test]
    fn proportional_pnl_rejects_closing_more_than_exists() {
        assert_eq!(
            proportional_pnl(100, 5, 4),
            Err(MathError::InvalidParameter)
        );
        assert_eq!(proportional_pnl(100, 1, 0), Err(MathError::DivideByZero));
    }

    /// PRICE_PRECISION is imported for the conversion maths; assert the scale we assume.
    #[test]
    fn conversion_uses_price_precision_scale() {
        assert_eq!(PRICE_PRECISION, 1_000_000_000);
    }
}
