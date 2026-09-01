import { describe, expect, it } from "vitest";
import { Direction } from "../generated/index.js";
import { carryOver, carryRatePerHour, notionalOf } from "../carry.js";

const RATE = 1_000_000_000n;

describe("carryRatePerHour", () => {
  /**
   * The whole reason the two components are separate fields on `Market`: a trader can see the
   * real interest differential and what SolFX adds to it, rather than one blended number.
   */
  it("keeps the differential and the markup apart", () => {
    const r = carryRatePerHour({
      baseAnnual: 2n * RATE, // 200% — deliberately large so the hourly figure is exact
      quoteAnnual: 5n * RATE,
      markupPerHour: 100n,
      direction: Direction.Long,
    });
    expect(r.differential).toBe(mulCeil(3n * RATE, 365n * 24n));
    expect(r.markup).toBe(100n);
    expect(r.total).toBe(r.differential + 100n);
  });

  /** A short has the mirrored exposure, so the differential flips — but the markup does not. */
  it("flips the differential for a short and never the markup", () => {
    const args = { baseAnnual: 2n * RATE, quoteAnnual: 5n * RATE, markupPerHour: 100n };
    const long = carryRatePerHour({ ...args, direction: Direction.Long });
    const short = carryRatePerHour({ ...args, direction: Direction.Short });
    expect(short.differential < 0n).toBe(true);
    expect(long.differential > 0n).toBe(true);
    expect(short.markup).toBe(long.markup);
  });
});

describe("carryOver", () => {
  it("is zero when the net rate is not a charge", () => {
    // `advance_carry_index` refuses to run backwards; a negative estimate would promise a
    // payment the chain will never make.
    expect(carryOver(1_000_000n, -50n, 24n)).toBe(0n);
    expect(carryOver(1_000_000n, 0n, 24n)).toBe(0n);
  });

  it("scales with notional and hours", () => {
    const oneHour = carryOver(1_000_000_000n, 1_000n, 1n);
    expect(carryOver(1_000_000_000n, 1_000n, 24n)).toBe(oneHour * 24n);
    expect(carryOver(2_000_000_000n, 1_000n, 1n)).toBe(oneHour * 2n);
  });
});

describe("notionalOf", () => {
  it("converts one lot of EUR/USD at 1.0850 to $108,500", () => {
    // 1 lot = 100,000 units at BASE_PRECISION 1e9; price 1.0850 at PRICE_PRECISION 1e9.
    expect(notionalOf(100_000_000_000_000n, 1_085_000_000n)).toBe(108_500_000_000n);
  });
});

/** `ceil(a / d)` for positive values, matching `mul_div_ceil_signed`. */
function mulCeil(a: bigint, d: bigint): bigint {
  return a / d + (a % d > 0n ? 1n : 0n);
}
