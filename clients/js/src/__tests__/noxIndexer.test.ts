import {
  getAddressEncoder,
  getI64Encoder,
  getStructEncoder,
  getU16Encoder,
  getU32Encoder,
  getU64Encoder,
  getU8Encoder,
  type Address,
  type Encoder,
} from "@solana/kit";
import { describe, expect, it } from "vitest";

import {
  EVALUATION_FAILED_DISCRIMINATOR,
  EVALUATION_STARTED_DISCRIMINATOR,
  STAGE_PASSED_DISCRIMINATOR,
  TRADE_RECORDED_DISCRIMINATOR,
  TRADER_PROFILE_CREATED_DISCRIMINATOR,
  TRADER_VERIFIED_DISCRIMINATOR,
} from "../nox/events.generated.js";
import { NOXFUNDS_PROGRAM_ADDRESS } from "../generated-noxfunds/programs/noxfunds.js";
import {
  isRateLimit,
  isTruncated,
  listTraders,
  NoxStore,
  syncOnce,
  traderEntity,
  type IndexedTx,
  type RpcCall,
} from "../nox/indexer.js";

const PROFILE = "5JomF1SJUawqdLV6tWMFwoLuLKjbM6h67jGB3FrVf83w" as Address;
const TRADER = "2CdttmCi65rJb5UcWR9zCfwocYjcoYBpcWWVpJYhu4oF" as Address;
const EVAL = "SysvarRent111111111111111111111111111111111" as Address;
const MANDATE = "SysvarC1ock11111111111111111111111111111111" as Address;
const a = getAddressEncoder();

function event(disc: Uint8Array, enc: Encoder<object>, value: object): string {
  const body = enc.encode(value);
  const bytes = new Uint8Array(8 + body.length);
  bytes.set(disc);
  bytes.set(body, 8);
  return `Program data: ${Buffer.from(bytes).toString("base64")}`;
}

const created = event(
  TRADER_PROFILE_CREATED_DISCRIMINATOR,
  getStructEncoder([
    ["profile", a],
    ["authority", a],
    ["ts", getI64Encoder()],
  ]) as Encoder<object>,
  { profile: PROFILE, authority: TRADER, ts: 1n },
);

const started = event(
  EVALUATION_STARTED_DISCRIMINATOR,
  getStructEncoder([
    ["evaluation", a],
    ["trader", a],
    ["seq", getU8Encoder()],
    ["accountSize", getU64Encoder()],
    ["stake", getU64Encoder()],
    ["ts", getI64Encoder()],
  ]) as Encoder<object>,
  { evaluation: EVAL, trader: TRADER, seq: 0, accountSize: 10_000_000_000n, stake: 50_000_000n, ts: 2n },
);

const passed = (stage: number) =>
  event(
    STAGE_PASSED_DISCRIMINATOR,
    getStructEncoder([
      ["evaluation", a],
      ["trader", a],
      ["stage", getU8Encoder()],
      ["equity", getU64Encoder()],
      ["trades", getU32Encoder()],
      ["tradingDays", getU16Encoder()],
      ["ts", getI64Encoder()],
    ]) as Encoder<object>,
    { evaluation: EVAL, trader: TRADER, stage, equity: 0n, trades: 10, tradingDays: 5, ts: 3n },
  );

const failed = event(
  EVALUATION_FAILED_DISCRIMINATOR,
  getStructEncoder([
    ["evaluation", a],
    ["trader", a],
    ["stage", getU8Encoder()],
    ["rule", getU8Encoder()],
    ["equity", getU64Encoder()],
    ["ts", getI64Encoder()],
  ]) as Encoder<object>,
  { evaluation: EVAL, trader: TRADER, stage: 1, rule: 2, equity: 0n, ts: 4n },
);

const verified = event(
  TRADER_VERIFIED_DISCRIMINATOR,
  getStructEncoder([
    ["trader", a],
    ["evaluation", a],
    ["accountSize", getU64Encoder()],
    ["evaluationsPassed", getU32Encoder()],
    ["ts", getI64Encoder()],
  ]) as Encoder<object>,
  { trader: TRADER, evaluation: EVAL, accountSize: 10_000_000_000n, evaluationsPassed: 1, ts: 5n },
);

const win = event(
  TRADE_RECORDED_DISCRIMINATOR,
  getStructEncoder([
    ["profile", a],
    ["mandate", a],
    ["trader", a],
    ["marketIndex", getU16Encoder()],
    ["nonce", getU8Encoder()],
    ["realizedPnl", getI64Encoder()],
    ["holdSlots", getU64Encoder()],
    ["trades", getU32Encoder()],
    ["wins", getU32Encoder()],
    ["losses", getU32Encoder()],
    ["grossProfit", getU64Encoder()],
    ["grossLoss", getU64Encoder()],
    ["ts", getI64Encoder()],
  ]) as Encoder<object>,
  {
    profile: PROFILE,
    mandate: MANDATE,
    trader: TRADER,
    marketIndex: 0,
    nonce: 0,
    realizedPnl: 25n,
    holdSlots: 9n,
    trades: 1,
    wins: 1,
    losses: 0,
    grossProfit: 25n,
    grossLoss: 0n,
    ts: 6n,
  },
);

let n = 0;
/** A program transaction loading the profile, carrying `lines` inside the program's invoke. */
function tx(lines: string[], opts: { failed?: boolean; extraLogs?: string[] } = {}): IndexedTx {
  n++;
  return {
    signature: `sig${String(n).padStart(4, "0")}`,
    slot: BigInt(n),
    blockTime: BigInt(1_700_000_000 + n),
    failed: opts.failed ?? false,
    accounts: [TRADER, PROFILE, NOXFUNDS_PROGRAM_ADDRESS],
    logs: [
      `Program ${NOXFUNDS_PROGRAM_ADDRESS} invoke [1]`,
      ...lines,
      `Program ${NOXFUNDS_PROGRAM_ADDRESS} success`,
      ...(opts.extraLogs ?? []),
    ],
  };
}

/** Oldest first in, newest first to the store — as the RPC lists them. */
const newestFirst = (txs: IndexedTx[]) => [...txs].reverse();

describe("the store", () => {
  it("is idempotent: a transaction stored twice is stored once", () => {
    const s = new NoxStore();
    const t = tx([created]);
    expect(s.insertNewestFirst([t])).toBe(1);
    expect(s.insertNewestFirst([t])).toBe(0);
    expect(s.count()).toEqual({ txs: 1, events: 1, truncated: 0 });
  });

  it("keeps the RPC's order across separate syncs", () => {
    const s = new NoxStore();
    const [t1, t2, t3, t4] = [tx([created]), tx([started]), tx([passed(1)]), tx([passed(2)])];
    s.insertNewestFirst(newestFirst([t1!, t2!]));
    s.insertNewestFirst(newestFirst([t3!, t4!]));
    expect(s.txsTouching(PROFILE).map((t) => t.signature)).toEqual(
      [t1, t2, t3, t4].map((t) => t!.signature),
    );
  });

  it("stores no event from a failed transaction", () => {
    const s = new NoxStore();
    s.insertNewestFirst([tx([created], { failed: true })]);
    expect(s.count().events).toBe(0);
  });

  it("flags a transaction whose logs were truncated", () => {
    expect(isTruncated(["Program data: x", "Log truncated"])).toBe(true);
    expect(isTruncated(["Program data: x"])).toBe(false);
    const s = new NoxStore();
    s.insertNewestFirst([tx([created], { extraLogs: ["Log truncated"] })]);
    expect(s.count().truncated).toBe(1);
  });
});

describe("the trader entity", () => {
  const history = () => [
    tx([created]),
    tx([started]),
    tx([win]),
    tx([passed(1)]),
    tx([passed(2), verified]),
  ];

  it("folds the record, the evaluation and the verified mark", () => {
    const s = new NoxStore();
    s.insertNewestFirst(newestFirst(history()));
    expect(listTraders(s)).toEqual([{ trader: TRADER, profile: PROFILE }]);
    const e = traderEntity(s, TRADER, PROFILE);
    expect(e.verified).toBe(true);
    expect(e.record.trades).toBe(1);
    expect(e.record.grossProfit).toBe(25n);
    expect(e.record.evaluationsPassed).toBe(1);
    expect(e.firstDisagreement).toBeNull();
    expect(e.evaluations).toEqual([
      expect.objectContaining({ evaluation: EVAL, state: "passed", stagesPassed: [1, 2] }),
    ]);
    expect(e.truncatedTxs).toBe(0);
  });

  it("is not verified by a failure, and says which rule ended it", () => {
    const s = new NoxStore();
    s.insertNewestFirst(newestFirst([tx([created]), tx([started]), tx([failed])]));
    const e = traderEntity(s, TRADER, PROFILE);
    expect(e.verified).toBe(false);
    expect(e.evaluations[0]).toEqual(
      expect.objectContaining({ state: "failed", failedRule: 2 }),
    );
  });

  /** **The replay property.** One sync or two, the same history gives the same entity. */
  it("is the same however the history arrived", () => {
    const h = history();
    const once = new NoxStore();
    once.insertNewestFirst(newestFirst(h));
    const twice = new NoxStore();
    twice.insertNewestFirst(newestFirst(h.slice(0, 2)));
    twice.insertNewestFirst(newestFirst(h.slice(2)));
    twice.insertNewestFirst(newestFirst(h)); // a replay of everything changes nothing
    expect(traderEntity(twice, TRADER, PROFILE)).toEqual(traderEntity(once, TRADER, PROFILE));
  });

  it("counts the truncated transactions a record rests on", () => {
    const s = new NoxStore();
    s.insertNewestFirst(newestFirst([tx([created]), tx([win], { extraLogs: ["Log truncated"] })]));
    expect(traderEntity(s, TRADER, PROFILE).truncatedTxs).toBe(1);
  });
});

describe("syncing", () => {
  it("knows a rate limit in either of the forms an RPC sends it", () => {
    expect(isRateLimit("Too many requests for a specific RPC call")).toBe(true);
    expect(isRateLimit("Connection rate limits exceeded")).toBe(true);
    expect(isRateLimit("Transaction version (0) is not supported")).toBe(false);
  });

  /** A cluster of `txs`, listed newest first, paged as the RPC pages. */
  function fakeRpc(txs: IndexedTx[], pageSize = 1000): { rpc: RpcCall; calls: unknown[][] } {
    const calls: unknown[][] = [];
    const listed = newestFirst(txs);
    const rpc: RpcCall = async (method, params) => {
      calls.push([method, params]);
      if (method === "getSignaturesForAddress") {
        const opts = params[1] as { before?: string; until?: string; limit: number };
        let from = 0;
        if (opts.before) from = listed.findIndex((t) => t.signature === opts.before) + 1;
        let to = listed.length;
        if (opts.until) to = listed.findIndex((t) => t.signature === opts.until);
        return listed
          .slice(from, to)
          .slice(0, Math.min(pageSize, opts.limit))
          .map((t) => ({ signature: t.signature, slot: Number(t.slot), err: t.failed ? {} : null }));
      }
      const t = txs.find((x) => x.signature === params[0]);
      return {
        slot: Number(t!.slot),
        blockTime: Number(t!.blockTime),
        meta: { err: t!.failed ? {} : null, logMessages: t!.logs },
        transaction: { message: { accountKeys: t!.accounts } },
      };
    };
    return { rpc, calls };
  }

  /** A sync that dies part-way keeps what it fetched, and the next one resumes from it. */
  it("resumes a failed backfill without fetching anything twice", async () => {
    const h = [tx([created]), tx([started]), tx([win])];
    const cluster = fakeRpc(h);
    let fetches = 0;
    let failOn = 2;
    const flaky: RpcCall = async (method, params) => {
      if (method === "getTransaction" && ++fetches === failOn) throw new Error("boom");
      return cluster.rpc(method, params);
    };
    const s = new NoxStore();
    await expect(syncOnce(s, flaky, undefined, 1)).rejects.toThrow("boom");
    expect(s.count().txs).toBe(0);
    failOn = -1;
    const before = fetches;
    expect(await syncOnce(s, flaky, undefined, 1)).toBe(3);
    expect(fetches - before).toBe(2); // the one cached before the failure was not refetched
  });

  it("backfills the whole history, then only what is new", async () => {
    const h = [tx([created]), tx([started]), tx([win])];
    const s = new NoxStore();
    const cluster = fakeRpc(h);
    expect(await syncOnce(s, cluster.rpc)).toBe(3);
    expect(await syncOnce(s, cluster.rpc)).toBe(0);

    const later = [...h, tx([passed(1)])];
    const grown = fakeRpc(later);
    expect(await syncOnce(s, grown.rpc)).toBe(1);
    // The forward sync asked only for what came after the newest it held.
    const ask = grown.calls.find((c) => c[0] === "getSignaturesForAddress")!;
    expect((ask[1] as unknown[])[1]).toMatchObject({ until: h[2]!.signature });
    expect(s.txsTouching(PROFILE).map((t) => t.signature)).toEqual(
      later.map((t) => t.signature),
    );
  });
});
