/**
 * The NOXFUNDS indexer: every transaction the program has run, kept, decoded and folded into a
 * record per trader — the "subgraph" an investor reads instead of scanning the chain.
 *
 * # Why not The Graph
 *
 * The Solana MCP has no documentation of The Graph or Substreams serving a Solana devnet, and
 * nothing here may rest on a guess. What it does document is the standard self-hosted pattern:
 * backfill with `getSignaturesForAddress` + `getTransaction`, then keep up by polling (Helius,
 * *How to Index Solana Data*). That is what this is, with SQLite for storage and nothing else.
 *
 * # Why the record is the verifier's, not a second fold
 *
 * A trader's figures come from `rederiveProfile` — the same function `/nox/verify` uses to prove
 * a profile against its events — over exactly the transactions that touched the profile. So the
 * indexer cannot disagree with the verifier by construction, and every figure it serves can be
 * compared with the account by `compareProfile`.
 *
 * # What it does about truncated logs
 *
 * Events are `emit!`, which writes to the transaction's logs, and logs can be truncated (MCP:
 * Anchor Events, "logs may be truncated"). A transaction whose logs say so is stored and
 * **flagged**; every trader it touches reports how many such transactions their record rests on,
 * so a missing event is visible rather than a silently low count.
 */
import { DatabaseSync } from "node:sqlite";

import { parseNoxEvents, type NoxEvent } from "./events.js";
import { findProfile } from "./pdas.js";
import {
  compareProfile,
  rederiveProfile,
  type Comparison,
  type Derived,
  type ProfileFigures,
  type ProfileTx,
} from "./verify.js";
import { NOXFUNDS_PROGRAM_ADDRESS } from "../generated-noxfunds/programs/noxfunds.js";
import type { Address } from "@solana/kit";

/** One program transaction, as the indexer stores it. */
export type IndexedTx = {
  readonly signature: string;
  readonly slot: bigint;
  readonly blockTime: bigint | null;
  readonly logs: readonly string[];
  /** Every account the transaction loaded, static and from lookup tables. */
  readonly accounts: readonly string[];
  readonly failed: boolean;
};

/** A transaction whose logs were cut off by the runtime: some of its events may be missing. */
export function isTruncated(logs: readonly string[]): boolean {
  return logs.some((l) => l.includes("Log truncated"));
}

/** JSON with bigints as strings — event fields are u64/i64 and must not lose precision. */
const toJson = (v: unknown) =>
  JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x));

// --- the store ---------------------------------------------------------------------------------

export class NoxStore {
  readonly db: DatabaseSync;

  constructor(path = ":memory:") {
    this.db = new DatabaseSync(path);
    this.db.exec(`
      PRAGMA journal_mode = WAL;
      CREATE TABLE IF NOT EXISTS txs (
        signature  TEXT PRIMARY KEY,
        ord        INTEGER NOT NULL UNIQUE,
        slot       INTEGER NOT NULL,
        block_time INTEGER,
        failed     INTEGER NOT NULL,
        truncated  INTEGER NOT NULL,
        logs       TEXT NOT NULL,
        accounts   TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS events (
        signature TEXT NOT NULL,
        idx       INTEGER NOT NULL,
        ord       INTEGER NOT NULL,
        name      TEXT NOT NULL,
        data      TEXT NOT NULL,
        PRIMARY KEY (signature, idx)
      );
      CREATE INDEX IF NOT EXISTS events_name ON events (name, ord);
      CREATE TABLE IF NOT EXISTS tx_accounts (
        account   TEXT NOT NULL,
        signature TEXT NOT NULL,
        ord       INTEGER NOT NULL,
        PRIMARY KEY (account, signature)
      );
      CREATE INDEX IF NOT EXISTS tx_accounts_ord ON tx_accounts (account, ord);
      -- Transactions fetched but not yet placed in order. A sync that fails part-way keeps
      -- what it fetched, so the next one resumes instead of starting the history again.
      CREATE TABLE IF NOT EXISTS fetched (
        signature TEXT PRIMARY KEY,
        tx        TEXT NOT NULL
      );
    `);
  }

  cachedTx(signature: string): IndexedTx | undefined {
    const row = this.db
      .prepare("SELECT tx FROM fetched WHERE signature = ?")
      .get(signature) as { tx: string } | undefined;
    if (!row) return undefined;
    const t = JSON.parse(row.tx) as Omit<IndexedTx, "slot" | "blockTime"> & {
      slot: string;
      blockTime: string | null;
    };
    return {
      ...t,
      slot: BigInt(t.slot),
      blockTime: t.blockTime === null ? null : BigInt(t.blockTime),
    };
  }

  cacheTx(t: IndexedTx): void {
    this.db
      .prepare("INSERT OR REPLACE INTO fetched (signature, tx) VALUES (?, ?)")
      .run(t.signature, toJson(t));
  }

  /** The newest transaction indexed, which a forward sync stops at. */
  newestSignature(): string | undefined {
    const row = this.db
      .prepare("SELECT signature FROM txs ORDER BY ord DESC LIMIT 1")
      .get() as { signature: string } | undefined;
    return row?.signature;
  }

  maxOrd(): number {
    const row = this.db.prepare("SELECT MAX(ord) AS m FROM txs").get() as {
      m: number | null;
    };
    return row.m ?? 0;
  }

  count(): { txs: number; events: number; truncated: number } {
    const one = (sql: string) =>
      Number((this.db.prepare(sql).get() as { n: number }).n);
    return {
      txs: one("SELECT COUNT(*) AS n FROM txs"),
      events: one("SELECT COUNT(*) AS n FROM events"),
      truncated: one("SELECT COUNT(*) AS n FROM txs WHERE truncated = 1"),
    };
  }

  /**
   * Store transactions given **newest first**, as the RPC lists them, all newer than anything
   * stored. They are numbered so that ascending `ord` is oldest first, preserving the RPC's
   * order inside a slot. A signature already stored is skipped, so a replay changes nothing.
   */
  insertNewestFirst(txs: readonly IndexedTx[]): number {
    const fresh = txs.filter(
      (t) =>
        !this.db
          .prepare("SELECT 1 FROM txs WHERE signature = ?")
          .get(t.signature)
    );
    const base = this.maxOrd();
    const insTx = this.db.prepare(
      "INSERT INTO txs (signature, ord, slot, block_time, failed, truncated, logs, accounts) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    );
    const insEv = this.db.prepare(
      "INSERT INTO events (signature, idx, ord, name, data) VALUES (?, ?, ?, ?, ?)"
    );
    const insAcc = this.db.prepare(
      "INSERT OR IGNORE INTO tx_accounts (account, signature, ord) VALUES (?, ?, ?)"
    );
    this.db.exec("BEGIN");
    try {
      fresh.forEach((t, i) => {
        const ord = base + (fresh.length - i);
        insTx.run(
          t.signature,
          ord,
          Number(t.slot),
          t.blockTime === null ? null : Number(t.blockTime),
          t.failed ? 1 : 0,
          isTruncated(t.logs) ? 1 : 0,
          JSON.stringify(t.logs),
          JSON.stringify(t.accounts)
        );
        for (const a of t.accounts) insAcc.run(a, t.signature, ord);
        // A failed transaction's logs can carry events emitted before it failed; nothing it
        // emitted happened, so none of it is stored as an event.
        if (!t.failed) {
          parseNoxEvents(t.logs).events.forEach((e, idx) =>
            insEv.run(t.signature, idx, ord, e.name, toJson(e.data))
          );
        }
      });
      // Placed now, so no longer pending.
      for (const t of txs) this.db.prepare("DELETE FROM fetched WHERE signature = ?").run(t.signature);
      this.db.exec("COMMIT");
    } catch (e) {
      this.db.exec("ROLLBACK");
      throw e;
    }
    return fresh.length;
  }

  /** Every transaction that loaded `account`, oldest first, in the shape the verifier folds. */
  txsTouching(account: string): (ProfileTx & { truncated: boolean })[] {
    const rows = this.db
      .prepare(
        `SELECT t.signature, t.slot, t.block_time, t.failed, t.truncated, t.logs
           FROM tx_accounts a JOIN txs t ON t.signature = a.signature
          WHERE a.account = ? ORDER BY t.ord ASC`
      )
      .all(account) as {
      signature: string;
      slot: number;
      block_time: number | null;
      failed: number;
      truncated: number;
      logs: string;
    }[];
    return rows.map((r) => ({
      signature: r.signature,
      slot: BigInt(r.slot),
      blockTime: r.block_time === null ? null : BigInt(r.block_time),
      logs: JSON.parse(r.logs) as string[],
      failed: r.failed === 1,
      truncated: r.truncated === 1,
    }));
  }

  /** Every stored event of the given names, oldest first, decoded back from JSON. */
  eventsNamed(names: readonly string[]): StoredEvent[] {
    if (names.length === 0) return [];
    const rows = this.db
      .prepare(
        `SELECT e.signature, e.name, e.data, t.block_time
           FROM events e JOIN txs t ON t.signature = e.signature
          WHERE e.name IN (${names.map(() => "?").join(",")})
          ORDER BY e.ord ASC, e.idx ASC`
      )
      .all(...names) as {
      signature: string;
      name: string;
      data: string;
      block_time: number | null;
    }[];
    return rows.map((r) => ({
      signature: r.signature,
      name: r.name,
      data: JSON.parse(r.data) as Record<string, string | number | boolean>,
      blockTime: r.block_time,
    }));
  }
}

export type StoredEvent = {
  readonly signature: string;
  readonly name: string;
  /** Integers arrive as decimal strings; addresses as base58. */
  readonly data: Record<string, string | number | boolean>;
  readonly blockTime: number | null;
};

// --- the trader entity -------------------------------------------------------------------------

export type EvaluationEntity = {
  evaluation: string;
  seq: number;
  accountSize: string;
  startedAt: number;
  /** "active", "passed" or "failed", as the events last left it. */
  state: "active" | "passed" | "failed";
  stage: number;
  stagesPassed: number[];
  failedRule: number | null;
};

export type MandateEntity = {
  mandate: string;
  investor: string;
  principal: string;
  fundedAt: number;
  state: "active" | "breached" | "settled";
  finalEquity: string | null;
};

export type TraderEntity = {
  trader: string;
  profile: string;
  /** The record as the events fold it — the verifier's own figures. */
  record: Derived;
  verified: boolean;
  evaluations: EvaluationEntity[];
  mandates: MandateEntity[];
  /** How many of the transactions this record rests on had truncated logs. */
  truncatedTxs: number;
  /** The first place the program's own running totals disagreed with the fold, if any. */
  firstDisagreement: ReturnType<typeof rederiveProfile>["firstDisagreement"];
  undecodable: number;
};

/** Every trader with a profile, from `TraderProfileCreated`, oldest first. */
export function listTraders(store: NoxStore): { trader: string; profile: string }[] {
  return store.eventsNamed(["TraderProfileCreated"]).map((e) => ({
    trader: String(e.data.authority),
    profile: String(e.data.profile),
  }));
}

/** Fold one trader. Pure over the store: the same transactions always give the same entity. */
export function traderEntity(
  store: NoxStore,
  trader: string,
  profile: string
): TraderEntity {
  const txs = store.txsTouching(profile);
  const r = rederiveProfile(profile as Address, trader as Address, txs);

  const evaluations = new Map<string, EvaluationEntity>();
  const mandates = new Map<string, MandateEntity>();
  const lifecycle = store.eventsNamed([
    "EvaluationStarted",
    "StagePassed",
    "EvaluationFailed",
    "MandateFunded",
    "MandateBreached",
    "MandateSettled",
  ]);
  for (const e of lifecycle) {
    const d = e.data;
    if (String(d.trader) !== trader) continue;
    switch (e.name) {
      case "EvaluationStarted":
        evaluations.set(String(d.evaluation), {
          evaluation: String(d.evaluation),
          seq: Number(d.seq),
          accountSize: String(d.accountSize),
          startedAt: Number(d.ts),
          state: "active",
          stage: 1,
          stagesPassed: [],
          failedRule: null,
        });
        break;
      case "StagePassed": {
        const ev = evaluations.get(String(d.evaluation));
        if (!ev) break;
        const stage = Number(d.stage);
        ev.stagesPassed.push(stage);
        if (stage >= 2) ev.state = "passed";
        else ev.stage = stage + 1;
        break;
      }
      case "EvaluationFailed": {
        const ev = evaluations.get(String(d.evaluation));
        if (!ev) break;
        ev.state = "failed";
        ev.failedRule = Number(d.rule);
        break;
      }
      case "MandateFunded":
        mandates.set(String(d.mandate), {
          mandate: String(d.mandate),
          investor: String(d.investor),
          principal: String(d.principal),
          fundedAt: Number(d.ts),
          state: "active",
          finalEquity: null,
        });
        break;
      case "MandateBreached": {
        const md = mandates.get(String(d.mandate));
        if (md) md.state = "breached";
        break;
      }
      case "MandateSettled": {
        const md = mandates.get(String(d.mandate));
        if (md) {
          md.state = "settled";
          md.finalEquity = String(d.finalEquity);
        }
        break;
      }
    }
  }

  return {
    trader,
    profile,
    record: r.derived,
    // Verified is the program's own statement: a `TraderVerified` event, or — for a pass
    // recorded before the profile counter existed — an evaluation whose events show Phase 2.
    verified:
      r.derived.evaluationsPassed > 0 ||
      [...evaluations.values()].some((e) => e.state === "passed"),
    evaluations: [...evaluations.values()],
    mandates: [...mandates.values()],
    truncatedTxs: txs.filter((t) => t.truncated).length,
    firstDisagreement: r.firstDisagreement,
    undecodable: r.undecodable,
  };
}

/** The indexed record beside the account's own figures. */
export function verifyEntity(
  entity: TraderEntity,
  onChain: ProfileFigures
): Comparison[] {
  return compareProfile(onChain, entity.record);
}

// --- syncing from an RPC ---------------------------------------------------------------------

/** One JSON-RPC call. Injected, so a test can stand in for the cluster. */
export type RpcCall = (method: string, params: unknown[]) => Promise<unknown>;

/** Whether an RPC error is a rate limit, which is worth waiting out rather than failing on. */
export function isRateLimit(message: string): boolean {
  return /too many requests|rate limit/i.test(message);
}

/**
 * A JSON-RPC client over `fetch`, retrying the rate limits every public endpoint imposes, and
 * spacing calls at least `minIntervalMs` apart — a backfill should not be a burst.
 */
export function httpRpc(url: string, attempts = 8, minIntervalMs = 0): RpcCall {
  let id = 0;
  let nextAt = 0;
  const pace = async () => {
    const now = Date.now();
    const at = Math.max(now, nextAt);
    nextAt = at + minIntervalMs;
    if (at > now) await new Promise((r) => setTimeout(r, at - now));
  };
  return async (method, params) => {
    let wait = 500;
    await pace();
    for (let i = 1; ; i++) {
      const res = await fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }),
      });
      const body =
        res.status === 429
          ? { error: { message: "Too many requests" } }
          : ((await res.json()) as {
              result?: unknown;
              error?: { message: string };
            });
      // A rate limit arrives either as HTTP 429 or as a JSON-RPC error saying so — measured
      // against api.devnet.solana.com on 2026-10-03, which answers `getTransaction` bursts with
      // "Too many requests for a specific RPC call" in the body of a 200.
      if (body.error && i < attempts && isRateLimit(body.error.message)) {
        await new Promise((r) => setTimeout(r, wait));
        wait = Math.min(wait * 2, 8_000);
        continue;
      }
      if (body.error) throw new Error(`${method}: ${body.error.message}`);
      return body.result;
    }
  };
}

type SigInfo = { signature: string; slot: number; err: unknown };
type TxResult = {
  slot: number;
  blockTime: number | null;
  meta: {
    err: unknown;
    logMessages?: string[] | null;
    loadedAddresses?: { writable: string[]; readonly: string[] };
  } | null;
  transaction: { message: { accountKeys: string[] } };
} | null;

/**
 * Bring the store up to date: every program transaction newer than the newest stored, or the
 * whole history on the first run. Returns how many were added.
 */
export async function syncOnce(
  store: NoxStore,
  rpc: RpcCall,
  program: string = NOXFUNDS_PROGRAM_ADDRESS,
  /** `getTransaction` calls in flight. Bounded: every public endpoint rate-limits a burst. */
  concurrency = 2
): Promise<number> {
  const until = store.newestSignature();
  const sigs: SigInfo[] = [];
  let before: string | undefined;
  for (;;) {
    const page = (await rpc("getSignaturesForAddress", [
      program,
      { limit: 1000, before, until, commitment: "confirmed" },
    ])) as SigInfo[];
    sigs.push(...page);
    if (page.length < 1000) break;
    before = page[page.length - 1]?.signature;
    if (!before) break;
  }
  if (sigs.length === 0) return 0;

  // Fetched in parallel, written back by index, so the RPC's order survives the concurrency.
  const txs: IndexedTx[] = new Array<IndexedTx>(sigs.length);
  let next = 0;
  const worker = async () => {
    for (;;) {
      const i = next++;
      const s = sigs[i];
      if (!s) return;
      const cached = store.cachedTx(s.signature);
      if (cached) {
        txs[i] = cached;
        continue;
      }
      const t = await fetchTx(rpc, s);
      store.cacheTx(t);
      txs[i] = t;
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, concurrency) }, worker));
  return store.insertNewestFirst(txs);
}

async function fetchTx(rpc: RpcCall, s: SigInfo): Promise<IndexedTx> {
  {
    const t = (await rpc("getTransaction", [
      s.signature,
      {
        commitment: "confirmed",
        encoding: "json",
        maxSupportedTransactionVersion: 0,
      },
    ])) as TxResult;
    const meta = t?.meta;
    return {
      signature: s.signature,
      slot: BigInt(t?.slot ?? s.slot),
      blockTime:
        t?.blockTime === null || t?.blockTime === undefined
          ? null
          : BigInt(t.blockTime),
      logs: meta?.logMessages ?? [],
      accounts: [
        ...(t?.transaction.message.accountKeys ?? []),
        ...(meta?.loadedAddresses?.writable ?? []),
        ...(meta?.loadedAddresses?.readonly ?? []),
      ],
      failed: s.err !== null || (meta?.err ?? null) !== null,
    };
  }
}

/** The profile address for a trader, derived as the program derives it. */
export const profileOf = (trader: string) => findProfile(trader as Address);

export type { NoxEvent };
