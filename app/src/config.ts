/**
 * The RPC the app talks to. Devnet by default; `VITE_SOLFX_RPC_URL` overrides it, because
 * the public devnet endpoint rate-limits hard enough to make the terminal look broken.
 */
export const RPC_URL: string =
  import.meta.env.VITE_SOLFX_RPC_URL ?? "https://api.devnet.solana.com";

export function rpcLabel(url: string): string {
  try {
    // The deployed app talks to the same-origin `/rpc` proxy, which forwards to a keyed
    // devnet endpoint. The host is the site, so name the cluster it actually reaches.
    if (url === "/rpc" || url.endsWith("/rpc")) return "devnet";
    const host = new URL(url).hostname;
    if (host.includes("devnet")) return "devnet";
    if (host.includes("testnet")) return "testnet";
    if (host.includes("127.0.0.1") || host.includes("localhost")) return "localnet";
    return host;
  } catch {
    return "rpc";
  }
}

/**
 * Hermes, for oracle price history — a same-origin path, not the upstream URL.
 *
 * Two reasons. Hermes answers the CORS preflight but omits `access-control-allow-origin` on
 * the actual response, so a direct browser fetch is blocked even though curl and Node
 * succeed. And the bearer token it has required since 26 Aug 2026 must not ship in client
 * JavaScript, where it would be public. Both are solved by the proxy in vite.config.ts,
 * which attaches the token server-side.
 *
 * A production deployment needs the equivalent proxy in front of it — this path assumes one.
 */
export const HERMES_URL: string = import.meta.env.VITE_HERMES_URL ?? "/hermes";

/** Unused when proxying; the proxy holds the token. Kept for a direct-fetch fallback. */
export const HERMES_TOKEN: string | undefined = import.meta.env.VITE_HERMES_TOKEN;

/** Pyth Pro History API, proxied — see the note in vite.config.ts. */
export const PYTHPRO_URL: string = import.meta.env.VITE_PYTHPRO_URL ?? "/pythpro";
