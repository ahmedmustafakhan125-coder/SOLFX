import { describe, expect, it } from "vitest";
import { Direction } from "../generated/index.js";
import {
  confBps,
  confidenceSpreadBps,
  effectiveLeverage,
  executionPrice,
  initialMargin,
  minCollateralFor,
  notionalInQuote,
  quoteOpen,
  sideForClose,
  sideForOpen,
  skewDelta,
  skewImpactBps,
  totalSpreadBps,
  type SpreadParams,
} from "../pricing.js";

const PRICE = 1_000_000_000n; // PRICE_PRECISION
const ONE_LOT = 100_000_000_000_000n;

/**
 * Parity with `crates/solfx-math/src/pricing.rs` and `margin.rs`.
 *
 * The rounding directions are the whole point. Every one of them is a ceiling that works
 * against the trader, and a port that floors any of them produces a ticket the chain refuses.
 */
describe("spread components mirror pricing.rs", () => {
  it("rounds the confidence spread up", () => {
    // 7 bps of confidence at a 1.5x multiplier is 10.5 bps -> 11.
    expect(confidenceSpreadBps(7n, 15_000)).toBe(11n);
    expect(confidenceSpreadBps(0n, 15_000)).toBe(0n);
  });

  it("derives conf_bps from price the way ValidatedPrice does", () => {
    // conf 15.22 on a price of 77,358.25 -> 1.967 bps, rounded up.
    expect(confBps(77_358_250_000_000n, 15_220_000_000n)).toBe(2n);
    expect(confBps(0n, 1n)).toBe(0n);
  });

  it("charges skew only when the trade worsens the imbalance", () => {
    // Book already 10 lots short; a 3-lot buy pulls it toward flat.
    expect(skewDelta(0n, 10n * ONE_LOT, 3n * ONE_LOT)).toBe(-3n * ONE_LOT);
    expect(skewImpactBps(-3n * ONE_LOT, 1_000_000)).toBe(0n);

    // The same book, a 3-lot sell: three lots further out.
    expect(skewDelta(0n, 10n * ONE_LOT, -3n * ONE_LOT)).toBe(3n * ONE_LOT);
    // rate 1e6 = 0.001 bps per lot, so 3 lots is 0.003 bps -> ceils to 1.
    expect(skewImpactBps(3n * ONE_LOT, 1_000_000)).toBe(1n);
    expect(skewImpactBps(3n * ONE_LOT, 0)).toBe(0n);
  });

  it("refuses a total spread past the 50% ceiling", () => {
    expect(totalSpreadBps(6, 4n, 0n)).toBe(10n);
    expect(totalSpreadBps(5_000, 1n, 0n)).toBeUndefined();
  });

  it("moves the fill against the trader on both sides", () => {
    expect(executionPrice(100n * PRICE, 6n, "buy")).toBe(100_060_000_000n);
    expect(executionPrice(100n * PRICE, 6n, "sell")).toBe(99_940_000_000n);
    // Ceiling-rounded, so any non-zero spread moves the price by at least one unit.
    expect(executionPrice(1n, 1n, "buy")).toBe(2n);
    expect(executionPrice(1n, 1n, "sell")).toBeUndefined();
    expect(executionPrice(100n * PRICE, 5_001n, "buy")).toBeUndefined();
  });

  it("resolves the side the way Side::resolve does", () => {
    expect(sideForOpen(Direction.Long)).toBe("buy");
    expect(sideForOpen(Direction.Short)).toBe("sell");
    expect(sideForClose(Direction.Long)).toBe("sell");
    expect(sideForClose(Direction.Short)).toBe("buy");
  });
});

describe("notional and leverage mirror the program's checks", () => {
  it("ceils the notional, as notional_in_quote does", () => {
    // 1 wei of base at 1 unit of price is a fraction of a micro-USDC, and still costs one.
    expect(notionalInQuote(1n, 1n)).toBe(1n);
    expect(notionalInQuote(ONE_LOT, PRICE)).toBe(100_000_000_000n);
  });

  it("ceils effective leverage", () => {
    expect(effectiveLeverage(1_000n, 100n)).toBe(10n);
    // One unit over exactly 10x is 11x as far as the program is concerned.
    expect(effectiveLeverage(1_001n, 100n)).toBe(11n);
    expect(effectiveLeverage(1_000n, 0n)).toBeUndefined();
  });

  it("ceils the initial margin", () => {
    expect(initialMargin(1_000n, 1_000)).toBe(100n);
    expect(initialMargin(1_001n, 1_000)).toBe(101n);
  });
});

/**
 * The devnet failure this module was written for.
 *
 * ETH/USD, market #9: `base_spread_bps` 6, `imr_bps` 1000, `max_leverage` 10, and a book
 * already 9,990 USDC short. A long of ~3.993 ETH at 10x was refused with `LeverageTooHigh`
 * (error 6047, `open_position.rs:157`) because the ticket sized margin off the oracle mid.
 */
describe("the ETH/USD 10x refusal", () => {
  const eth: SpreadParams = {
    baseSpreadBps: 6,
    confSpreadMultiplierBps: 0,
    skewImpactBpsPerUnit: 0,
    baseOiLong: 0n,
    baseOiShort: 9_990_000_000n,
  };
  const mid = 2_502_000_000_000n; // ~$2,502
  const sizeBase = 3_993_369_904n;
  const MAX_LEVERAGE = 10;
  const IMR_BPS = 1_000;

  /** What the ticket used to do: notional at the mid, margin floored. */
  const midNotional = (sizeBase * mid) / 1_000_000_000_000n;
  const oldMargin = midNotional / BigInt(MAX_LEVERAGE);

  it("reproduces the refusal when margin is sized off the mid", () => {
    const q = quoteOpen({
      market: eth,
      oraclePrice: mid,
      confidenceBps: 0n,
      sizeBase,
      side: "buy",
    });
    expect(q).toBeDefined();
    // The program's notional is larger than the one the ticket quoted...
    expect(q!.notional).toBeGreaterThan(midNotional);
    // ...so ceil(notional / collateral) lands at 11 against a 10x cap.
    expect(effectiveLeverage(q!.notional, oldMargin)).toBe(11n);
  });

  it("accepts the same trade once margin is sized off the fill", () => {
    const q = quoteOpen({
      market: eth,
      oraclePrice: mid,
      confidenceBps: 0n,
      sizeBase,
      side: "buy",
    })!;
    const collateral = minCollateralFor({
      notional: q.notional,
      leverage: MAX_LEVERAGE,
      imrBps: IMR_BPS,
    });

    expect(effectiveLeverage(q.notional, collateral)).toBeLessThanOrEqual(
      BigInt(MAX_LEVERAGE),
    );
    expect(collateral).toBeGreaterThanOrEqual(
      initialMargin(q.notional, IMR_BPS),
    );
    // The correction is small — this was never a sizing error, only a rounding one.
    expect(collateral - oldMargin).toBeLessThan(1_000_000n);
  });

  it("explains why the short at the same leverage went through", () => {
    // A sell fills below the mid, so its notional is smaller and the same floored margin
    // happened to survive the ceiling. That is luck, not correctness.
    const sell = quoteOpen({
      market: eth,
      oraclePrice: mid,
      confidenceBps: 0n,
      sizeBase,
      side: "sell",
    })!;
    expect(sell.notional).toBeLessThan(midNotional);
    expect(effectiveLeverage(sell.notional, oldMargin)).toBe(10n);
  });
});

/**
 * The property that matters: whatever the market, size or side, collateral from
 * `minCollateralFor` must satisfy *both* of the program's checks. If this holds, the ticket
 * cannot quote a trade the chain refuses for margin reasons.
 */
describe("minCollateralFor satisfies both program checks", () => {
  const markets: readonly SpreadParams[] = [
    {
      baseSpreadBps: 6,
      confSpreadMultiplierBps: 0,
      skewImpactBpsPerUnit: 0,
      baseOiLong: 0n,
      baseOiShort: 9_990_000_000n,
    },
    {
      baseSpreadBps: 1,
      confSpreadMultiplierBps: 15_000,
      skewImpactBpsPerUnit: 1_000_000,
      baseOiLong: 5n * ONE_LOT,
      baseOiShort: 0n,
    },
    {
      baseSpreadBps: 30,
      confSpreadMultiplierBps: 20_000,
      skewImpactBpsPerUnit: 5_000_000,
      baseOiLong: 0n,
      baseOiShort: 0n,
    },
  ];
  const prices = [1_000_000_000n, 77_358_250_000_000n, 88_500_000_000n];
  const sizes = [1_000_000n, 1_425_760n, 3_993_369_904n, ONE_LOT];
  const levs = [1, 3, 10, 50];

  it("holds across markets, prices, sizes, sides and leverages", () => {
    for (const market of markets) {
      for (const oraclePrice of prices) {
        for (const sizeBase of sizes) {
          for (const side of ["buy", "sell"] as const) {
            const q = quoteOpen({
              market,
              oraclePrice,
              confidenceBps: confBps(oraclePrice, oraclePrice / 5_000n),
              sizeBase,
              side,
            });
            if (!q || q.notional === 0n) continue;
            for (const leverage of levs) {
              const imrBps = Math.ceil(10_000 / leverage);
              const c = minCollateralFor({
                notional: q.notional,
                leverage,
                imrBps,
              });
              expect(effectiveLeverage(q.notional, c)).toBeLessThanOrEqual(
                BigInt(leverage),
              );
              expect(c).toBeGreaterThanOrEqual(
                initialMargin(q.notional, imrBps),
              );
            }
          }
        }
      }
    }
  });
});
