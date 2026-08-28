//! Contract sizes, shared by every tool that has to turn a human size into base units.
//!
//! # Why this is a file and not a constant
//!
//! `solfx-math` carries `LOT_SIZE_BASE = 100,000`, documented there as "a presentation
//! concept only". That is the **FX** convention, and it is the only one the engine knows —
//! `Market` stores no contract size, because the engine works in base units and does not
//! need one.
//!
//! Every *client* does need one, and they must agree. Retail platforms size asset classes
//! differently:
//!
//! | Class     | 1 lot        | Source |
//! |-----------|--------------|--------|
//! | FX        | 100,000 base | MT4/MT5 standard |
//! | Gold      | 100 troy oz  | XAUUSD CFD standard |
//! | Crypto    | 1 coin       | BTCUSD = 1 BTC at XM and Exness |
//!
//! Applying the FX number to Bitcoin makes `--lots 0.01` mean 1,000 BTC instead of 0.01 —
//! five orders of magnitude, and it reads as a plausible order the whole way. The same table
//! also sets each market's position bounds at listing time, so a size that is legal to ask
//! for is legal to fill.
//!
//! Kept in one file, included by `#[path]`, so the poster, the initialiser and the trading
//! CLI cannot drift apart.

/// Units of the base asset in one standard lot.
///
/// Silver's 5,000 oz and oil's 1,000 barrels are the common MT5 figures but are **not**
/// verified against a broker specification here.
#[must_use]
pub fn units_per_lot(symbol: &str) -> u128 {
    let sym = symbol.to_uppercase();
    match () {
        () if sym.starts_with("XAU") => 100,
        () if sym.starts_with("XAG") => 5_000,
        () if sym.starts_with("XPT") || sym.starts_with("XPD") => 100,
        () if sym.starts_with("BTC") || sym.starts_with("ETH") || sym.starts_with("SOL") => 1,
        () if sym.contains("OIL") => 1_000,
        () => 100_000,
    }
}

/// Base units in one lot, at `BASE_PRECISION`.
#[must_use]
pub fn one_lot_base(symbol: &str) -> u128 {
    units_per_lot(symbol).saturating_mul(1_000_000_000)
}

/// The largest single position, in base units: 100 lots.
#[must_use]
pub fn max_position_base(symbol: &str) -> u64 {
    u64::try_from(one_lot_base(symbol).saturating_mul(100)).unwrap_or(u64::MAX)
}

/// The smallest single position, in base units: a millionth of a lot.
///
/// Deliberately far below anything tradeable. A base-unit floor cannot express a *value*
/// floor — an ounce of gold and a euro differ by orders of magnitude — so the real minimum
/// is `pricing::validate_notional` ($1 of notional), which applies uniformly. This only
/// stops dust.
#[must_use]
pub fn min_position_base(symbol: &str) -> u64 {
    let millionth = one_lot_base(symbol).checked_div(1_000_000).unwrap_or(1);
    u64::try_from(millionth).unwrap_or(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lot_means_different_amounts_per_asset_class() {
        assert_eq!(units_per_lot("EUR/USD"), 100_000);
        assert_eq!(units_per_lot("USD/INR"), 100_000);
        assert_eq!(units_per_lot("XAU/USD"), 100);
        assert_eq!(units_per_lot("BTC/USD"), 1);
        assert_eq!(units_per_lot("ETH/USD"), 1);
        assert_eq!(units_per_lot("USOILSPOT/USD"), 1_000);
    }

    /// The failure this prevents: bounds derived from the FX lot made the *minimum* BTC
    /// position 0.1 BTC (~$7,700), so a $1,000 order was rejected as too small.
    #[test]
    fn position_bounds_track_the_contract_size() {
        // 100 BTC max, a millionth of a BTC min.
        assert_eq!(max_position_base("BTC/USD"), 100_000_000_000);
        assert_eq!(min_position_base("BTC/USD"), 1_000);

        // A $1,000 BTC order at ~$77k is ~0.0129 BTC = 12,900,000 base units.
        let realistic = 12_900_000u64;
        assert!(
            realistic >= min_position_base("BTC/USD"),
            "must clear the floor"
        );
        assert!(
            realistic <= max_position_base("BTC/USD"),
            "must clear the ceiling"
        );

        // FX keeps the figures the reference market used.
        assert_eq!(min_position_base("EUR/USD"), 100_000_000);
    }

    #[test]
    fn a_floor_is_never_zero() {
        for s in ["BTC/USD", "XAU/USD", "EUR/USD", "USOILSPOT/USD"] {
            assert!(min_position_base(s) >= 1, "{s} floor must be positive");
            assert!(
                min_position_base(s) < max_position_base(s),
                "{s} bounds must order"
            );
        }
    }
}
