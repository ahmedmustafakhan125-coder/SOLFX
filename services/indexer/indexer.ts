/**
 * The NOXFUNDS indexer service: keeps `clients/js/src/nox/indexer.ts`'s store in step with the
 * chain and answers questions about traders over HTTP.
 *
 *   GET /api/nox/health                     last sync, last error, what is stored
 *   GET /api/nox/traders                    every trader: ✓, tier, the record's headline figures
 *   GET /api/nox/traders/:address           one trader in full: record, evaluations, mandates
 *   GET /api/nox/traders/:address?verify=1  the same, beside the profile account's own figures
 *
 * Run:  clients/js/node_modules/.bin/tsx services/indexer/indexer.ts
 *
 * Configuration, all optional, environment first and then the repo-root `.env`:
 *   NOX_INDEXER_RPC_URL  RPC to read from (falls back to SOLFX_RPC_URL, then public devnet)
 *   NOX_INDEXER_DB       SQLite file (default services/indexer/data/nox.sqlite)
 *   NOX_INDEXER_PORT     HTTP port (default 8788), bound to 127.0.0.1 unless NOX_INDEXER_HOST
 *   NOX_INDEXER_INTERVAL_MS  how often to sync (default 15000)
 *   NOX_INDEXER_PACE_MS  minimum gap between RPC calls (default 250; public endpoints throttle)
 *
 * The RPC URL may carry a key, so it is never logged — only its host.
 */
import { createServer } from "node:http";
import { mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  httpRpc,
  listTraders,
  NoxStore,
  syncOnce,
  traderEntity,
  verifyEntity,
} from "../../clients/js/src/nox/indexer.js";
import { findProfile } from "../../clients/js/src/nox/pdas.js";
import { getTraderProfileDecoder } from "../../clients/js/src/generated-noxfunds/accounts/traderProfile.js";
import type { Address } from "@solana/kit";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..");

function fromEnvFile(name: string): string | undefined {
  try {
    const env = readFileSync(join(repoRoot, ".env"), "utf8");
    return new RegExp(`^${name}=(.*)$`, "m").exec(env)?.[1]?.trim() || undefined;
  } catch {
    return undefined;
  }
}
const setting = (name: string) => process.env[name]?.trim() || fromEnvFile(name);

const RPC_URL =
  setting("NOX_INDEXER_RPC_URL") ?? setting("SOLFX_RPC_URL") ?? "https://api.devnet.solana.com";
const DB_PATH = setting("NOX_INDEXER_DB") ?? join(here, "data", "nox.sqlite");
const PORT = Number(setting("NOX_INDEXER_PORT") ?? 8788);
const HOST = setting("NOX_INDEXER_HOST") ?? "127.0.0.1";
const INTERVAL_MS = Number(setting("NOX_INDEXER_INTERVAL_MS") ?? 15_000);

mkdirSync(dirname(DB_PATH), { recursive: true });
const store = new NoxStore(DB_PATH);
// 250 ms between calls: a public endpoint refuses getTransaction bursts outright.
const rpc = httpRpc(RPC_URL, 8, Number(setting("NOX_INDEXER_PACE_MS") ?? 250));

const status = {
  rpcHost: new URL(RPC_URL).host,
  lastSyncAt: null as string | null,
  lastAdded: 0,
  lastError: null as string | null,
  syncing: false,
};

async function sync(): Promise<void> {
  if (status.syncing) return;
  status.syncing = true;
  try {
    status.lastAdded = await syncOnce(store, rpc);
    status.lastSyncAt = new Date().toISOString();
    status.lastError = null;
    if (status.lastAdded > 0) console.log(`[nox-indexer] +${status.lastAdded} transactions`);
  } catch (e) {
    // The message can contain the URL; keep only the method and the reason.
    status.lastError = String((e as Error)?.message ?? e).replace(RPC_URL, status.rpcHost);
    console.error(`[nox-indexer] sync failed: ${status.lastError}`);
  } finally {
    status.syncing = false;
  }
}

/** JSON with bigints as decimal strings: every figure here is a u64 or i64. */
function send(res: import("node:http").ServerResponse, code: number, body: unknown): void {
  const text = JSON.stringify(body, (_k, v) => (typeof v === "bigint" ? v.toString() : v), 2);
  res.writeHead(code, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "access-control-allow-origin": "*",
  });
  res.end(text);
}

async function onChainProfile(profile: string) {
  const r = (await rpc("getAccountInfo", [profile, { encoding: "base64", commitment: "confirmed" }])) as {
    value: { data: [string, string] } | null;
  };
  if (!r.value) return null;
  return getTraderProfileDecoder().decode(Buffer.from(r.value.data[0], "base64"));
}

const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const path = url.pathname.replace(/\/+$/, "");
  void (async () => {
    try {
      if (req.method !== "GET") return send(res, 405, { error: "GET only" });
      if (path === "/api/nox/health") {
        return send(res, 200, { ...status, stored: store.count(), newest: store.newestSignature() ?? null });
      }
      if (path === "/api/nox/traders") {
        const traders = listTraders(store).map(({ trader, profile }) => {
          const e = traderEntity(store, trader, profile);
          return {
            trader,
            profile,
            verified: e.verified,
            tier: e.record.lastTier ?? 0,
            trades: e.record.trades,
            wins: e.record.wins,
            grossProfit: e.record.grossProfit,
            grossLoss: e.record.grossLoss,
            maxDrawdownBps: e.record.maxDrawdownBps,
            evaluationsPassed: e.record.evaluationsPassed,
            mandates: e.mandates.length,
            truncatedTxs: e.truncatedTxs,
          };
        });
        return send(res, 200, { traders });
      }
      const m = /^\/api\/nox\/traders\/([1-9A-HJ-NP-Za-km-z]{32,44})$/.exec(path);
      if (m) {
        const trader = m[1]!;
        const profile = await findProfile(trader as Address);
        const entity = traderEntity(store, trader, profile);
        if (url.searchParams.get("verify") === "1") {
          const account = await onChainProfile(profile);
          return send(res, 200, {
            ...entity,
            verification: account ? verifyEntity(entity, account) : "no profile account on chain",
          });
        }
        return send(res, 200, entity);
      }
      return send(res, 404, { error: "not found" });
    } catch (e) {
      return send(res, 500, { error: String((e as Error)?.message ?? e).replace(RPC_URL, status.rpcHost) });
    }
  })();
});

server.listen(PORT, HOST, () => {
  console.log(`[nox-indexer] http://${HOST}:${PORT}/api/nox/health · rpc ${status.rpcHost} · db ${DB_PATH}`);
});
void sync();
setInterval(() => void sync(), INTERVAL_MS);
