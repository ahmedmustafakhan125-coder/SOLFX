import { useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";

function short(a: string) {
  return `${a.slice(0, 4)}…${a.slice(-4)}`;
}

/**
 * Connect, or show who is connected and disconnect.
 *
 * Lifted out of the terminal header so the NOXFUNDS marketplace uses the same button rather
 * than a second implementation that would drift. Colour comes from `bg-brand`, so it is purple
 * on SolFX and cyan on NOXFUNDS without a prop.
 */
export function WalletButton() {
  const { connectors, connect, disconnect, wallet, status } =
    useWalletConnection();
  const [open, setOpen] = useState(false);
  const address = wallet?.account.address.toString();

  if (address) {
    return (
      <button
        onClick={() => void disconnect()}
        className="tnum rounded-md border border-line bg-surface-high px-3 py-1.5 text-xs text-ink hover:border-brand"
        title={`${address}. Click to disconnect`}
      >
        {short(address)}
      </button>
    );
  }

  return (
    <div className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        disabled={status === "connecting" || connectors.length === 0}
        className="rounded-md bg-brand px-4 py-1.5 text-xs font-semibold text-[var(--sf-on-brand)] hover:bg-brand-dim disabled:opacity-50"
      >
        {connectors.length === 0
          ? "No wallet detected"
          : status === "connecting"
            ? "Connecting…"
            : "Connect Wallet"}
      </button>

      {open && connectors.length > 0 ? (
        <div className="absolute right-0 z-20 mt-2 w-52 overflow-hidden rounded-md border border-line bg-surface-high shadow-xl">
          {connectors.map((c) => (
            <button
              key={c.id}
              onClick={() => {
                setOpen(false);
                void connect(c.id);
              }}
              className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs hover:bg-surface-highest"
            >
              {c.icon ? (
                <img src={c.icon} alt="" className="h-4 w-4 rounded" />
              ) : (
                <span className="h-4 w-4 rounded bg-line" />
              )}
              {c.name}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
