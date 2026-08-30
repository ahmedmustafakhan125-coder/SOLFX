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
