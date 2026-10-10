import { useEffect, useId, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";

/** One destination: a route on this site (optionally with a `#section`), or an external page. */
export type NavLink = {
  readonly label: string;
  /** A route, optionally with a `#section`. */
  readonly to?: string;
  /** An external page, opened in a new tab. */
  readonly href?: string;
  /** One line under the label, shown inside a section's menu. */
  readonly note?: string;
};

/** A top-level entry: a plain link, or a section whose links open beneath it. */
export type NavEntry =
  NavLink | { readonly label: string; readonly items: readonly NavLink[] };

const TAB = "whitespace-nowrap border-b-2 pb-1 uppercase tracking-wider";
const TAB_ACTIVE = `${TAB} border-brand-soft font-bold text-brand-soft`;
const TAB_IDLE = `${TAB} border-transparent text-ink-muted transition-colors hover:text-ink`;
const ITEM =
  "flex flex-col gap-0.5 rounded-md px-3 py-2 outline-none transition-colors hover:bg-surface-highest focus-visible:bg-surface-highest";

/** How long a section stays open after the mouse leaves it, so a diagonal move does not drop it. */
const CLOSE_DELAY_MS = 150;

const routeOf = (to: string) => to.split("#")[0] || "/";

/** A link is "here" only as a page, not as a section of one: `/#scope` does not light up. */
const isHere = (link: NavLink, pathname: string) =>
  link.to !== undefined && !link.to.includes("#") && link.to === pathname;

/** A link inside a section's menu: its label, and a line saying what is there. */
function MenuItem({ link, onPick }: { link: NavLink; onPick: () => void }) {
  const { pathname } = useLocation();
  const here = isHere(link, pathname);
  const body = (
    <>
      <span
        className={`text-[13px] font-semibold ${here ? "text-brand-soft" : "text-ink"}`}
      >
        {link.label}
        {link.href ? (
          <span aria-hidden className="ml-1 text-ink-dim">
            ↗
          </span>
        ) : null}
      </span>
      {link.note ? (
        <span className="text-xs leading-snug text-ink-dim">{link.note}</span>
      ) : null}
    </>
  );
  return link.href ? (
    <a
      href={link.href}
      target="_blank"
      rel="noreferrer"
      className={ITEM}
      onClick={onPick}
    >
      {body}
    </a>
  ) : (
    <Link
      to={link.to ?? "/"}
      aria-current={here ? "page" : undefined}
      className={ITEM}
      onClick={onPick}
    >
      {body}
    </Link>
  );
}

/**
 * The header's navigation, as a few sections that each open into their own links.
 *
 * Built because a flat row stopped fitting: NOXFUNDS had eleven tabs, and the header wrapped and
 * crushed the devnet hint into a column. A section opens on hover for a mouse, and on click, tap
 * or Enter otherwise. Escape, a click elsewhere, tabbing out or navigating closes it. A section is
 * marked current when any of its pages is the one you are on.
 *
 * It also scrolls to `#section` targets. The browser does that for a plain anchor on the same
 * page, but not when React Router moves between routes, so `/nox#rules` from another page would
 * otherwise land at the top.
 */
export function NavMenu({
  entries,
  className = "hidden items-center gap-6 text-sm md:flex",
}: {
  entries: readonly NavEntry[];
  className?: string;
}) {
  const { pathname, hash, key } = useLocation();
  const [open, setOpen] = useState<number | null>(null);
  const nav = useRef<HTMLElement | null>(null);
  const closeTimer = useRef<number | undefined>(undefined);
  const id = useId();

  // Every navigation closes the menu, and lands on its #section when it names one.
  useEffect(() => {
    setOpen(null);
    if (!hash) return;
    const target = document.getElementById(decodeURIComponent(hash.slice(1)));
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    target?.scrollIntoView({
      behavior: still ? "auto" : "smooth",
      block: "start",
    });
  }, [pathname, hash, key]);

  // A click anywhere outside the navigation closes an open section.
  useEffect(() => {
    if (open === null) return;
    const onDown = (e: PointerEvent) => {
      if (!nav.current?.contains(e.target as Node)) setOpen(null);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open]);

  useEffect(() => () => window.clearTimeout(closeTimer.current), []);

  const show = (i: number) => {
    window.clearTimeout(closeTimer.current);
    setOpen(i);
  };
  const hideSoon = () => {
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setOpen(null), CLOSE_DELAY_MS);
  };
  const close = () => setOpen(null);

  return (
    <nav ref={nav} className={className}>
      {entries.map((entry, i) => {
        if (!("items" in entry)) {
          const here = isHere(entry, pathname);
          return entry.href ? (
            <a
              key={entry.label}
              href={entry.href}
              target="_blank"
              rel="noreferrer"
              className={TAB_IDLE}
            >
              {entry.label}
            </a>
          ) : (
            <Link
              key={entry.label}
              to={entry.to ?? "/"}
              aria-current={here ? "page" : undefined}
              className={here ? TAB_ACTIVE : TAB_IDLE}
            >
              {entry.label}
            </Link>
          );
        }

        const current = entry.items.some(
          (l) => l.to !== undefined && routeOf(l.to) === pathname
        );
        const isOpen = open === i;
        const panel = `${id}-${i}`;
        return (
          <div
            key={entry.label}
            className="relative"
            onPointerEnter={(e) => e.pointerType === "mouse" && show(i)}
            onPointerLeave={(e) => e.pointerType === "mouse" && hideSoon()}
            onKeyDown={(e) => {
              if (e.key !== "Escape" || !isOpen) return;
              close();
              e.currentTarget.querySelector("button")?.focus();
            }}
            onBlur={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
                close();
              }
            }}
          >
            <button
              type="button"
              aria-expanded={isOpen}
              aria-controls={panel}
              onClick={(e) => {
                // A pointer opens it and leaves it open under the cursor; Enter or Space toggles.
                // `detail` is 0 only for a click synthesised from the keyboard.
                if (!isOpen) show(i);
                else if (e.detail === 0) close();
              }}
              className={`flex items-center gap-1.5 ${current ? TAB_ACTIVE : TAB_IDLE}`}
            >
              {entry.label}
              <svg
                viewBox="0 0 10 6"
                aria-hidden
                className={`h-1.5 w-2.5 transition-transform ${isOpen ? "rotate-180" : ""}`}
              >
                <path
                  d="M1 1l4 4 4-4"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                />
              </svg>
            </button>

            {/* Padding rather than margin above the panel, so there is no gap to fall through. */}
            <div
              id={panel}
              className={`absolute left-0 top-full z-50 pt-3 transition duration-150 ${
                isOpen
                  ? "visible translate-y-0 opacity-100"
                  : "invisible -translate-y-1 opacity-0"
              }`}
            >
              <ul className="min-w-[260px] rounded-lg border border-line bg-surface/95 p-1.5 shadow-2xl shadow-black/60 backdrop-blur-md">
                {entry.items.map((link) => (
                  <li key={link.label}>
                    <MenuItem link={link} onPick={close} />
                  </li>
                ))}
              </ul>
            </div>
          </div>
        );
      })}
    </nav>
  );
}
