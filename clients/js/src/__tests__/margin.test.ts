import { describe, expect, it } from "vitest";
import { Direction } from "../generated/index.js";
import {
  liquidationPrice,
  liquidationPriceIsExact,
  maintenanceMargin,
  mmrMultiplierBps,
} from "../margin.js";

/** One USDC in native units. */
const USD = 1_000_000n;
/** One standard FX lot in base units. */
const ONE_LOT = 100_000_000_000_000n;

const NO_COSTS = { carry: 0n, funding: 0n, closeFee: 0n } as const;

/**
 * Parity with `crates/solfx-math/src/margin.rs`.
 *
 * Every expected value here is the assertion from the Rust test of the same name, copied
 * rather than recomputed. That is the point: if the port drifts from the engine — a rounding
 * direction, a sign, a tier boundary — these fail, and a liquidation price that disagrees with
 * the chain is exactly the bug worth catching in CI rather than on someone's position.
 */
describe("liquidationPrice matches solfx-math", () => {
  // margin.rs: liquidation_price_matches_the_explainer_worked_example
  it("matches the explainer worked example", () => {
    expect(
      liquidationPrice({
        sizeBase: ONE_LOT,
        entryPrice: 1_085_000_000n,
        collateral: 2_170n * USD,
        maintenanceMargin: 1_085n * USD,
        costs: NO_COSTS,
        direction: Direction.Long,
      }),
    ).toBe(1_074_150_000n);
  });

  // margin.rs: short_liquidation_price_is_above_entry
  it("puts a short's liquidation above entry", () => {
    const entry = 1_085_000_000n;
    const liq = liquidationPrice({
      sizeBase: ONE_LOT,
      entryPrice: entry,
      collateral: 2_170n * USD,
      maintenanceMargin: 1_085n * USD,
      costs: NO_COSTS,
      direction: Direction.Short,
    });
    expect(liq).toBe(1_095_850_000n);
    expect(liq! > entry).toBe(true);
  });

  // margin.rs: accrued_costs_pull_liquidation_price_toward_entry
  it("pulls liquidation toward entry as costs accrue", () => {
    const common = {
      sizeBase: ONE_LOT,
      entryPrice: 1_085_000_000n,
      collateral: 2_170n * USD,
      maintenanceMargin: 1_085n * USD,
      direction: Direction.Long,
    };
    const clean = liquidationPrice({ ...common, costs: NO_COSTS })!;
    const dirty = liquidationPrice({
      ...common,
      costs: { carry: 50n * USD, funding: 10n * USD, closeFee: 5n * USD },
    })!;
    expect(dirty > clean).toBe(true);
  });

  /**
   * The Rust clamp fires when the loss budget is wider than the entry price itself — a long
   * so over-collateralised that the arithmetic wants a negative liquidation price. Not, as it
   * first appears, when the position is already underwater: a long past liquidation produces a
   * price *above* entry, which is meaningless but positive.
   */
  it("clamps to 1 when the buffer is wider than the entry price", () => {
    expect(
      liquidationPrice({
        sizeBase: ONE_LOT,
        entryPrice: 1_085_000_000n,
        collateral: 1_000_000n * USD,
        maintenanceMargin: 1n,
        costs: NO_COSTS,
        direction: Direction.Long,
      }),
    ).toBe(1n);
  });

  it("refuses a zero size rather than dividing by it", () => {
    expect(
      liquidationPrice({
        sizeBase: 0n,
        entryPrice: 1_085_000_000n,
        collateral: 2_170n * USD,
        maintenanceMargin: 1_085n * USD,
        costs: NO_COSTS,
        direction: Direction.Long,
      }),
    ).toBeUndefined();
  });
});

describe("mmrMultiplierBps matches the architecture table", () => {
  // margin.rs: mmr_tiers_match_the_architecture_table
  it("returns the documented multipliers", () => {
    expect(mmrMultiplierBps(50_000n * USD)).toBe(10_000n);
    expect(mmrMultiplierBps(500_000n * USD)).toBe(15_000n);
    expect(mmrMultiplierBps(2_000_000n * USD)).toBe(25_000n);
    expect(mmrMultiplierBps(10_000_000n * USD)).toBe(40_000n);
  });

  // margin.rs: mmr_tier_boundaries_are_exclusive_upper
  it("treats tier boundaries as exclusive-upper", () => {
    expect(mmrMultiplierBps(100_000n * USD - 1n)).toBe(10_000n);
    expect(mmrMultiplierBps(100_000n * USD)).toBe(15_000n);
    expect(mmrMultiplierBps(1_000_000n * USD - 1n)).toBe(15_000n);
    expect(mmrMultiplierBps(1_000_000n * USD)).toBe(25_000n);
    expect(mmrMultiplierBps(5_000_000n * USD - 1n)).toBe(25_000n);
    expect(mmrMultiplierBps(5_000_000n * USD)).toBe(40_000n);
  });
});

describe("maintenanceMargin", () => {
  /** Rounded up twice, so it can never understate what the position must hold. */
  it("rounds up rather than down", () => {
    // 1 unit of notional at 1 bp is a fraction of a unit; ceiling makes it 1, not 0.
    expect(maintenanceMargin(1n, 1)).toBe(1n);
  });
});

describe("liquidationPriceIsExact", () => {
  it("is exact only for USD-quoted markets", () => {
    for (const s of ["EUR/USD", "XAU/USD", "BTC/USD", "XAGUSD"]) {
      expect(liquidationPriceIsExact(s)).toBe(true);
    }
    // The quote currency is not USD, so the conversion rate moves the answer too.
    for (const s of ["USD/JPY", "USD/CNH"]) {
      expect(liquidationPriceIsExact(s)).toBe(false);
    }
  });
});
