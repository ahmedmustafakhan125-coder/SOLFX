import { describe, expect, it } from "vitest";
import { findTriggerOrderPda, nox, noxPdas } from "@solfx/client";
import type { Address, TransactionSigner } from "@solana/kit";

import {
  defaultPriceLimit,
  entryBracketRefusal,
  entryMet,
  EntryKind,
  evalPlaceEntryOrderIx,
  fundedMoveStopIx,
  fundedReduceIx,
  holdLeftSecs,
  isTighter,
  nextFreeId,
} from "@/lib/noxorders";
import { evalObserveIx, solfxAccounts } from "@/lib/nox";

const LONG = nox.Direction.Long;
const SHORT = nox.Direction.Short;
const LIMIT = EntryKind.Limit;
const STOP = EntryKind.Stop;

describe("isTighter — toward the price, strictly, as the program requires", () => {
  it("raises a long's stop and lowers a short's", () => {
    expect(isTighter(LONG, 100n, 101n)).toBe(true);
    expect(isTighter(LONG, 100n, 100n)).toBe(false);
    expect(isTighter(LONG, 100n, 99n)).toBe(false);
    expect(isTighter(SHORT, 100n, 99n)).toBe(true);
    expect(isTighter(SHORT, 100n, 100n)).toBe(false);
    expect(isTighter(SHORT, 100n, 101n)).toBe(false);
  });
});

describe("entryMet — the program's four cases, inclusive", () => {
  it.each([
    [LIMIT, LONG, 100n, true],
    [LIMIT, LONG, 101n, false],
    [LIMIT, SHORT, 100n, true],
    [LIMIT, SHORT, 99n, false],
    [STOP, LONG, 100n, true],
    [STOP, LONG, 99n, false],
    [STOP, SHORT, 100n, true],
    [STOP, SHORT, 101n, false],
  ] as const)("%s %s at %s → %s", (kind, dir, price, want) => {
    expect(entryMet(kind, dir, 100n, price)).toBe(want);
  });

  it("a buy limit is a sell stop's condition, at any price", () => {
    for (const price of [1n, 99n, 100n, 101n, 10_000n]) {
      expect(entryMet(LIMIT, LONG, 100n, price)).toBe(
        entryMet(STOP, SHORT, 100n, price)
      );
    }
  });
});

describe("defaultPriceLimit — there is no unbounded order", () => {
  it("bounds a limit at its own price", () => {
    expect(defaultPriceLimit(LIMIT, LONG, 1_000n, 50)).toBe(1_000n);
    expect(defaultPriceLimit(LIMIT, SHORT, 1_000n, 50)).toBe(1_000n);
  });
  it("bounds a stop beyond its trigger, on the side it chases", () => {
    expect(defaultPriceLimit(STOP, LONG, 10_000n, 50)).toBe(10_050n);
    expect(defaultPriceLimit(STOP, SHORT, 10_000n, 50)).toBe(9_950n);
  });
});

describe("holdLeftSecs", () => {
  it("counts down the ten minutes and stops at zero", () => {
    expect(holdLeftSecs(1_000n, 1_000n)).toBe(600);
    expect(holdLeftSecs(1_000n, 1_599n)).toBe(1);
    expect(holdLeftSecs(1_000n, 1_600n)).toBe(0);
    expect(holdLeftSecs(1_000n, 9_999n)).toBe(0);
  });
});

describe("nextFreeId", () => {
  it("takes the lowest id nothing occupies", () => {
    expect(nextFreeId([])).toBe(0);
    expect(nextFreeId([0, 1, 3])).toBe(2);
  });
  it("says so when every id is taken", () => {
    expect(
      nextFreeId(Array.from({ length: 256 }, (_, i) => i))
    ).toBeUndefined();
  });
});

describe("entryBracketRefusal — what placement refuses without a price", () => {
  const ok = {
    direction: LONG,
    trigger: 100n,
    stop: 90n,
    takeProfit: 0n,
    sizeBase: 1n,
  };
  it("accepts a bracket the right way round, long and short", () => {
    expect(entryBracketRefusal(ok)).toBeNull();
    expect(
      entryBracketRefusal({
        ...ok,
        direction: SHORT,
        stop: 110n,
        takeProfit: 80n,
      })
    ).toBeNull();
  });
  it("refuses a stop or target on the wrong side, and a missing stop", () => {
    expect(entryBracketRefusal({ ...ok, stop: 100n })).toMatch(/below/);
    expect(entryBracketRefusal({ ...ok, takeProfit: 95n })).toMatch(/above/);
    expect(entryBracketRefusal({ ...ok, stop: 0n })).toMatch(/mandatory/);
    expect(entryBracketRefusal({ ...ok, sizeBase: 0n })).toMatch(/size/);
  });
});

// --- builders: the account order the IDL declares --------------------------------------------

const signer = {
  address: "Trader1111111111111111111111111111111111111" as Address,
} as TransactionSigner;
const MANDATE = "Bf7aVemJQwTq5trPgqo6t7vjmHy4M3FcxjjHSp1BCyrc" as Address;
const PRICE = "SysvarC1ock11111111111111111111111111111111" as Address;

describe("builders follow the IDL's account order", () => {
  /**
   * The one that matters most: `old_stop` and `new_stop` are both trigger orders on the same
   * position. Swapped, the program would refuse — or worse, a valid pair would cancel the wrong
   * order. IDL order: …, position (7), old_stop (8), new_stop (9), price_update (10), …
   */
  it("funded_move_stop puts the old stop before the new one", async () => {
    const ix = await fundedMoveStopIx({
      signer,
      mandate: MANDATE,
      marketIndex: 5,
      nonce: 2,
      oldOrderId: 2,
      newOrderId: 3,
      newStop: 123n,
      legs: { priceUpdate: PRICE },
    });
    const a = await solfxAccounts(MANDATE, 5, 2);
    const [oldStop] = await findTriggerOrderPda({
      position: a.position,
      orderId: 2,
    });
    const [newStop] = await findTriggerOrderPda({
      position: a.position,
      orderId: 3,
    });
    const keys = (ix.accounts ?? []).map((x) => x.address);
    expect(keys[7]).toBe(a.position);
    expect(keys[8]).toBe(oldStop);
    expect(keys[9]).toBe(newStop);
    expect(keys[10]).toBe(PRICE);
    expect(keys).toHaveLength(15);
  });

  it("funded_reduce_position carries the trader's own profile", async () => {
    const ix = await fundedReduceIx({
      signer,
      mandate: MANDATE,
      marketIndex: 5,
      nonce: 2,
      sizeDelta: 1n,
      priceLimit: 1n,
      legs: { priceUpdate: PRICE },
    });
    const keys = (ix.accounts ?? []).map((x) => x.address);
    expect(keys[8]).toBe(await noxPdas.findProfile(signer.address));
    expect(keys).toHaveLength(20);
  });

  it("eval_place_entry_order addresses the order at its own seeds", async () => {
    const evaluation = await noxPdas.findEvaluation(signer.address, 0);
    const ix = await evalPlaceEntryOrderIx({
      signer,
      evaluation,
      orderId: 4,
      market: PRICE,
      marketIndex: 5,
      nonce: 1,
      direction: LONG,
      kind: LIMIT,
      triggerPrice: 100n,
      sizeBase: 1n,
      stopLossPrice: 90n,
      takeProfitPrice: 0n,
      expiresAt: 0n,
    });
    const keys = (ix.accounts ?? []).map((x) => x.address);
    expect(keys[3]).toBe(await noxPdas.findEvalEntryOrder(evaluation, 4));
  });
});

describe("evalObserveIx — each group in its market's own shape", () => {
  it("sends a direct market's three and a synthetic, converted market's five, in order", () => {
    const a = (n: number) => `Addr${String(n).padStart(39, "1")}` as Address;
    const ix = evalObserveIx(signer, a(1), [
      { position: a(2), market: a(3), priceUpdate: a(4) },
      {
        position: a(5),
        market: a(6),
        priceUpdate: a(7),
        secondaryPriceUpdate: a(8),
        quoteConversionPriceUpdate: a(9),
      },
    ]);
    const tail = (ix.accounts ?? []).slice(2).map((x) => x.address);
    expect(tail).toEqual([a(2), a(3), a(4), a(5), a(6), a(7), a(8), a(9)]);
  });
});
