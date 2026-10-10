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
 * A reminder, beside Connect Wallet, that this is devnet.
 *
 * Phantom opens on Mainnet, where none of SolFX's accounts or test tokens exist, and the wallet
 * gives no hint why a balance reads zero or a signature request looks wrong. Dismissed once per
 * browser: a per-viewer convenience with nothing at stake, so `localStorage` is the right place.
 *
 * A short chip with the instruction one hover, focus or tap away. Spelled out in full, the
 * sentence was too long for the header and got squeezed into a tall column beside the wallet.
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
    <span className="group relative hidden items-center gap-1.5 whitespace-nowrap rounded-md border border-line px-2 py-1 text-[11px] text-ink-muted xl:flex">
      <span
        tabIndex={0}
        aria-describedby="devnet-hint"
        className="flex cursor-help items-center gap-1.5 outline-none"
      >
        <span className="h-1.5 w-1.5 rounded-full bg-warn" />
        <span className="font-semibold text-ink">Devnet only</span>
      </span>
      <button
        onClick={dismiss}
        className="text-ink-dim hover:text-ink"
        aria-label="Dismiss the devnet hint"
      >
        ×
      </button>
      <span
        id="devnet-hint"
        role="tooltip"
        className="invisible absolute right-0 top-full z-50 mt-2 w-64 whitespace-normal rounded-md border border-line bg-surface p-3 text-xs leading-relaxed text-ink-muted opacity-0 shadow-xl transition group-focus-within:visible group-focus-within:opacity-100 group-hover:visible group-hover:opacity-100"
      >
        Switch Phantom to Devnet in Settings → Developer Settings. On Mainnet,
        none of these accounts or test tokens exist.
      </span>
    </span>
  );
}
