/**
 * The "Get test USDC" path for a first-time wallet: when to offer it, how to ask, and how to
 * explain the two failures a newcomer on devnet actually hits.
 *
 * The server side is `services/faucet/faucet.mjs`, reached through `POST /faucet` on this origin.
 * It decides amounts and limits; nothing here can change either.
 */

/** Below this wallet balance the button is shown: 100 test USDC at 6 decimals. */
export const FAUCET_OFFER_BELOW = 100_000_000n;

/**
 * Fired on `window` after a claim lands, so every panel showing a balance re-reads it now
 * instead of on its next poll — a judge who just clicked the button should not stare at $0.00
 * for twenty seconds.
 */
export const BALANCES_CHANGED = "solfx:balances-changed";

export function shouldOfferFaucet(walletUsdc: bigint | undefined): boolean {
  return walletUsdc !== undefined && walletUsdc < FAUCET_OFFER_BELOW;
}

export type FaucetResult =
  | {
      readonly ok: true;
      readonly signature: string;
      readonly usdc: string;
      readonly sol: string;
      /** Sent but not yet confirmed when the server stopped waiting. */
      readonly pending: boolean;
    }
  | { readonly ok: false; readonly error: string };

/**
 * Read the server's answer. Every refusal carries a sentence meant for the person — "try again in
 * 5h 12m" — so it is shown as written rather than replaced by a status code.
 */
export function parseFaucetResponse(
  status: number,
  body: unknown
): FaucetResult {
  const b = (body ?? {}) as Record<string, unknown>;
  if ((status === 200 || status === 202) && typeof b.signature === "string") {
    return {
      ok: true,
      signature: b.signature,
      usdc: String(b.usdc ?? "0"),
      sol: String(b.sol ?? "0"),
      pending: status === 202 || b.pending === true,
    };
  }
  if (typeof b.error === "string" && b.error)
    return { ok: false, error: b.error };
  if (status === 429)
    return { ok: false, error: "Too many requests. Try again in a minute." };
  return {
    ok: false,
    error: `The faucet did not answer (HTTP ${status}). Please try again.`,
  };
}

export async function requestFaucet(
  wallet: string,
  fetcher: typeof fetch = fetch
): Promise<FaucetResult> {
  try {
    const r = await fetcher("/faucet", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ wallet }),
    });
    let body: unknown;
    try {
      body = await r.json();
    } catch {
      body = undefined;
    }
    return parseFaucetResponse(r.status, body);
  } catch {
    return {
      ok: false,
      error: "Could not reach the faucet. Check your connection.",
    };
  }
}

export function explorerTx(signature: string): string {
  return `https://explorer.solana.com/tx/${signature}?cluster=devnet`;
}

/** "10000" → "10,000". Display only; the amount itself is the server's string. */
export function groupThousands(amount: string): string {
  const [whole, frac] = amount.split(".");
  const grouped = (whole ?? "").replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return frac ? `${grouped}.${frac}` : grouped;
}

/** What a failure says when the wallet is pointed at the wrong cluster, or has no SOL at all. */
export const CLUSTER_HINT =
  "SolFX runs on Solana devnet. If your wallet is on Mainnet, switch it to Devnet (Phantom: Settings → Developer Settings). A new wallet also needs devnet SOL: use Get test USDC.";

/**
 * Does this error look like a devnet transaction meeting a wallet on another cluster?
 *
 * Each pattern is what one of those failures actually reads as. A devnet blockhash is unknown
 * to mainnet ("Blockhash not found"); a fresh wallet has no SOL on whichever cluster it is on
 * ("no record of a prior credit"); a devnet-only account does not exist on mainnet
 * ("AccountNotFound"). The last two also fire for a devnet wallet with no SOL, whose fix is the
 * faucet, which is why the hint names both.
 */
export function looksLikeWrongCluster(message: string): boolean {
  return /blockhash not found|no record of a prior credit|AccountNotFound|account does not exist/i.test(
    message
  );
}

export function withClusterHint(message: string): string {
  return looksLikeWrongCluster(message) && !message.includes(CLUSTER_HINT)
    ? `${message} ${CLUSTER_HINT}`
    : message;
}
