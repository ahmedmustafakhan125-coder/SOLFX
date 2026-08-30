import { describe, expect, it } from "vitest";

import { maxPositionBase, minPositionBase, unitsPerLot } from "../contracts.js";
import {
  SizingError,
  lotsToBase,
  notionalToBase,
  quantityToBase,
  resolveSize,
} from "../sizing.js";

// Every vector below is copied from the Rust tests in contracts.rs and trade.rs. Two clients
// that disagree about size are the failure this file exists to prevent — and the CLI is the
// one already proven against a real cluster, so it is the reference, not this.

describe("a lot means different amounts per asset class", () => {
  it.each([
    ["EUR/USD", 100_000n],
    ["USD/INR", 100_000n],
    ["XAU/USD", 100n],
    ["XAG/USD", 5_000n],
    ["BTC/USD", 1n],
    ["ETH/USD", 1n],
    ["USOILSPOT/USD", 1_000n],
  ])("%s", (symbol, expected) => {
    expect(unitsPerLot(symbol)).toBe(expected);
  });
});

// The failure this prevents: bounds derived from the FX lot made the *minimum* BTC position
// 0.1 BTC (~$7,700), so a $1,000 order was rejected as too small.
describe("position bounds track the contract size", () => {
  it("BTC", () => {
    expect(maxPositionBase("BTC/USD")).toBe(100_000_000_000n);
    expect(minPositionBase("BTC/USD")).toBe(1_000n);
  });

  it("a realistic $1,000 BTC order clears both bounds", () => {
    const realistic = 12_900_000n; // ~0.0129 BTC at ~$77k
    expect(realistic).toBeGreaterThanOrEqual(minPositionBase("BTC/USD"));
    expect(realistic).toBeLessThanOrEqual(maxPositionBase("BTC/USD"));
  });

  it("FX keeps the reference figures", () => {
    expect(minPositionBase("EUR/USD")).toBe(100_000_000n);
  });

  it.each(["BTC/USD", "XAU/USD", "EUR/USD", "USOILSPOT/USD"])("%s bounds order", (s) => {
    expect(minPositionBase(s)).toBeGreaterThan(0n);
    expect(minPositionBase(s)).toBeLessThan(maxPositionBase(s));
  });
});

describe("lots convert to base units", () => {
  const fx = unitsPerLot("EUR/USD");

  it.each([
    ["1", 100_000_000_000_000n],
    ["0.1", 10_000_000_000_000n],
    ["0.01", 1_000_000_000_000n],
    [" 0.01 ", 1_000_000_000_000n],
  ])("%s lots", (lots, expected) => {
    expect(lotsToBase(lots, fx)).toBe(expected);
  });

  // 0.07 is not representable in binary: `0.07 * 10_000` is 699.9999... in floating point,
  // and truncating sizes the trade one ten-thousandth of a lot short, silently.
  it.each([
    ["0.07", 7_000_000_000_000n],
    ["0.29", 29_000_000_000_000n],
    ["2.35", 235_000_000_000_000n],
  ])("%s is exact, not floating point", (lots, expected) => {
    expect(lotsToBase(lots, fx)).toBe(expected);
  });

  it.each(["0", "-1", "", "abc", "1.2.3", "0.00001"])("rejects %s", (bad) => {
    expect(() => lotsToBase(bad, fx)).toThrow(SizingError);
  });
});

describe("quantity", () => {
  it("accepts decimals", () => {
    expect(quantityToBase("0.5")).toBe(500_000_000n);
    expect(quantityToBase("1000")).toBe(1_000_000_000_000n);
  });

  it.each(["0", "-1", "abc"])("rejects %s", (bad) => {
    expect(() => quantityToBase(bad)).toThrow(SizingError);
  });
});

// § the whole point of having three modes: they must agree where they overlap.
// `quantity` is the anchor — the only one with no dependency on price or contract size.
describe("the three sizing modes agree", () => {
  it("1,000 EUR is 0.01 FX lots", () => {
    expect(quantityToBase("1000")).toBe(lotsToBase("0.01", unitsPerLot("EUR/USD")));
  });

  it("1 BTC is 1 BTC lot, because a BTC lot is one coin", () => {
    expect(quantityToBase("1")).toBe(lotsToBase("1", unitsPerLot("BTC/USD")));
  });

  it("$77,000 buys ~1 BTC at $77,000", () => {
    const oneBtc = quantityToBase("1");
    const byNotional = notionalToBase(77_000n, 77_000_000_000_000n);
    const diff = byNotional > oneBtc ? byNotional - oneBtc : oneBtc - byNotional;
    expect(diff).toBeLessThan(oneBtc / 1_000n); // within 0.1%
  });
});

describe("notional inverts the notional formula", () => {
  const btc = 77_000_000_000_000n; // $77,000 at PRICE_PRECISION

  it("$1,000 round-trips back to ~$1,000 of exposure", () => {
    const size = notionalToBase(1_000n, btc);
    const backQuote = (size * btc) / 1_000_000_000_000n; // NOTIONAL_DIVISOR
    const dollars = backQuote / 1_000_000n;
    expect(dollars).toBeGreaterThanOrEqual(999n);
    expect(dollars).toBeLessThanOrEqual(1_001n);
  });

  it("rejects nonsense", () => {
    expect(() => notionalToBase(0n, btc)).toThrow(SizingError);
    expect(() => notionalToBase(1_000n, 0n)).toThrow(SizingError);
  });
});

describe("resolveSize", () => {
  it("routes each mode", () => {
    expect(resolveSize("EUR/USD", { mode: "lots", value: "0.01" })).toBe(1_000_000_000_000n);
    expect(resolveSize("EUR/USD", { mode: "quantity", value: "1000" })).toBe(1_000_000_000_000n);
    expect(
      resolveSize("BTC/USD", { mode: "notional", usd: 1_000n }, 77_000_000_000_000n),
    ).toBeGreaterThan(0n);
  });

  it("refuses notional without a price rather than guessing one", () => {
    expect(() => resolveSize("BTC/USD", { mode: "notional", usd: 1_000n })).toThrow(SizingError);
  });
});
