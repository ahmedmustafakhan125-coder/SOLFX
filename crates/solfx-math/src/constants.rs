//! Fixed-point scales. See `docs/ARCHITECTURE.md` § 6.1.

/// USDC native decimals. All collateral, fees and PnL are denominated here.
pub const QUOTE_PRECISION: u128 = 1_000_000; // 1e6

/// Base-currency units. Position size is stored in these, never in lots.
pub const BASE_PRECISION: u128 = 1_000_000_000; // 1e9

/// Quote per 1 base. EUR/USD 1.08543 -> 1_085_430_000.
///
/// One pip (0.0001) is 100_000 units and one pipette is 10_000, giving 10,000 ticks of
/// sub-pip resolution — fine enough that rounding never dominates a 1 bp fee.
pub const PRICE_PRECISION: u128 = 1_000_000_000; // 1e9

/// Fees, funding and carry rates. Finer than basis points so 0.8 bps is expressible
/// (`ARCHITECTURE.md` § 5.3, "Note on fee precision").
pub const RATE_PRECISION: u128 = 1_000_000_000; // 1e9

/// Basis points. 10_000 bps == 100%.
pub const BPS_PRECISION: u128 = 10_000;

/// Converts `base_units * price` into quote units: `BASE * PRICE / QUOTE` = 1e12.
///
/// Written as a literal rather than the expression so it does not need a division; the
/// assertion below pins it to the derivation, checked at compile time.
pub const NOTIONAL_DIVISOR: u128 = 1_000_000_000_000;
const _: () = assert!(NOTIONAL_DIVISOR * QUOTE_PRECISION == BASE_PRECISION * PRICE_PRECISION);

/// One standard lot = 100,000 units of base currency. A presentation concept only —
/// the engine works in base units and the UI divides by this for display.
pub const LOT_SIZE_BASE: u128 = 100_000 * BASE_PRECISION; // 1e14

/// Smallest notional the engine will price, in quote units ($1.00).
///
/// Below this, a 1 bp fee and a 1 bp spread both round to zero, and the round-trip cost of a
/// trade can reach zero — which is the rounding-loop drain in `ARCHITECTURE.md` § 13.1 T14.
/// Enforced by [`crate::pricing::validate_notional`]; the guarantee that a round trip always
/// costs the trader something holds only at or above this size.
pub const MIN_NOTIONAL_QUOTE: u64 = 1_000_000; // $1.00

/// Seconds in an hour. Funding and carry accrue per hour.
pub const SECONDS_PER_HOUR: i64 = 3_600;

// Signed mirrors. Library code never writes `as` — narrowing goes through
// `crate::fixed`, and widening uses `From`. These exist so signed maths can reference
// the same scales without a cast.

pub const NOTIONAL_DIVISOR_I128: i128 = 1_000_000_000_000;
pub const PRICE_PRECISION_I128: i128 = 1_000_000_000;
pub const RATE_PRECISION_I128: i128 = 1_000_000_000;
pub const BPS_PRECISION_I128: i128 = 10_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notional_divisor_is_1e12() {
        assert_eq!(NOTIONAL_DIVISOR, 1_000_000_000_000);
    }

    /// The signed mirrors must never drift from the unsigned originals.
    #[test]
    fn signed_mirrors_match_their_unsigned_originals() {
        assert_eq!(NOTIONAL_DIVISOR_I128 as u128, NOTIONAL_DIVISOR);
        assert_eq!(PRICE_PRECISION_I128 as u128, PRICE_PRECISION);
        assert_eq!(RATE_PRECISION_I128 as u128, RATE_PRECISION);
        assert_eq!(BPS_PRECISION_I128 as u128, BPS_PRECISION);
    }

    #[test]
    fn one_standard_lot_is_1e14_base_units() {
        assert_eq!(LOT_SIZE_BASE, 100_000_000_000_000);
    }

    /// A pip on a 4-decimal pair is 1e-4, so 1e5 units at PRICE_PRECISION = 1e9.
    #[test]
    fn one_pip_is_100_000_price_units() {
        assert_eq!(PRICE_PRECISION / 10_000, 100_000);
    }

    /// USD/JPY near 157 must sit far inside i64. Headroom check against the largest
    /// price we expect to store.
    #[test]
    fn price_precision_leaves_headroom_in_i64() {
        let usd_jpy = 157_i128 * PRICE_PRECISION as i128;
        assert!(usd_jpy < i64::MAX as i128 / 1_000_000);
    }
}
