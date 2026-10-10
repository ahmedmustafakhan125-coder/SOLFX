import { Link } from "react-router-dom";

import { GlassHeader } from "@/components/GlassHeader";
import { KineticGrid } from "@/components/KineticGrid";
import { Wordmark } from "@/components/Logo";
import { NavMenu, type NavEntry } from "@/components/NavMenu";
import { ProductSwitcher } from "@/components/ProductSwitcher";
import { DevnetHint } from "@/components/DevnetHint";
import { FaucetButton } from "@/components/FaucetButton";
import { WalletButton } from "@/components/WalletButton";

const SPEC =
  "https://github.com/ahmedmustafakhan125-coder/SOLFX/blob/main/docs/NOXFUNDS.md";

/**
 * The header, mirrored from SolFX's: an Overview section of the landing page's own parts, the
 * product's working pages as a second section, then the way across to SolFX and to About. Same
 * shape on both products, so moving between them changes the colour and nothing else.
 */
const NAV: readonly NavEntry[] = [
  {
    label: "Overview",
    items: [
      { to: "/nox", label: "Overview", note: "NOXFUNDS on one page" },
      {
        to: "/nox#how",
        label: "How it works",
        note: "One investor, one trader, one rule set",
      },
      {
        to: "/nox#rules",
        label: "Rules",
        note: "Ten rules, checked before the fill",
      },
      {
        to: "/nox#money",
        label: "Economics",
        note: "5% to the protocol, then 70/30",
      },
      {
        to: "/nox#proof",
        label: "Proof",
        note: "Four mandates settled on devnet",
      },
      {
        href: SPEC,
        label: "Specification",
        note: "The full design, on GitHub",
      },
    ],
  },
  {
    label: "Marketplace",
    items: [
      {
        to: "/nox/market",
        label: "Marketplace",
        note: "Find each other, agree terms, escrow the rest",
      },
      {
        to: "/nox/investor",
        label: "Investor",
        note: "Your offers and the mandates you fund",
      },
      {
        to: "/nox/trader",
        label: "Trader",
        note: "Your listing, your offers, your record",
      },
    ],
  },
  { to: "/nox/verify", label: "Verify" },
  { to: "/", label: "SolFX" },
  { to: "/about", label: "About" },
];

/**
 * The NOXFUNDS frame: backdrop, product switcher, nav, footer.
 *
 * Shared by every NOXFUNDS page so they cannot drift from one another, and it is what sets
 * `data-product="nox"`, the one attribute that turns SolFX's purple into NOXFUNDS' cyan for
 * everything inside, and the backdrop grid's purple into blue.
 */
export function NoxShell({ children }: { children: React.ReactNode }) {
  return (
    <div
      data-product="nox"
      className="has-grid relative isolate min-h-screen bg-bg text-ink"
    >
      {/* The shared backdrop. Identical to SolFX's; only the grid's hue differs. */}
      <KineticGrid />

      <div className="relative z-10">
        <ProductSwitcher active="nox" />

        <GlassHeader className="sticky top-0 z-50">
          <div className="mx-auto flex max-w-[1440px] items-center gap-8 px-4 py-4 md:px-8">
            <Link to="/nox">
              <Wordmark product="nox" />
            </Link>

            {/*
             * From lg up. The long wordmark, five entries and the wallet do not fit a medium
             * screen, and a header that wraps is worse than one that waits for room.
             */}
            <NavMenu
              entries={NAV}
              className="hidden items-center gap-6 text-sm lg:flex"
            />

            <div className="ml-auto flex items-center gap-3">
              <DevnetHint />
              <FaucetButton />
              <WalletButton />
            </div>
          </div>
        </GlassHeader>

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
