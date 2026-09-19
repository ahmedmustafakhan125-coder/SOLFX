import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";

import { SOLFX_EVENTS } from "../events/generated.js";
import { parseEvents } from "../events/parse.js";

const PROGRAM = "2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi";

// Real logs, captured from the devnet transaction that halted market 1:
// EsDFJa1yVn67FbiPnJK9ZJTNmifeAApipyJoCPDNX5Nh7oT9oAfK9UsgcyyFt8LRgGzDcLpG1gqfkPqSmbUX5Uu
const REAL_LOGS = [
  "Program ComputeBudget111111111111111111111111111111 invoke [1]",
  "Program ComputeBudget111111111111111111111111111111 success",
  `Program ${PROGRAM} invoke [1]`,
  "Program log: Instruction: SetMarketStatus",
  "Program data: NUig0Q/eLp0BAAEDAPRCk2oAAAAA",
  `Program ${PROGRAM} consumed 9698 of 59850 compute units`,
  `Program ${PROGRAM} success`,
];

describe("every event discriminator is Anchor's derivation, not a transcription", () => {
  it.each(SOLFX_EVENTS.map((e) => [e.name, e.discriminator] as const))(
    "%s",
    (name, disc) => {
      const derived = new Uint8Array(
        createHash("sha256").update(`event:${name}`).digest(),
      ).slice(0, 8);
      expect(Array.from(disc)).toEqual(Array.from(derived));
    },
  );
});

describe("decoding a real on-chain event", () => {
  it("recovers exactly what we did to market 1", () => {
    const events = parseEvents(REAL_LOGS);
    expect(events).toHaveLength(1);
    const [e] = events;
    expect(e!.name).toBe("MarketStatusChanged");
    // Hand-decoded from the base64 before this parser existed, so the parser is
    // checked against arithmetic rather than against itself.
    expect(e!.data).toEqual({
      marketIndex: 1,
      oldStatus: 1, // Active
      newStatus: 3, // Halted
      actor: 0, // admin
      ts: 1_788_035_828n,
    });
  });
});

describe("the invoke stack is respected", () => {
  it("ignores an identical payload emitted by a different program", () => {
    const other = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
    const logs = [
      `Program ${other} invoke [1]`,
      "Program data: NUig0Q/eLp0BAAEDAPRCk2oAAAAA",
      `Program ${other} success`,
    ];
    expect(parseEvents(logs)).toHaveLength(0);
  });

  it("still finds our event when another program ran first", () => {
    const logs = [
      "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [1]",
      "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA success",
      ...REAL_LOGS,
    ];
    expect(parseEvents(logs).map((e) => e.name)).toEqual([
      "MarketStatusChanged",
    ]);
  });

  it("returns nothing for logs with no events", () => {
    expect(
      parseEvents([
        `Program ${PROGRAM} invoke [1]`,
        `Program ${PROGRAM} success`,
      ]),
    ).toEqual([]);
  });

  it("skips an unknown discriminator instead of throwing", () => {
    const logs = [
      `Program ${PROGRAM} invoke [1]`,
      `Program data: ${Buffer.from(new Uint8Array(16).fill(7)).toString("base64")}`,
      `Program ${PROGRAM} success`,
    ];
    expect(parseEvents(logs)).toEqual([]);
  });
});

describe("coverage", () => {
  it("decodes all 35 events the IDL declares", () => {
    expect(SOLFX_EVENTS).toHaveLength(35);
    expect(SOLFX_EVENTS.map((e) => e.name)).toContain("MarketOracleChanged");
  });
});

describe("decoding without Node's Buffer", () => {
  // The terminal parses its own trade history in the browser, where `Buffer` does not exist
  // and `atob` is the decoder. Every other test in this file runs in Node and so only ever
  // exercises the `Buffer` branch — this one takes the branch the browser actually takes,
  // against the same real logs, and demands an identical result.
  it("takes the atob path and decodes identically", async () => {
    const parsed = parseEvents(REAL_LOGS);

    const buffer = globalThis.Buffer;
    // @ts-expect-error — deleting a global is the point of the test.
    delete globalThis.Buffer;
    try {
      expect(typeof Buffer).toBe("undefined");
      expect(parseEvents(REAL_LOGS)).toEqual(parsed);
    } finally {
      globalThis.Buffer = buffer;
    }
  });
});

// --- field offsets --------------------------------------------------------------------------

/**
 * Byte-level layout tests for the two events the trigger-binding upgrade changed.
 *
 * # Why these exist
 *
 * `entry_price` was inserted into the **middle** of `PositionDecreased` and
 * `TriggerOrderExecuted` — after `oracle_price`, before `exec_price`/`size_base`. Every field
 * behind it moved eight bytes. Nothing in this suite would have noticed if the decoder
 * disagreed with the program about where they now sit: the discriminator tests derive from the
 * event *name* and are blind to the layout, and the only hand-built payload here is a
 * `MarketStatusChanged` that neither event touches. Two layouts changed with zero coverage.
 *
 * The decoders are generated from the IDL, and the IDL is generated from the program — but an
 * IDL can be stale, which is exactly the failure `referral.test.ts` was written to catch on
 * the instruction side. So the bytes below are laid out from the **Rust struct order in
 * `programs/solfx-core/src/events.rs`**, not from the decoder, and the decoder is asked to
 * agree. A stale IDL regenerated against an older program fails here rather than on chain.
 *
 * Every field gets a distinct value, so a decoder reading the right type at the wrong offset
 * cannot pass by coincidence.
 */
class Payload {
  private readonly parts: Uint8Array[] = [];

  disc(d: Uint8Array): this {
    this.parts.push(d);
    return this;
  }

  /** A 32-byte address, filled with one repeated byte so each is visually distinct. */
  addr(fill: number): this {
    this.parts.push(new Uint8Array(32).fill(fill));
    return this;
  }

  u8(v: number): this {
    this.parts.push(Uint8Array.of(v));
    return this;
  }

  bool(v: boolean): this {
    return this.u8(v ? 1 : 0);
  }

  u16(v: number): this {
    const b = new Uint8Array(2);
    new DataView(b.buffer).setUint16(0, v, true);
    return this.push(b);
  }

  /** Little-endian 64-bit, signed or unsigned — Borsh writes both the same way. */
  i64(v: bigint): this {
    const b = new Uint8Array(8);
    new DataView(b.buffer).setBigInt64(0, v, true);
    return this.push(b);
  }

  private push(b: Uint8Array): this {
    this.parts.push(b);
    return this;
  }

  logs(): string[] {
    let total = 0;
    for (const p of this.parts) total += p.length;
    const out = new Uint8Array(total);
    let at = 0;
    for (const p of this.parts) {
      out.set(p, at);
      at += p.length;
    }
    return [
      `Program ${PROGRAM} invoke [1]`,
      `Program data: ${Buffer.from(out).toString("base64")}`,
      `Program ${PROGRAM} success`,
    ];
  }
}

function discriminatorOf(name: string): Uint8Array {
  const e = SOLFX_EVENTS.find((x) => x.name === name);
  if (!e) throw new Error(`no such event: ${name}`);
  return e.discriminator;
}

/** 32 bytes of `fill`, as the base58 address the decoder will produce. */
function addressOf(fill: number): string {
  const e = SOLFX_EVENTS.find((x) => x.name === "PositionDecreased");
  if (!e) throw new Error("PositionDecreased missing");
  const probe = new Payload()
    .disc(e.discriminator)
    .addr(fill)
    .addr(fill)
    .u16(0)
    .i64(0n)
    .i64(0n)
    .i64(0n)
    .i64(0n)
    .i64(0n)
    .i64(0n)
    .i64(0n)
    .i64(0n)
    .bool(false)
    .i64(0n);
  const [ev] = parseEvents(probe.logs());
  return (ev!.data as { position: string }).position;
}

describe("the events the upgrade changed decode at the right offsets", () => {
  // programs/solfx-core/src/events.rs — PositionDecreased, in declaration order.
  it("PositionDecreased puts entry_price between oracle_price and exec_price", () => {
    const logs = new Payload()
      .disc(discriminatorOf("PositionDecreased"))
      .addr(0x11) // position
      .addr(0x22) // user_account
      .u16(7) // market_index
      .i64(1_000n) // size_closed
      .i64(2_000n) // remaining_size
      .i64(3_000n) // oracle_price
      .i64(4_000n) // entry_price      <- inserted here
      .i64(5_000n) // exec_price
      .i64(-6_000n) // realized_pnl, signed on purpose
      .i64(7_000n) // fee
      .i64(8_000n) // collateral_returned
      .bool(true) // fully_closed
      .i64(9_000n) // ts
      .logs();

    const events = parseEvents(logs);
    expect(events).toHaveLength(1);
    expect(events[0]!.name).toBe("PositionDecreased");
    expect(events[0]!.data).toEqual({
      position: addressOf(0x11),
      userAccount: addressOf(0x22),
      marketIndex: 7,
      sizeClosed: 1_000n,
      remainingSize: 2_000n,
      oraclePrice: 3_000n,
      entryPrice: 4_000n,
      execPrice: 5_000n,
      realizedPnl: -6_000n,
      fee: 7_000n,
      collateralReturned: 8_000n,
      fullyClosed: true,
      ts: 9_000n,
    });
  });

  // programs/solfx-core/src/events.rs — TriggerOrderExecuted, in declaration order.
  it("TriggerOrderExecuted puts entry_price between oracle_price and size_base", () => {
    const logs = new Payload()
      .disc(discriminatorOf("TriggerOrderExecuted"))
      .addr(0x33) // order
      .addr(0x44) // position
      .addr(0x55) // keeper
      .u16(9) // market_index
      .u8(2) // order_id
      .u8(1) // kind — 1 is StopLoss
      .i64(1_100n) // trigger_price
      .i64(1_200n) // oracle_price
      .i64(1_300n) // entry_price      <- inserted here
      .i64(1_400n) // size_base
      .i64(1_500n) // keeper_tip_lamports
      .i64(1_600n) // ts
      .logs();

    const events = parseEvents(logs);
    expect(events).toHaveLength(1);
    expect(events[0]!.name).toBe("TriggerOrderExecuted");
    expect(events[0]!.data).toEqual({
      order: addressOf(0x33),
      position: addressOf(0x44),
      keeper: addressOf(0x55),
      marketIndex: 9,
      orderId: 2,
      kind: 1,
      triggerPrice: 1_100n,
      oraclePrice: 1_200n,
      entryPrice: 1_300n,
      sizeBase: 1_400n,
      keeperTipLamports: 1_500n,
      ts: 1_600n,
    });
  });

  /**
   * The distance a take-profit travelled, computed the way NOXFUNDS will compute it.
   *
   * This is the whole reason `entry_price` was added: without it the move is not recoverable
   * from the event, because a position that has been increased no longer entered where
   * `PositionOpened` said it did.
   */
  it("makes a take-profit's distance derivable from the one event", () => {
    const ENTRY = 2_509_610_000_000n; // 2509.61 at PRICE_PRECISION
    const TRIGGER = 2_520_000_000_000n;
    const ORACLE = 2_521_000_000_000n;

    const logs = new Payload()
      .disc(discriminatorOf("TriggerOrderExecuted"))
      .addr(0x33)
      .addr(0x44)
      .addr(0x55)
      .u16(9)
      .u8(0)
      .u8(0) // kind 0 — TakeProfit
      .i64(TRIGGER)
      .i64(ORACLE)
      .i64(ENTRY)
      .i64(3_993_369_904n)
      .i64(0n)
      .i64(1_789_330_497n)
      .logs();

    const d = parseEvents(logs)[0]!.data as {
      kind: number;
      oraclePrice: bigint;
      entryPrice: bigint;
    };
    expect(d.kind).toBe(0);
    // A long take-profit: the oracle finished above the entry, and by how much is arithmetic
    // on this event alone — no join against PositionDecreased, no replay of increases.
    expect(d.oraclePrice - d.entryPrice).toBe(11_390_000_000n);
  });
});

describe("events whose shape changed in an upgrade", () => {
  // A `PositionDecreased` exactly as the program emitted it before Phase 10 inserted
  // `entry_price`: the same discriminator, one i64 shorter. Built field by field rather than
  // pasted, so what is being asserted is the layout and not a blob nobody can read.
  const legacyPositionDecreased = () => {
    const parts: number[] = [];
    const push = (bytes: readonly number[]) => parts.push(...bytes);
    const i64 = (v: bigint) => {
      const b = new Uint8Array(8);
      new DataView(b.buffer).setBigInt64(0, v, true);
      return Array.from(b);
    };
    const u64 = (v: bigint) => {
      const b = new Uint8Array(8);
      new DataView(b.buffer).setBigUint64(0, v, true);
      return Array.from(b);
    };
    push(
      Array.from(
        SOLFX_EVENTS.find((e) => e.name === "PositionDecreased")!.discriminator,
      ),
    );
    push(new Array(32).fill(1)); // position
    push(new Array(32).fill(2)); // userAccount
    push([3, 0]); // marketIndex
    push(u64(1_000n)); // sizeClosed
    push(u64(0n)); // remainingSize
    push(i64(4_354_000_000_000n)); // oraclePrice
    // no entryPrice — this is the whole point
    push(i64(4_351_000_000_000n)); // execPrice
    push(i64(-2_100_000n)); // realizedPnl
    push(u64(100_000n)); // fee
    push(u64(197_900_000n)); // collateralReturned
    push([1]); // fullyClosed
    push(i64(1_789_000_000n)); // ts
    return Buffer.from(parts).toString("base64");
  };

  it("still reads a close from before the upgrade, with no entry price to report", () => {
    const events = parseEvents([
      `Program ${PROGRAM} invoke [1]`,
      `Program data: ${legacyPositionDecreased()}`,
      `Program ${PROGRAM} success`,
    ]);
    expect(events).toHaveLength(1);
    const d = events[0]!.data as Record<string, unknown>;
    expect(events[0]!.name).toBe("PositionDecreased");
    expect(d.realizedPnl).toBe(-2_100_000n);
    expect(d.collateralReturned).toBe(197_900_000n);
    expect(d.fullyClosed).toBe(true);
    expect(d.entryPrice).toBe(0n);
  });

  // The failure this replaces: the whole history panel showed an error because one event in
  // one transaction could not be decoded.
  it("skips a payload it cannot read rather than losing the rest of the transaction", () => {
    const truncated = Buffer.from([
      ...Array.from(
        SOLFX_EVENTS.find((e) => e.name === "PositionDecreased")!.discriminator,
      ),
      1,
      2,
      3,
    ]).toString("base64");
    const events = parseEvents([
      `Program ${PROGRAM} invoke [1]`,
      `Program data: ${truncated}`,
      "Program data: NUig0Q/eLp0BAAEDAPRCk2oAAAAA",
      `Program ${PROGRAM} success`,
    ]);
    expect(events.map((e) => e.name)).toEqual(["MarketStatusChanged"]);
  });
});
