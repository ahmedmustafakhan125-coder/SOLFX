import { useState } from "react";

const KEY = "solfx.devnet-hint.dismissed";

function readDismissed(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    // Private windows and blocked storage throw. The hint then shows every visit, which is fine.
    return false;
  }
}

/**
 * A one-line reminder, beside Connect Wallet, that this is devnet.
 *
 * Phantom opens on Mainnet, where none of SolFX's accounts or test tokens exist, and the wallet
 * gives no hint why a balance reads zero or a signature request looks wrong. Dismissed once per
 * browser: a per-viewer convenience with nothing at stake, so `localStorage` is the right place.
 */
export function DevnetHint() {
  const [dismissed, setDismissed] = useState(readDismissed);
  if (dismissed) return null;

  function dismiss() {
    setDismissed(true);
    try {
      localStorage.setItem(KEY, "1");
    } catch {
      // Not remembered; dismissed for this page view only.
    }
  }

  return (
    <span className="hidden items-center gap-1.5 rounded-md border border-line px-2 py-1 text-[11px] text-ink-muted lg:flex">
      <span>
        <span className="font-semibold text-ink">Devnet only:</span> switch
        Phantom to Devnet (Settings → Developer Settings)
      </span>
      <button
        onClick={dismiss}
        className="text-ink-dim hover:text-ink"
        aria-label="Dismiss the devnet hint"
      >
        ×
      </button>
    </span>
  );
}
