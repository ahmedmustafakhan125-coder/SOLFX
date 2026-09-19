import { Link, useLocation } from "react-router-dom";

import { Logo } from "@/components/Logo";
import { WalletButton } from "@/components/WalletButton";

const NAV = [
  { to: "/trade", label: "Trade" },
  { to: "/pool", label: "Pool" },
  { to: "/partners", label: "Partners" },
  { to: "/nox", label: "Noxfunds" },
  { to: "/about", label: "About" },
  { to: "/", label: "Home" },
] as const;

export function Header({ rpcLabel }: { rpcLabel: string }) {
  // The active tab was hardcoded to Trade, which was true while there was one destination
  // and quietly wrong the moment there were two.
  const { pathname } = useLocation();

  return (
    <header className="flex items-center gap-6 border-b border-line-soft px-5 py-3">
      <Link to="/" className="flex items-center gap-2">
        <Logo className="h-6 w-6 shrink-0 text-brand" />
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

        <WalletButton />
      </div>
    </header>
  );
}
