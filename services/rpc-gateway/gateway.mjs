/**
 * One RPC budget, shared between the poster and the keeper, with the poster winning.
 *
 * # Why this exists
 *
 * Every off-chain process talks to the same endpoint, and that endpoint allows roughly ten
 * calls a second between all of them. Each process throttles *itself* (`--max-rps`), which
 * works in isolation and not at all together: two processes each politely holding to 4 rps
 * still put 8 rps on a shared pipe, and neither can see the other to back off.
 *
 * Measured 2026-09-08 on this box: the poster alone ran 14 consecutive passes at 6 of 6
 * feeds. Starting the keeper beside it dropped the poster to `0 posted, 6 failed` within a
 * minute, and stopping the keeper restored it immediately. Nothing was misconfigured — there
 * was simply no component whose job was to divide the budget.
 *
 * # Why priority rather than a smaller share each
 *
 * The two workloads are not equally urgent, so splitting the budget evenly is the wrong
 * answer. A price is only tradeable for 60 seconds after publication, so a poster call that
 * waits is a feed that goes stale and a trade that fails `OracleStale` — every trade, for
 * every user. A keeper call that waits is a liquidation that happens a few seconds later,
 * which is a delay and not a failure. So the poster is served first and the keeper drinks
 * what is left, which under load means the keeper slows down and the prices stay fresh.
 *
 * Both processes still keep their own `--max-rps`. This is a ceiling over both of them, not
 * a replacement for either.
 *
 * # Shape
 *
 *   poster  ->  http://127.0.0.1:8899/high  \
 *                                            >-- token bucket --> the real endpoint
 *   keeper  ->  http://127.0.0.1:8899/low   /
 *
 * Loopback only, and no api key of its own: the upstream URL carries it and never leaves
 * this process.
 */
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HOST = process.env.SOLFX_GATEWAY_HOST ?? "127.0.0.1";
const PORT = Number(process.env.SOLFX_GATEWAY_PORT ?? 8899);
/** Calls per second to allow upstream, across every client. Below the tier's ceiling. */
const RATE = Number(process.env.SOLFX_GATEWAY_RPS ?? 9);
/** A short burst is fine and smooths a pass's opening; a long one is what triggers a 429. */
const BURST = Number(process.env.SOLFX_GATEWAY_BURST ?? 4);
/**
 * Queue depth per priority, and they are deliberately far apart.
 *
 * Shedding the poster defeats the point: its call *is* the critical path, and a 429 from
 * here fails the feed instantly where waiting 200ms would have posted it. The first version
 * of this file used one cap for both and did exactly that — the poster's own log filled with
 * `429 Too Many Requests for url (http://127.0.0.1:8899/high)`, a rate limit it had imposed
 * on itself. So the poster queues almost without bound and the keeper is what yields: a
 * dropped crank is retried on the next tick and nothing is lost but a few seconds.
 */
const MAX_QUEUE_HIGH = Number(process.env.SOLFX_GATEWAY_MAX_QUEUE_HIGH ?? 4096);
const MAX_QUEUE_LOW = Number(process.env.SOLFX_GATEWAY_MAX_QUEUE_LOW ?? 32);
const UPSTREAM_TIMEOUT_MS = Number(process.env.SOLFX_GATEWAY_TIMEOUT_MS ?? 20_000);
const MAX_BODY = 2 * 1024 * 1024;

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

/** The real endpoint, resolved like every other secret here: env first, then repo `.env`. */
async function upstream() {
  const fromEnv = process.env.SOLFX_RPC_URL?.trim();
  if (fromEnv && !fromEnv.includes(`:${PORT}`)) return fromEnv;
  try {
    const env = await readFile(join(repoRoot, ".env"), "utf8");
    const found = /^SOLFX_RPC_URL=(.*)$/m.exec(env)?.[1]?.trim() ?? "";
    // Guard against the obvious footgun: pointing the gateway at itself.
    return found.includes(`:${PORT}`) ? "" : found;
  } catch {
    return "";
  }
}

const UPSTREAM = await upstream();
if (!UPSTREAM) {
  console.error("[rpc-gateway] no usable SOLFX_RPC_URL — refusing to start");
  process.exit(1);
}

// --- the budget -------------------------------------------------------------------------

let tokens = BURST;
let last = Date.now();

function refill() {
  const now = Date.now();
  tokens = Math.min(BURST, tokens + ((now - last) / 1000) * RATE);
  last = now;
}

/** Two queues, not one with a sort: arrival order must hold *within* a priority. */
const queues = { high: [], low: [] };
const stats = { high: 0, low: 0, shed: 0, upstream429: 0, retried: 0, gaveUp: 0 };

/**
 * Retry an upstream 429, briefly.
 *
 * Measured 2026-09-13: Helius's free tier refuses roughly **20% of `sendTransaction` calls at
 * any offered rate** — 3 of 8 at one per second, and still 2 of 10 at one per three seconds.
 * A rate limit goes away when you go below it; this does not, so no amount of spacing on the
 * caller's side fixes it. With 7 sends in a crypto pass, 0.8^7 leaves about a fifth of passes
 * intact, which is what was observed.
 *
 * `price_poster.rs` argues against retrying, on the grounds that `solana-rpc-client` already
 * retries five times and honours `Retry-After` for up to 120 s, which would hold a feed for
 * minutes against a 60-second gate. **That reasoning does not apply to this endpoint:**
 * measured the same day, its 429 carries *no* `Retry-After` header at all. So a retry is
 * immediate and costs one more attempt rather than two minutes.
 *
 * Three attempts turns a 20% refusal into 0.8%, and a 7-send pass from ~21% intact to ~95%.
 *
 * Retrying a *send* is safe here specifically because a 429 is refused at Helius's edge — the
 * transaction never reached the network. Even if it had, Solana deduplicates by signature, so
 * resubmitting a signed transaction is the normal confirmation path rather than a double-spend.
 */
const RETRY_ATTEMPTS = Number(process.env.SOLFX_GATEWAY_RETRIES ?? 3);
const RETRY_BASE_MS = Number(process.env.SOLFX_GATEWAY_RETRY_MS ?? 350);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function pump() {
  refill();
  while (tokens >= 1) {
    const next = queues.high.shift() ?? queues.low.shift();
    if (!next) return;
    tokens -= 1;
    next.go();
  }
}
// One timer for the whole process. Fine-grained enough to smooth a 9/s bucket, and it does
// not spin per request.
setInterval(pump, 40).unref();

function schedule(priority) {
  const queue = queues[priority];
  const cap = priority === "high" ? MAX_QUEUE_HIGH : MAX_QUEUE_LOW;
  if (queue.length >= cap) {
    stats.shed += 1;
    return null;
  }
  stats[priority] += 1;
  return new Promise((resolve) => {
    queue.push({ go: resolve });
    pump();
  });
}

// --- the proxy --------------------------------------------------------------------------

const server = createServer(async (req, res) => {
  const send = (status, obj) => {
    res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
    res.end(JSON.stringify(obj));
  };

  const path = (req.url ?? "/").split("?")[0];

  if (path === "/healthz") {
    return void send(200, {
      ok: true,
      rate: RATE,
      queued: { high: queues.high.length, low: queues.low.length },
      served: stats,
    });
  }

  const priority = path === "/high" ? "high" : path === "/low" ? "low" : null;
  if (!priority) return void send(404, { error: "use /high (poster) or /low (keeper)" });
  if (req.method !== "POST") return void send(405, { error: "rpc is POST only" });

  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > MAX_BODY) return void send(413, { error: "request too large" });
    chunks.push(chunk);
  }
  const body = Buffer.concat(chunks);

  const slot = schedule(priority);
  if (!slot) {
    // Deliberately 429, not 503: every client here already knows how to interpret a 429 from
    // an RPC endpoint, and this is the same fact — you asked for more than the budget allows.
    return void send(429, { error: "gateway queue full" });
  }
  await slot;

  try {
    let upstreamRes;
    for (let attempt = 0; ; attempt++) {
      upstreamRes = await fetch(UPSTREAM, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body,
        signal: AbortSignal.timeout(UPSTREAM_TIMEOUT_MS),
      });
      if (upstreamRes.status === 429) stats.upstream429 += 1;
      if (upstreamRes.status !== 429 || attempt >= RETRY_ATTEMPTS) {
        if (upstreamRes.status === 429) stats.gaveUp += 1;
        break;
      }
      // Jittered, so a pass's sends do not all retry on the same tick and collide again.
      // Bounded on purpose: worst case here is well under a second, against a 60 s gate.
      stats.retried += 1;
      await sleep(RETRY_BASE_MS + Math.floor(Math.random() * RETRY_BASE_MS));
    }
    const text = await upstreamRes.text();
    res.writeHead(upstreamRes.status, {
      "content-type": upstreamRes.headers.get("content-type") ?? "application/json",
      "cache-control": "no-store",
    });
    res.end(text);
  } catch (err) {
    const timedOut = err?.name === "TimeoutError" || err?.name === "AbortError";
    // Never log UPSTREAM: it carries the api key in its query string.
    console.error(`[rpc-gateway] ${timedOut ? "upstream timeout" : "upstream unreachable"}`);
    send(timedOut ? 504 : 502, { error: timedOut ? "upstream timeout" : "upstream unreachable" });
  }
});

server.listen(PORT, HOST, () => {
  console.log(`[rpc-gateway] ${HOST}:${PORT} -> upstream at ${RATE}/s (burst ${BURST})`);
  console.log("[rpc-gateway] /high = poster (served first), /low = keeper");
});

// A periodic line, so the journal shows whether the keeper is actually being starved rather
// than merely suspected of it.
setInterval(() => {
  if (stats.high + stats.low === 0) return;
  console.log(
    `[rpc-gateway] served high=${stats.high} low=${stats.low} shed=${stats.shed} ` +
      `upstream429=${stats.upstream429} retried=${stats.retried} gaveup=${stats.gaveUp} ` +
      `queued=${queues.high.length}/${queues.low.length}`,
  );
  stats.high = stats.low = stats.shed = stats.upstream429 = stats.retried = stats.gaveUp = 0;
}, 60_000).unref();
