//! Parity with what a real broker terminal shows.
//!
//! Every case here is taken from an actual MT5 position panel, and asserts that this engine
//! produces **the same number to the cent**. The point is not that the arithmetic is
//! plausible — the unit tests already cover that — but that it agrees with the figure a
//! trader is looking at somewhere else.
//!
//! That matters more than it sounds. A retail FX trader has twenty years of muscle memory
//! from MT4/MT5 (`ARCHITECTURE.md` § 10.2). If our P&L differs from theirs by so much as a
//! rounding unit, they will not conclude that we round differently — they will conclude the
//! platform is wrong, and they will be right to wonder what else is.
//!
//! # The case these tests exist for
//!
//! **GBP/CHF is quoted in Swiss Francs.** `size × Δprice` produces a number in *CHF*, and
//! booking it as USDC would be wrong by the CHF/USD rate — about 23% on this pair, and about
//! **8,700%** on USD/INR. That is correction C-3, and it is the single highest-consequence
//! arithmetic detail in the protocol because *nearly every listable market needs it*: 106 of
//! Pyth's 110 direct FX feeds are `USD/XXX`, and every `XXX/JPY` cross is quoted in Yen.
//!
//! A test that only ever checked EUR/USD would never see it, because for EUR/USD the
//! conversion is the identity.

#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use solfx_math::pnl;
use solfx_math::{Direction, QuoteConversion};

/// 1 standard lot = 100,000 units of base currency, at `BASE_PRECISION` (1e9).
const LOT: u64 = 100_000 * 1_000_000_000;

/// Convert a decimal price to `PRICE_PRECISION` (1e9). 1.09659 -> 1_096_590_000.
fn price(p: f64) -> i64 {
    (p * 1e9).round() as i64
}

/// USDC at `QUOTE_PRECISION` (1e6) back to dollars, for readable assertions.
fn dollars(units: i64) -> f64 {
    units as f64 / 1e6
}

/// **The screenshot.** GBP/CHF, buy 60.00 lots, 1.09659 → 1.09850, terminal shows 14 141.69.
///
/// The broker's own working, which this reproduces exactly:
///
/// ```text
/// size      = 60 lots × 100,000            = 6,000,000 GBP
/// move      = 1.09850 − 1.09659            = 0.00191 CHF per GBP  (19.1 pips)
/// P&L quote = 6,000,000 × 0.00191          = 11,460.00 CHF        ← NOT dollars
/// P&L USD   = 11,460 ÷ 0.810370 (USD/CHF)  = 14,141.69 USD
/// ```
///
/// The middle line is the whole reason `QuoteConversion` exists. Skip it and the position
/// reads as $11,460 — a 23% error that looks entirely reasonable and would never be noticed
/// until a trader compared it with their other broker.
#[test]
fn gbp_chf_sixty_lots_matches_the_terminal_to_the_cent() {
    let size = 60 * LOT;
    let entry = price(1.09659);
    let exit = price(1.09850);
    // Pyth quotes Swiss Francs as USD/CHF — francs per dollar — so the conversion divides.
    let usd_chf = price(0.810_370);

    // Step 1: P&L in the quote currency (CHF).
    let quote = pnl::upnl_in_quote(size, entry, exit, Direction::Long).unwrap();
    assert_eq!(quote, 11_460_000_000, "expected 11,460.00 CHF");

    // Step 2: converted to the collateral currency (USDC).
    let usd = pnl::upnl_in_collateral(
        size,
        entry,
        exit,
        Direction::Long,
        QuoteConversion::QuotePerUsd,
        usd_chf,
    )
    .unwrap();

    assert_eq!(
        usd,
        14_141_688_364,
        "expected $14,141.69 (terminal figure), got ${:.2}",
        dollars(usd)
    );
    // To the cent, which is all a trader can see.
    assert_eq!(format!("{:.2}", dollars(usd)), "14141.69");
}

/// The other two rows of the same screenshot — same size, one pip apart at entry. Included
/// because three positions differing only in the fifth decimal is exactly where an
/// off-by-one in the price scale would show up.
#[test]
fn the_other_two_positions_in_the_screenshot_also_match() {
    let size = 60 * LOT;
    let exit = price(1.09850);
    let usd_chf = price(0.810_370);

    for (entry_dec, expected) in [(1.09657_f64, "14289.77"), (1.09656_f64, "14363.81")] {
        let usd = pnl::upnl_in_collateral(
            size,
            price(entry_dec),
            exit,
            Direction::Long,
            QuoteConversion::QuotePerUsd,
            usd_chf,
        )
        .unwrap();
        assert_eq!(
            format!("{:.2}", dollars(usd)),
            expected,
            "entry {entry_dec} should show {expected}"
        );
    }
}

/// The textbook case every FX trader knows: **1 standard lot, 1 pip, $10.**
///
/// It falls straight out of the fixed-point scales rather than being special-cased, which is
/// the check that `BASE_PRECISION`, `PRICE_PRECISION` and `NOTIONAL_DIVISOR` agree.
#[test]
fn one_lot_of_a_usd_quoted_pair_is_ten_dollars_per_pip() {
    let usd = pnl::upnl_in_quote(LOT, price(1.08500), price(1.08510), Direction::Long).unwrap();
    assert_eq!(usd, 10_000_000, "1 lot, 1 pip = $10.00");

    // 50 pips on 1 lot = $500 — the worked example in FOREX-EXPLAINED.md § 1.
    let usd = pnl::upnl_in_quote(LOT, price(1.08500), price(1.09000), Direction::Long).unwrap();
    assert_eq!(usd, 500_000_000, "1 lot, 50 pips = $500.00");
}

/// USD/JPY: quoted in Yen, and a **2-decimal pip**. A trader's pip value here is not $10 —
/// it is $10 divided by the USD/JPY rate, which is what the conversion produces.
#[test]
fn usd_jpy_one_lot_one_pip_converts_out_of_yen() {
    let usd_jpy = price(157.200);
    // One pip on a JPY pair is 0.01.
    let usd = pnl::upnl_in_collateral(
        LOT,
        price(157.200),
        price(157.210),
        Direction::Long,
        QuoteConversion::QuotePerUsd,
        usd_jpy,
    )
    .unwrap();

    // 100,000 × 0.01 = ¥1,000 -> ¥1,000 / 157.20 = $6.36
    assert_eq!(format!("{:.2}", dollars(usd)), "6.36");
}

/// **USD/INR — the case that makes C-3 non-negotiable.**
///
/// Booking the quote figure as USDC would overstate this position by a factor of ~88. The EM
/// pairs are the product's differentiation (`ARCHITECTURE.md` § 1.2) and *every one of them*
/// is quoted this way.
#[test]
fn usd_inr_would_be_wrong_by_eighty_eight_times_without_conversion() {
    let size = LOT; // 100,000 USD
    let entry = price(88.4200);
    let exit = price(88.5200); // +0.10 INR
    let usd_inr = exit;

    let quote = pnl::upnl_in_quote(size, entry, exit, Direction::Long).unwrap();
    assert_eq!(quote, 10_000_000_000, "₹10,000");

    let usd = pnl::upnl_in_collateral(
        size,
        entry,
        exit,
        Direction::Long,
        QuoteConversion::QuotePerUsd,
        usd_inr,
    )
    .unwrap();
    assert_eq!(format!("{:.2}", dollars(usd)), "112.97", "₹10,000 ÷ 88.52");

    // The error the conversion prevents, stated as a number.
    let ratio = quote as f64 / usd as f64;
    assert!(
        (ratio - 88.52).abs() < 0.01,
        "skipping the conversion overstates this position {ratio:.2}x"
    );
}

/// Gold: 1 lot is **100 ounces**, not 100,000 — but the engine stores base units, so the
/// difference is entirely in what the UI calls a lot. Nothing in the maths changes.
#[test]
fn gold_prices_at_its_own_contract_size() {
    let one_gold_lot = 100 * 1_000_000_000; // 100 oz at BASE_PRECISION
                                            // $10 move on 100 oz = $1,000.
    let usd = pnl::upnl_in_quote(
        one_gold_lot,
        price(4046.95),
        price(4056.95),
        Direction::Long,
    )
    .unwrap();
    assert_eq!(usd, 1_000_000_000, "100 oz × $10 = $1,000.00");
}

/// A short is the exact mirror of a long — the same figure with the sign flipped, to within
/// the one unit of protocol-favourable rounding.
#[test]
fn a_short_mirrors_the_long_on_the_screenshot_numbers() {
    let size = 60 * LOT;
    let entry = price(1.09659);
    let exit = price(1.09850);
    let usd_chf = price(0.810_370);

    let long = pnl::upnl_in_collateral(
        size,
        entry,
        exit,
        Direction::Long,
        QuoteConversion::QuotePerUsd,
        usd_chf,
    )
    .unwrap();
    let short = pnl::upnl_in_collateral(
        size,
        entry,
        exit,
        Direction::Short,
        QuoteConversion::QuotePerUsd,
        usd_chf,
    )
    .unwrap();

    assert!(
        (long + short).abs() <= 1,
        "long {long} and short {short} must be equal and opposite within one unit"
    );
    assert!(short < 0, "a short into a rising market must lose");
}

/// The notional a margin check runs on, converted the same way. 60 lots of GBP/CHF is
/// **not** 6,000,000 dollars — it is 6,000,000 GBP, worth ~6.6m CHF, worth ~$8.1m.
#[test]
fn notional_is_converted_before_any_margin_check_sees_it() {
    let size = 60 * LOT;
    let mark = price(1.09850);
    let usd_chf = price(0.810_370);

    let quote = pnl::notional_in_quote(size, mark).unwrap();
    assert_eq!(quote, 6_591_000_000_000, "6,591,000 CHF");

    let usd =
        pnl::notional_in_collateral(size, mark, QuoteConversion::QuotePerUsd, usd_chf).unwrap();
    // 6,591,000 CHF ÷ 0.810370 = $8,133,321.82
    assert_eq!(format!("{:.2}", usd as f64 / 1e6), "8133321.82");

    // Sizing margin off the unconverted figure would under-collateralise by ~19%.
    assert!(
        usd > quote,
        "CHF is worth more than a dollar, so the USD notional must be larger"
    );
}

/// A `XXX/USD` market converts the other way — multiply, not divide. Getting this backwards
/// is the failure `QuoteConversion` exists to make impossible to express.
#[test]
fn a_usd_per_quote_market_multiplies_instead_of_dividing() {
    // A hypothetical market quoted in EUR, with EUR/USD at 1.08543.
    let eur_usd = price(1.08543);
    let usd = pnl::upnl_in_collateral(
        LOT,
        price(2.00000),
        price(2.01000),
        Direction::Long,
        QuoteConversion::UsdPerQuote,
        eur_usd,
    )
    .unwrap();

    // 100,000 × 0.01 = €1,000 -> €1,000 × 1.08543 = $1,085.43
    assert_eq!(format!("{:.2}", dollars(usd)), "1085.43");
}
