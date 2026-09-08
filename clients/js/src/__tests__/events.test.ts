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
  it.each(SOLFX_EVENTS.map((e) => [e.name, e.discriminator] as const))("%s", (name, disc) => {
    const derived = new Uint8Array(
      createHash("sha256").update(`event:${name}`).digest(),
    ).slice(0, 8);
    expect(Array.from(disc)).toEqual(Array.from(derived));
  });
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
    expect(parseEvents(logs).map((e) => e.name)).toEqual(["MarketStatusChanged"]);
  });

  it("returns nothing for logs with no events", () => {
    expect(parseEvents([`Program ${PROGRAM} invoke [1]`, `Program ${PROGRAM} success`])).toEqual([]);
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
