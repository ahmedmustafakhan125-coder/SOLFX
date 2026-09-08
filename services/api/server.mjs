/**
 * The production stand-in for the two Vite dev-server proxies.
 *
 * `app/vite.config.ts` proxies `/hermes` and `/pythpro` and attaches the Pyth bearer token
 * server-side. Those proxies exist only while `npx vite` is running, so a static production
 * build has nothing behind those paths: the ticker and the charts 404. Moving the calls
 * upstream in the browser is not an option either — that would publish `PYTH_API_KEY`, and a
 * key in client JavaScript is a public key.
 *
 * So this serves the built app and answers the same two prefixes with the same rewrites the
 * dev server uses. No dependencies: Node 22's http and fetch are enough.
 */
import { createServer } from "node:http";
import { createReadStream } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath, URL as NodeURL } from "node:url";

const here = fileURLToPath(new NodeURL(".", import.meta.url));
const repoRoot = resolve(here, "..", "..");

/** Upstream host for both prefixes — Pyth's upgraded backend, one key for Hermes and Pro. */
const UPSTREAM = process.env.SOLFX_PYTH_UPSTREAM ?? "https://pyth.dourolabs.app";
const PORT = Number(process.env.PORT ?? 8787);
const HOST = process.env.HOST ?? "0.0.0.0";
const WEB_ROOT = resolve(process.env.SOLFX_WEB_ROOT ?? join(repoRoot, "app", "dist"));
const TIMEOUT_MS = Number(process.env.SOLFX_PROXY_TIMEOUT_MS ?? 10_000);

/**
 * The bearer token, read the same way `vite.config.ts` reads it: the process environment
 * first (systemd passes it that way), then the repo-root `.env`. Read once at startup — a
 * key that changes is a restart, which is what the runbook already says.
 */
async function pythKey() {
  const fromEnv = process.env.PYTH_API_KEY?.trim();
  if (fromEnv) return fromEnv;
  try {
    const env = await readFile(join(repoRoot, ".env"), "utf8");
    return /^PYTH_API_KEY=(.*)$/m.exec(env)?.[1]?.trim() ?? "";
  } catch {
    return "";
  }
}
/**
 * The Solana RPC the browser talks to, resolved like the Pyth key: environment first, then
 * the repo-root `.env`.
 *
 * The terminal used to point straight at `https://api.devnet.solana.com`, and that is why it
 * showed no prices. Measured 2026-09-08 from this box: 40 rapid `getMultipleAccounts` reads —
 * exactly the shape of one terminal poll — returned **0 OK and 40 `429`** on the public
 * endpoint and **40 OK, 0 `429`** on the keyed one. The public devnet RPC cannot serve a
 * trading UI.
 *
 * It is proxied rather than baked into the bundle for the same reason the Pyth token is: an
 * endpoint carrying `?api-key=` in client JavaScript is a public endpoint.
 */
async function rpcEndpoint() {
  const fromEnv = process.env.SOLFX_RPC_URL?.trim();
  if (fromEnv) return fromEnv;
  try {
    const env = await readFile(join(repoRoot, ".env"), "utf8");
    return /^SOLFX_RPC_URL=(.*)$/m.exec(env)?.[1]?.trim() ?? "";
  } catch {
    return "";
  }
}
/** Where the poster's live map is mounted. Empty disables the route and the bundle wins. */
const PUBLIC_ROOT = process.env.SOLFX_PUBLIC_ROOT?.trim() || "";

const RPC = await rpcEndpoint();
if (!RPC) console.error("[solfx-api] no SOLFX_RPC_URL — /rpc will return 502.");

/**
 * What the browser is allowed to ask for.
 *
 * `/rpc` is unauthenticated by necessity — the wallet in the browser cannot hold a secret —
 * so without this the route is an open door onto a metered endpoint. The list is what the
 * terminal actually calls; anything else belongs to a keeper or an admin tool, which have
 * their own key and do not come through here.
 */
const RPC_METHODS = new Set([
  "getAccountInfo", "getMultipleAccounts", "getProgramAccounts", "getBalance",
  "getLatestBlockhash", "isBlockhashValid", "getBlockHeight", "getSlot", "getEpochInfo",
  "getSignatureStatuses", "getSignaturesForAddress", "getTransaction",
  "sendTransaction", "simulateTransaction", "getFeeForMessage",
  "getMinimumBalanceForRentExemption", "getTokenAccountBalance", "getTokenAccountsByOwner",
  "getHealth", "getVersion",
]);

const RPC_MAX_BODY = 512 * 1024;

async function rpcProxy(req, res) {
  const deny = (status, error) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify({ error }));
  };
  if (req.method !== "POST") return void deny(405, "rpc is POST only");
  if (!RPC) return void deny(502, "SOLFX_RPC_URL is not configured on the server");

  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > RPC_MAX_BODY) return void deny(413, "request too large");
    chunks.push(chunk);
  }
  const body = Buffer.concat(chunks);

  // A JSON-RPC call may be a single object or a batch; every method in it must be allowed.
  try {
    const parsed = JSON.parse(body.toString("utf8"));
    const calls = Array.isArray(parsed) ? parsed : [parsed];
    for (const c of calls) {
      if (!RPC_METHODS.has(c?.method)) return void deny(403, `method not allowed: ${c?.method}`);
    }
  } catch {
    return void deny(400, "body is not JSON");
  }

  try {
    const upstream = await fetch(RPC, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    const headers = {};
    for (const [k, v] of upstream.headers) if (!DROP.has(k.toLowerCase())) headers[k] = v;
    headers["cache-control"] = "no-store";
    res.writeHead(upstream.status, headers);
    if (!upstream.body) return void res.end();
    for await (const chunk of upstream.body) res.write(chunk);
    res.end();
  } catch (err) {
    const timedOut = err?.name === "TimeoutError" || err?.name === "AbortError";
    // Never log RPC because it carries the api key in its query string.
    console.error(`[solfx-api] ${timedOut ? "timeout" : "upstream error"} for /rpc`);
    deny(timedOut ? 504 : 502, timedOut ? "upstream timeout" : "upstream unreachable");
  }
}

const KEY = await pythKey();
if (!KEY) {
  console.error(
    "[solfx-api] no PYTH_API_KEY — /hermes and /pythpro will return 502. " +
      `Put it in ${join(repoRoot, ".env")} or the unit's Environment= and restart.`,
  );
}

/** `/hermes/v2/...` keeps its prefix; `/pythpro/...` becomes `/v1/...`. As in vite.config.ts. */
function upstreamUrl(pathname, search) {
  if (pathname === "/hermes" || pathname.startsWith("/hermes/")) {
    return `${UPSTREAM}${pathname}${search}`;
  }
  if (pathname === "/pythpro" || pathname.startsWith("/pythpro/")) {
    return `${UPSTREAM}/v1${pathname.slice("/pythpro".length)}${search}`;
  }
  return null;
}

/**
 * Hop-by-hop headers must not be forwarded, and the upstream's CORS headers are meaningless
 * here — the browser sees a same-origin response, which is the whole point of proxying.
 */
const DROP = new Set([
  "connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer",
  "transfer-encoding", "upgrade", "content-encoding", "content-length",
  "access-control-allow-origin", "access-control-allow-credentials",
]);

async function proxy(req, res, target) {
  if (req.method !== "GET" && req.method !== "HEAD") {
    res.writeHead(405, { allow: "GET, HEAD", "content-type": "application/json" });
    res.end(JSON.stringify({ error: "read-only proxy" }));
    return;
  }
  if (!KEY) {
    res.writeHead(502, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "PYTH_API_KEY is not configured on the server" }));
    return;
  }

  const abort = AbortSignal.timeout(TIMEOUT_MS);
  try {
    const upstream = await fetch(target, {
      method: req.method,
      headers: { authorization: `Bearer ${KEY}`, accept: req.headers.accept ?? "application/json" },
      signal: abort,
    });

    const headers = {};
    for (const [k, v] of upstream.headers) if (!DROP.has(k.toLowerCase())) headers[k] = v;
    // Prices are live and history is cheap to refetch; never let a CDN or the browser pin
    // either. A stale tick is worse than a slow one.
    headers["cache-control"] = "no-store";

    res.writeHead(upstream.status, headers);
    if (req.method === "HEAD" || !upstream.body) return void res.end();
    for await (const chunk of upstream.body) res.write(chunk);
    res.end();
  } catch (err) {
    const timedOut = err?.name === "TimeoutError" || err?.name === "AbortError";
    // The token is in the request we built; keep it out of the log line and the response.
    console.error(`[solfx-api] ${timedOut ? "timeout" : "upstream error"} for ${new NodeURL(target).pathname}`);
    res.writeHead(timedOut ? 504 : 502, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: timedOut ? "upstream timeout" : "upstream unreachable" }));
  }
}

const MIME = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png",
  ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp", ".ico": "image/x-icon",
  ".woff": "font/woff", ".woff2": "font/woff2", ".ttf": "font/ttf", ".map": "application/json",
  ".wasm": "application/wasm", ".txt": "text/plain; charset=utf-8",
};

async function serveFile(res, path, status = 200) {
  const info = await stat(path);
  if (!info.isFile()) throw new Error("not a file");
  // Vite fingerprints everything under /assets, so those are safe to pin forever. index.html
  // is the map to them and must never be cached, or a deploy serves old chunk names.
  const immutable = path.includes(`${WEB_ROOT}/assets/`);
  res.writeHead(status, {
    "content-type": MIME[extname(path).toLowerCase()] ?? "application/octet-stream",
    "content-length": info.size,
    "cache-control": immutable ? "public, max-age=31536000, immutable" : "no-cache",
    "x-content-type-options": "nosniff",
  });
  createReadStream(path).pipe(res);
}

async function serveStatic(req, res, pathname) {
  // normalize() collapses `..` before the prefix check, so a crafted path cannot escape the
  // web root; anything that still points outside it is refused.
  const candidate = resolve(join(WEB_ROOT, normalize(decodeURIComponent(pathname))));
  if (candidate !== WEB_ROOT && !candidate.startsWith(`${WEB_ROOT}/`)) {
    res.writeHead(403).end();
    return;
  }
  try {
    await serveFile(res, pathname.endsWith("/") ? join(candidate, "index.html") : candidate);
  } catch {
    // A single-page app owns its own routes: an unknown path is a client route, not a 404.
    try {
      await serveFile(res, join(WEB_ROOT, "index.html"));
    } catch {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end("app/dist is missing — run `npm run build` in app/\n");
    }
  }
}

const server = createServer((req, res) => {
  const { pathname, search } = new NodeURL(req.url ?? "/", "http://localhost");

  if (pathname === "/healthz") {
    res.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
    res.end(JSON.stringify({ ok: true, key: KEY ? "present" : "missing", uptime: process.uptime() }));
    return;
  }

  // The live price-account map, served from the directory the poster writes to rather than
  // from the bundle. `app/public/` is a build-time input, so a map copied there is only
  // picked up by a rebuild — and the failure mode is silent: the poster publishes happily
  // while every market in the terminal reads "No price account for this feed on this
  // cluster". Serving it from disk means the browser always sees the current map.
  if (pathname === "/price-accounts.json" && PUBLIC_ROOT) {
    return void serveFile(res, join(PUBLIC_ROOT, "price-accounts.json")).catch(() =>
      serveStatic(req, res, pathname),
    );
  }

  if (pathname === "/rpc") return void rpcProxy(req, res);

  const target = upstreamUrl(pathname, search);
  if (target) return void proxy(req, res, target);
  serveStatic(req, res, pathname);
});

server.listen(PORT, HOST, () => {
  console.log(`[solfx-api] listening on ${HOST}:${PORT}, serving ${WEB_ROOT}, key ${KEY ? "loaded" : "MISSING"}`);
});

for (const sig of ["SIGTERM", "SIGINT"]) {
  process.on(sig, () => server.close(() => process.exit(0)));
}
