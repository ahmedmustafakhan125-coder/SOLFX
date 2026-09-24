import { Link, useLocation } from "react-router-dom";

import { Wordmark } from "@/components/Logo";
import { ProductSwitcher } from "@/components/ProductSwitcher";
import { WalletButton } from "@/components/WalletButton";

const SPEC =
  "https://github.com/ahmedmustafakhan125-coder/SOLFX/blob/main/docs/NOXFUNDS.md";

/**
 * The tabs, mirrored from SolFX's header: where SolFX links across to NOXFUNDS, NOXFUNDS links
 * back to SolFX, and both reach About. Same order of importance, same style, so moving between
 * the two products changes the colour and nothing else.
 */
const ROUTES = [
  { to: "/nox", label: "Overview" },
  { to: "/nox/market", label: "Marketplace" },
  { to: "/nox/investor", label: "Investor" },
  { to: "/nox/trader", label: "Trader" },
  { to: "/nox/verify", label: "Verify" },
  { to: "/", label: "SolFX" },
  { to: "/about", label: "About" },
] as const;

/** SolFX's tab classes, verbatim — active is bold and underlined, the rest muted. */
const TAB = "whitespace-nowrap uppercase tracking-wider";
const TAB_ACTIVE = `${TAB} border-b-2 border-brand-soft pb-1 font-bold text-brand-soft`;
const TAB_IDLE = `${TAB} text-ink-muted transition-colors hover:text-ink`;

/**
 * The NOXFUNDS frame: backdrop, product switcher, nav, footer.
 *
 * Shared by every NOXFUNDS page so they cannot drift from one another, and it is what sets
 * `data-product="nox"` — the one attribute that turns SolFX's purple into NOXFUNDS' cyan for
 * everything inside, grid and glow included.
 *
 * `anchors` are in-page links for the page that passes them; routes are the same everywhere.
 */
export function NoxShell({
  anchors = [],
  children,
}: {
  anchors?: readonly (readonly [string, string])[];
  children: React.ReactNode;
}) {
  const { pathname } = useLocation();

  return (
    <div data-product="nox" className="relative min-h-screen bg-bg text-ink">
      {/* The shared backdrop. Identical to SolFX's; only the accent hue differs. */}
      <div className="gridwork pointer-events-none fixed inset-0 z-0" />
      <div className="glowfield pointer-events-none fixed inset-0 z-0" />

      <div className="relative z-10">
        <ProductSwitcher active="nox" />

        <header className="sticky top-0 z-50 border-b border-line-soft bg-bg/95 backdrop-blur">
          <div className="mx-auto flex max-w-[1440px] items-center gap-8 px-4 py-4 md:px-8">
            <Link to="/nox">
              <Wordmark product="nox" />
            </Link>

            <nav className="hidden items-center gap-6 text-sm md:flex">
              {ROUTES.map(({ to, label }) => (
                <Link
                  key={to}
                  to={to}
                  aria-current={pathname === to ? "page" : undefined}
                  className={pathname === to ? TAB_ACTIVE : TAB_IDLE}
                >
                  {label}
                </Link>
              ))}
              {/*
               * In-page tabs only where there is room for them. Eight uppercase tabs, the wordmark
               * and the wallet button do not fit a medium screen, and a header that wraps is worse
               * than one that shows the four that matter.
               */}
              {anchors.map(([href, label]) => (
                <a
                  key={href}
                  href={href}
                  className={`${TAB_IDLE} hidden xl:inline`}
                >
                  {label}
                </a>
              ))}
            </nav>

            <div className="ml-auto flex items-center gap-3">
              <a
                href={SPEC}
                target="_blank"
                rel="noreferrer"
                className="hidden border border-line px-4 py-1.5 text-[11px] font-bold uppercase tracking-[0.14em] text-ink-muted transition-colors hover:border-brand hover:text-brand sm:block"
              >
                Spec
              </a>
              <WalletButton />
            </div>
          </div>
        </header>

        {children}

        <footer className="border-t border-line-soft px-4 py-12 md:px-8">
          <div className="mx-auto flex max-w-[1200px] flex-wrap items-center gap-x-10 gap-y-5">
            <Link to="/nox">
              <Wordmark product="nox" />
            </Link>
            <div className="flex flex-wrap gap-x-7 gap-y-2 text-[11px] uppercase tracking-[0.14em] text-ink-dim">
              <Link to="/" className="hover:text-brand">
                SolFX
              </Link>
              <Link to="/trade" className="hover:text-brand">
                Terminal
              </Link>
              <Link to="/nox/market" className="hover:text-brand">
                Marketplace
              </Link>
              <Link to="/about" className="hover:text-brand">
                About
              </Link>
              <a
                href={SPEC}
                target="_blank"
                rel="noreferrer"
                className="hover:text-brand"
              >
                Specification
              </a>
            </div>
            <span className="ml-auto text-[11px] uppercase tracking-[0.14em] text-ink-dim">
              Devnet · test USDC · not audited
            </span>
          </div>
        </footer>
      </div>
    </div>
  );
}
