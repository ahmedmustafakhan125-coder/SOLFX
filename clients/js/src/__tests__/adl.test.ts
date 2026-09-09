import { describe, expect, it } from "vitest";
import {
  adlExposure,
  adlRankBps,
  adlWithholding,
  isAdlEligible,
} from "../adl.js";

const USD = 1_000_000n;

describe("adlRankBps", () => {
  it("ranks by unrealised profit against collateral, as § 6.9 specifies", () => {
    // $50 of profit on $100 of collateral is 50% — 5,000 bps.
    expect(adlRankBps(50n * USD, 100n * USD)).toBe(5_000n);
    expect(adlRankBps(100n * USD, 100n * USD)).toBe(10_000n);
    expect(adlRankBps(1n * USD, 100n * USD)).toBe(100n);
  });

  it("puts a smaller position with a bigger proportional gain ahead of a larger one", () => {
    // The whole point of ranking by proportion rather than absolute profit: $60 on $100
    // outranks $500 on $10,000, and the queue must reflect that.
    const small = adlRankBps(60n * USD, 100n * USD);
    const large = adlRankBps(500n * USD, 10_000n * USD);
    expect(small).toBeGreaterThan(large);
  });

  it("is negative for a losing position, which sorts it to the back", () => {
    expect(adlRankBps(-25n * USD, 100n * USD)).toBe(-2_500n);
  });

  it("returns zero rather than dividing by zero", () => {
    // Invariant I5 forbids size without collateral, so this is a guard and not a case.
    expect(adlRankBps(50n * USD, 0n)).toBe(0n);
    expect(adlRankBps(50n * USD, -1n)).toBe(0n);
  });
});

describe("isAdlEligible", () => {
  it("admits only positions in profit, which is the program's own condition", () => {
    // adl.rs: require!(health.upnl > 0, SolfxError::NotAdlEligible)
    expect(isAdlEligible(1n)).toBe(true);
    expect(isAdlEligible(0n)).toBe(false);
    expect(isAdlEligible(-1n)).toBe(false);
  });
});

describe("adlExposure", () => {
  it("calls ADL active only when a shortfall is actually recorded", () => {
    expect(
      adlExposure({ pendingDebt: 0n, insuranceBalance: 50_000n * USD }).active,
    ).toBe(false);
    expect(
      adlExposure({ pendingDebt: 1n, insuranceBalance: 50_000n * USD }).active,
    ).toBe(true);
  });

  it("reports an empty insurance fund without inventing an alarm", () => {
    // An empty fund is not itself ADL: nothing has gapped yet. Conflating the two would cry
    // wolf on a venue that is merely young.
    const e = adlExposure({ pendingDebt: 0n, insuranceBalance: 0n });
    expect(e.active).toBe(false);
    expect(e.insuranceBalance).toBe(0n);
  });
});

describe("adlWithholding", () => {
  it("never takes principal — the worst case is being returned to flat", () => {
    const { withheld, profitKept } = adlWithholding(100n * USD, 500n * USD);
    expect(withheld).toBe(100n * USD); // the whole profit, and not a unit more
    expect(profitKept).toBe(0n);
  });

  it("stops at the shortfall and leaves the rest of the profit", () => {
    const { withheld, profitKept } = adlWithholding(100n * USD, 30n * USD);
    expect(withheld).toBe(30n * USD);
    expect(profitKept).toBe(70n * USD);
  });

  it("takes nothing from a position that is not in profit", () => {
    expect(adlWithholding(0n, 500n * USD)).toStrictEqual({
      withheld: 0n,
      profitKept: 0n,
    });
    expect(adlWithholding(-40n * USD, 500n * USD)).toStrictEqual({
      withheld: 0n,
      profitKept: 0n,
    });
  });

  it("conserves the profit: what is withheld plus what is kept is what there was", () => {
    for (const [pnl, shortfall] of [
      [100n * USD, 1n],
      [100n * USD, 99n * USD],
      [7n, 3n],
      [1n, 1n],
    ] as const) {
      const { withheld, profitKept } = adlWithholding(pnl, shortfall);
      expect(withheld + profitKept).toBe(pnl);
    }
  });
});
