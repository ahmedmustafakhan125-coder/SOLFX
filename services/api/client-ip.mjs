/**
 * Which address a request came from, for a limit that must not be forgeable.
 *
 * Behind Traefik, `X-Forwarded-For` is a list: whatever the client sent, then the address
 * Traefik saw, appended. Only that **last** entry was written by something we run. Counting the
 * first one — the usual idiom, and the one the token bucket in `server.mjs` uses — lets a client
 * pick its own address with a header. For a bucket that only smooths load that is tolerable; for
 * the faucet's once-a-day limit it would be the whole bypass.
 *
 * Dependency-free so `server.mjs` stays that way, and its own module so it can be tested.
 */
export function lastForwardedFor(header) {
  const value = Array.isArray(header) ? header.at(-1) : header;
  const parts = (value ?? "")
    .split(",")
    .map((p) => p.trim())
    .filter(Boolean);
  return parts.at(-1) ?? "";
}

/** The faucet's view of the caller: the trusted forwarded hop, else the socket peer. */
export function clientIp(req) {
  return lastForwardedFor(req.headers["x-forwarded-for"]) || req.socket?.remoteAddress || "unknown";
}
