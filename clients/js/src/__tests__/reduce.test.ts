import { describe, expect, it } from "vitest";
import {
  pnlOnClosedPortion,
  previewReduce,
  reduceIsFullClose,
  sizeForPercent,
  type ReduciblePosition,
} from "../reduce.js";

/** One standard FX lot in base units, as `margin.test.ts` defines it. */
const ONE_LOT = 100_000_000_000_000n;
const USD = 1_000_000n;

/**
 * A position deliberately built so every proportion divides unevenly. Round numbers cannot
 * fail a flooring test.
 */
const AWKWARD: ReduciblePosition = {
  sizeBase: 333_333_333_333_333n,
  collateral: 1_000_000_007n,
  entryNotional: 3_666_666_663n,
};

describe("the full-close boundary", () => {
  const p = { sizeBase: ONE_LOT };

  it("treats an equal size as a full close, not a reduction", () => {
    // decrease_position requires size_delta < size_base, strictly. Getting this wrong is
    // ReductionExceedsSize on a transaction the trader has already signed.
    expect(reduceIsFullClose(ONE_LOT, p)).toBe(true);
    expect(reduceIsFullClose(ONE_LOT - 1n, p)).toBe(false);
  });

  it("treats an oversized delta as a full close rather than pretending it is valid", () => {
    expect(reduceIsFullClose(ONE_LOT * 2n, p)).toBe(true);
  });
});

describe("sizeForPercent", () => {
  it("returns the exact size at 100, not a floored approximation", () => {
    // The trap: flooring at 100% leaves a dust position and routes to decrease_position,
    // which then rejects the next attempt to close it in one go.
    expect(sizeForPercent(AWKWARD, 100)).toBe(AWKWARD.sizeBase);
    expect(reduceIsFullClose(sizeForPercent(AWKWARD, 100), AWKWARD)).toBe(true);
  });

  it("floors every other percentage", () => {
    expect(sizeForPercent(AWKWARD, 50)).toBe(AWKWARD.sizeBase / 2n);
    expect(sizeForPercent(AWKWARD, 25)).toBe((AWKWARD.sizeBase * 25n) / 100n);
    // Floored, so a preset never asks for more than it names.
    expect(sizeForPercent(AWKWARD, 33)).toBeLessThanOrEqual(
      (AWKWARD.sizeBase * 33n) / 100n,
    );
  });

  it("returns nothing for a non-positive percentage", () => {
    expect(sizeForPercent(AWKWARD, 0)).toBe(0n);
    expect(sizeForPercent(AWKWARD, -10)).toBe(0n);
  });
});

/**
 * Parity with `reduce` in `close_position.rs`: collateral and entry notional are floored
 * proportions, and the remainder keeps the odd unit.
 */
describe("previewReduce splits the way the program does", () => {
  it("halves a position without losing a unit", () => {
    const p = previewReduce(AWKWARD, AWKWARD.sizeBase / 2n);
    expect(p.isFullClose).toBe(false);
    expect(p.remainingSize).toBe(AWKWARD.sizeBase - AWKWARD.sizeBase / 2n);
    // Conservation: nothing is created or destroyed by the split.
    expect(p.releasedCollateral + p.remainingCollateral).toBe(
      AWKWARD.collateral,
    );
    expect(p.closedEntryNotional + p.remainingEntryNotional).toBe(
      AWKWARD.entryNotional,
    );
  });

  it("floors the released collateral, so the remainder keeps the odd unit", () => {
    // collateral 1_000_000_007 x (size/2) / size floors to 500_000_003, not ...3.5.
    const p = previewReduce(AWKWARD, AWKWARD.sizeBase / 2n);
    const exactHalf = AWKWARD.collateral / 2n;
    expect(p.releasedCollateral).toBeLessThanOrEqual(exactHalf);
    expect(p.remainingCollateral).toBeGreaterThanOrEqual(
      AWKWARD.collateral - exactHalf,
    );
  });

  it("releases everything on a full close", () => {
    const p = previewReduce(AWKWARD, AWKWARD.sizeBase);
    expect(p.isFullClose).toBe(true);
    expect(p.releasedCollateral).toBe(AWKWARD.collateral);
    expect(p.closedEntryNotional).toBe(AWKWARD.entryNotional);
    expect(p.remainingSize).toBe(0n);
    expect(p.remainingCollateral).toBe(0n);
    expect(p.remainingEntryNotional).toBe(0n);
  });

  it("rejects the two sizes the program rejects", () => {
    expect(() => previewReduce(AWKWARD, 0n)).toThrow(/ZeroAmount/);
    expect(() => previewReduce(AWKWARD, AWKWARD.sizeBase + 1n)).toThrow(
      /ReductionExceedsSize/,
    );
  });

  /**
   * Repeated partial closes must never release more collateral than the position held. The
   * property that matters: flooring at each step cannot accumulate into an over-release.
   */
  it("never releases more than the position holds, however it is sliced", () => {
    let live: ReduciblePosition = AWKWARD;
    let released = 0n;
    for (const pct of [10, 25, 33, 50, 90]) {
      const delta = sizeForPercent(live, pct);
      if (delta === 0n) continue;
      const p = previewReduce(live, delta);
      released += p.releasedCollateral;
      live = {
        sizeBase: p.remainingSize,
        collateral: p.remainingCollateral,
        entryNotional: p.remainingEntryNotional,
      };
    }
    // Whatever is left plus everything released is exactly what we started with.
    expect(released + live.collateral).toBe(AWKWARD.collateral);
    expect(released).toBeLessThanOrEqual(AWKWARD.collateral);
  });
});

describe("pnlOnClosedPortion", () => {
  // One lot of EUR/USD entered at 1.10000, marked at 1.10500: 50 pips on 100,000 units.
  const position: ReduciblePosition = {
    sizeBase: ONE_LOT,
    collateral: 1_000n * USD,
    entryNotional: 110_000n * USD,
  };
  const MARK = 1_105_000_000n; // 1.105 at PRICE_PRECISION

  it("scales with the fraction closed", () => {
    const whole = previewReduce(position, position.sizeBase);
    const half = previewReduce(position, position.sizeBase / 2n);

    const pnlWhole = pnlOnClosedPortion(whole, MARK, true);
    const pnlHalf = pnlOnClosedPortion(half, MARK, true);

    expect(pnlWhole).toBe(500n * USD); // $500 on a standard lot
    expect(pnlHalf).toBe(250n * USD);
  });

  it("is antisymmetric in direction", () => {
    const half = previewReduce(position, position.sizeBase / 2n);
    expect(pnlOnClosedPortion(half, MARK, true)).toBe(
      -pnlOnClosedPortion(half, MARK, false),
    );
  });

  it("is a loss for a long when the mark is below entry", () => {
    const below = 1_095_000_000n;
    const whole = previewReduce(position, position.sizeBase);
    expect(pnlOnClosedPortion(whole, below, true)).toBe(-500n * USD);
    expect(pnlOnClosedPortion(whole, below, false)).toBe(500n * USD);
  });
});

/**
 * `app/src/lib/positions.ts` builds `decrease_position` from the *same* derivations as
 * `close_position`, on the grounds that the two take the same accounts in the same order.
 * That is a claim about generated code, so it is asserted here rather than believed.
 *
 * It is also not quite true, and the exception is the interesting part: `close_position`
 * takes the authority **writable** because it closes the position account and refunds its
 * rent to them; `decrease_position` closes nothing and correctly leaves the authority
 * read-only. Same addresses, same order, one different mutability — asserted so that if
 * either ever changes, it changes here rather than on someone's position.
 */
describe("decrease_position and close_position take the same accounts", async () => {
  const {
    getClosePositionInstruction,
    getDecreasePositionInstruction,
  } = await import("../generated/index.js");

  const A = (s: string) => s as never;
  const signer = { address: A("7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs"), signTransactions: async () => [] } as never;
  const shared = {
    authority: signer,
    protocol: A("11111111111111111111111111111111"),
    userAccount: A("11111111111111111111111111111112"),
    market: A("11111111111111111111111111111113"),
    position: A("11111111111111111111111111111114"),
    collateralVault: A("11111111111111111111111111111115"),
    lpPool: A("11111111111111111111111111111116"),
    lpVault: A("11111111111111111111111111111117"),
    insuranceFund: A("11111111111111111111111111111118"),
    insuranceVault: A("11111111111111111111111111111119"),
    feeVault: A("1111111111111111111111111111111A"),
    priceUpdate: A("1111111111111111111111111111111B"),
    priceLimit: 0n,
  };

  it("in the same order, at the same addresses", () => {
    const close = getClosePositionInstruction(shared);
    const decrease = getDecreasePositionInstruction({
      ...shared,
      sizeDelta: 1_000n,
    });

    expect(decrease.accounts.map((a) => a.address)).toStrictEqual(
      close.accounts.map((a) => a.address),
    );
    // Everything but the authority carries the same role.
    expect(decrease.accounts.slice(1).map((a) => a.role)).toStrictEqual(
      close.accounts.slice(1).map((a) => a.role),
    );
  });

  it("differing only in whether the authority is writable, and only because of rent", () => {
    const close = getClosePositionInstruction(shared);
    const decrease = getDecreasePositionInstruction({
      ...shared,
      sizeDelta: 1_000n,
    });
    // AccountRole: 2 READONLY_SIGNER, 3 WRITABLE_SIGNER.
    expect(close.accounts[0]!.role).toBe(3); // refunded the position account's rent
    expect(decrease.accounts[0]!.role).toBe(2); // closes nothing, so needs nothing back
  });

  it("and differ only in the instruction data", () => {
    const close = getClosePositionInstruction(shared);
    const decrease = getDecreasePositionInstruction({
      ...shared,
      sizeDelta: 1_000n,
    });
    expect(decrease.data).not.toStrictEqual(close.data);
    // The size rides in the data; a reduction is a close that names an amount.
    expect(decrease.data.length).toBe(close.data.length + 8);
  });
});
