import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { useWalletConnection } from "@solana/react-hooks";

const NAV = [
  { to: "/trade", label: "Trade" },
  { to: "/pool", label: "Pool" },
  { to: "/partners", label: "Partners" },
  { to: "/about", label: "About" },
  { to: "/", label: "Home" },
] as const;

function short(a: string) {
  return `${a.slice(0, 4)}…${a.slice(-4)}`;
}

export function Header({ rpcLabel }: { rpcLabel: string }) {
  const { connectors, connect, disconnect, wallet, status } =
    useWalletConnection();
  // The active tab was hardcoded to Trade, which was true while there was one destination
  // and quietly wrong the moment there were two.
  const { pathname } = useLocation();
  const [open, setOpen] = useState(false);
  const address = wallet?.account.address.toString();

  return (
    <header className="flex items-center gap-6 border-b border-line-soft px-5 py-3">
      <Link to="/" className="flex items-baseline gap-2">
        <span className="text-lg font-extrabold tracking-tight">SolFX</span>
        <span className="text-[10px] uppercase tracking-[0.2em] text-ink-dim">
          Pro Terminal
        </span>
      </Link>

      <nav className="hidden gap-5 text-sm text-ink-muted md:flex">
        {NAV.map(({ to, label }) =>
          pathname === to ? (
            <span
              key={to}
              className="border-b-2 border-brand pb-0.5 font-medium text-ink"
            >
              {label}
            </span>
          ) : (
            <Link key={to} to={to} className="hover:text-ink">
              {label}
            </Link>
          )
        )}
      </nav>

      <div className="ml-auto flex items-center gap-3">
        <span className="hidden items-center gap-1.5 text-xs text-ink-dim sm:flex">
          <span className="h-1.5 w-1.5 rounded-full bg-long" />
          {rpcLabel}
        </span>

        {address ? (
          <button
            onClick={() => void disconnect()}
            className="tnum rounded-md border border-line bg-surface-high px-3 py-1.5 text-xs text-ink hover:border-brand"
            title={`${address} — click to disconnect`}
          >
            {short(address)}
          </button>
        ) : (
          <div className="relative">
            <button
              onClick={() => setOpen((o) => !o)}
              disabled={status === "connecting" || connectors.length === 0}
              className="rounded-md bg-brand px-4 py-1.5 text-xs font-semibold text-white hover:bg-brand-dim disabled:opacity-50"
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
        )}
      </div>
    </header>
  );
}
