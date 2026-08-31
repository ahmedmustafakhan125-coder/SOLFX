/**
 * The RPC the app talks to. Devnet by default; `VITE_SOLFX_RPC_URL` overrides it, because
 * the public devnet endpoint rate-limits hard enough to make the terminal look broken.
 */
export const RPC_URL: string =
  import.meta.env.VITE_SOLFX_RPC_URL ?? "https://api.devnet.solana.com";

export function rpcLabel(url: string): string {
  try {
    const host = new URL(url).hostname;
    if (host.includes("devnet")) return "devnet";
    if (host.includes("testnet")) return "testnet";
    if (host.includes("127.0.0.1") || host.includes("localhost")) return "localnet";
    return host;
  } catch {
    return "rpc";
  }
}

/** Hermes, for oracle price history. Same endpoint the poster uses. */
export const HERMES_URL: string =
  import.meta.env.VITE_HERMES_URL ?? "https://pyth.dourolabs.app/hermes";

/**
 * Hermes needs a bearer token since 26 Aug 2026. Shipping it to the browser makes it public,
 * which is acceptable for a devnet demo and is not acceptable for production — there it
 * belongs behind a proxy that holds the key server-side.
 */
export const HERMES_TOKEN: string | undefined = import.meta.env.VITE_HERMES_TOKEN;
