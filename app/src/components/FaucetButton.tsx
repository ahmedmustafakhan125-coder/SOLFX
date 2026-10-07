import { useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";

import { useAccount } from "@/hooks/useAccount";
import {
  BALANCES_CHANGED,
  explorerTx,
  groupThousands,
  requestFaucet,
  shouldOfferFaucet,
  type FaucetResult,
} from "@/lib/faucet";

type State =
  | { readonly kind: "idle" }
  | { readonly kind: "loading" }
  | { readonly kind: "done"; readonly result: FaucetResult };

/**
 * "Get test USDC" beside the wallet button, for a wallet holding less than 100.
 *
 * The collateral mint is SolFX's own test token, so no public faucet has it, and a judge with a
 * fresh Phantom wallet otherwise has no way to place a first trade. One click asks
 * `services/faucet` for test USDC (and SOL, if the wallet has almost none), then tells every
 * balance on the page to re-read.
 *
 * The result stays on screen after the balance rises, so the explorer link does not vanish the
 * moment the claim succeeds.
 */
export function FaucetButton() {
  const { wallet } = useWalletConnection();
  const address = wallet?.account.address.toString();
  // Slower than the terminal's own account poll: this only decides whether to show a button.
  const { status, refresh } = useAccount(30_000);
  const [state, setState] = useState<State>({ kind: "idle" });

  if (!address) return null;
  const offer = shouldOfferFaucet(status?.walletUsdc);
  if (!offer && state.kind === "idle") return null;

  async function claim() {
    if (!address) return;
    setState({ kind: "loading" });
    const result = await requestFaucet(address);
    setState({ kind: "done", result });
    if (result.ok) {
      refresh();
      window.dispatchEvent(new Event(BALANCES_CHANGED));
    }
  }

  return (
    <div className="relative">
      {offer ? (
        <button
          onClick={() => void claim()}
          disabled={state.kind === "loading"}
          className="rounded-md border border-brand px-3 py-1.5 text-xs font-semibold text-brand hover:bg-brand hover:text-[var(--sf-on-brand)] disabled:opacity-60"
          title="Free devnet test USDC, plus a little SOL for fees if your wallet has none. Once a day."
        >
          {state.kind === "loading" ? "Sending…" : "Get test USDC"}
        </button>
      ) : null}

      {state.kind === "done" ? (
        <div
          role="status"
          className="absolute right-0 z-30 mt-2 w-72 rounded-md border border-line bg-surface-high p-3 text-xs shadow-xl"
        >
          <button
            onClick={() => setState({ kind: "idle" })}
            className="absolute right-2 top-1.5 text-ink-dim hover:text-ink"
            aria-label="Dismiss"
          >
            ×
          </button>
          {state.result.ok ? (
            <div className="space-y-1 pr-4">
              <div className="font-semibold text-long">
                +{groupThousands(state.result.usdc)} test USDC
                {state.result.sol !== "0" ? ` and ${state.result.sol} SOL` : ""}
              </div>
              <div className="text-ink-muted">
                {state.result.pending
                  ? "Sent. Devnet is slow to confirm right now; it should arrive within a minute."
                  : "In your wallet. Next: create your SolFX account and deposit."}
              </div>
              <a
                href={explorerTx(state.result.signature)}
                target="_blank"
                rel="noreferrer"
                className="text-brand underline"
              >
                View on Solana Explorer
              </a>
            </div>
          ) : (
            <div className="pr-4 text-short">{state.result.error}</div>
          )}
        </div>
      ) : null}
    </div>
  );
}
