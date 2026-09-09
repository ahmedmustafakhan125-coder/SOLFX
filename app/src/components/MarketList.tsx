import { useEffect, useMemo, useState } from "react";

import type { LoadedMarket } from "@/lib/markets";
import type { LivePrice } from "@/lib/prices";
import {
  loadFavourites,
  matchesQuery,
  orderByFavourite,
  saveFavourites,
  toggleFavourite,
} from "@/lib/favourites";
import { fmtPrice, priceplaces } from "@/lib/format";

type Props = {
  markets: LoadedMarket[];
  prices: Record<string, LivePrice | undefined>;
  selected: number | undefined;
  onSelect: (index: number) => void;
};

/** A star, filled when the market is a favourite. Inline so it inherits `currentColor`. */
function Star({ filled }: { filled: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-3.5 w-3.5"
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={filled ? 0 : 1.6}
      aria-hidden="true"
    >
      <path d="M12 2.5l2.9 5.9 6.6.9-4.8 4.6 1.2 6.6L12 17.4l-5.9 3.1 1.2-6.6L2.5 9.3l6.6-.9z" />
    </svg>
  );
}

export function MarketList({ markets, prices, selected, onSelect }: Props) {
  const [query, setQuery] = useState("");
  const [favourites, setFavourites] = useState<ReadonlySet<string>>(
    () => new Set<string>()
  );

  // Read after mount rather than in the initialiser: `localStorage` is unavailable during a
  // server render and can throw outright in a blocked context, and neither is worth taking
  // the whole list down for.
  useEffect(() => setFavourites(loadFavourites()), []);

  function star(symbol: string) {
    const next = toggleFavourite(favourites, symbol);
    setFavourites(next);
    saveFavourites(next);
  }

  const tradeable = useMemo(
    () =>
      orderByFavourite(
        markets.filter((m) => m.tradeable && matchesQuery(m, query)),
        favourites
      ),
    [markets, query, favourites]
  );
  const retired = useMemo(
    () => markets.filter((m) => !m.tradeable && matchesQuery(m, query)),
    [markets, query]
  );

  const liveTotal = markets.filter((m) => m.tradeable).length;
  const filtering = query.trim() !== "";

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-line-soft">
      <div className="flex items-baseline justify-between px-4 pt-3 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
        <span>Markets · {liveTotal} live</span>
        {/* The live count stays the truth about the venue; the filter reports itself
            separately, so a search can never make the venue look smaller than it is. */}
        {filtering ? (
          <span className="text-ink-muted">{tradeable.length} shown</span>
        ) : null}
      </div>

      <div className="px-4 py-2">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search markets"
          aria-label="Search markets"
          className="w-full rounded-md border border-line bg-surface px-2.5 py-1.5 text-xs outline-none placeholder:text-ink-muted focus:border-brand"
        />
      </div>

      <div className="flex-1 overflow-y-auto">
        {tradeable.length === 0 && retired.length === 0 ? (
          <div className="px-4 py-3 text-xs text-ink-dim">
            Nothing matches “{query.trim()}”.
          </div>
        ) : null}

        {tradeable.map((m) => {
          const p = prices[m.feedIdHex];
          const active = m.index === selected;
          const starred = favourites.has(m.symbol);
          return (
            <div
              key={m.index}
              className={`group flex items-center border-l-2 transition-colors ${
                active
                  ? "border-brand bg-surface-high"
                  : "border-transparent hover:bg-surface-high/60"
              }`}
            >
              <button
                onClick={() => onSelect(m.index)}
                className="min-w-0 flex-1 px-4 py-2.5 text-left"
              >
                <div className="flex items-center gap-1.5">
                  <span className="text-sm font-semibold">{m.symbol}</span>
                  <span className="tnum rounded border border-line px-1 text-[9px] text-ink-dim">
                    {m.maxLeverage}x
                  </span>
                </div>
                <div className="tnum text-xs text-ink-muted">
                  {p ? fmtPrice(p.price, priceplaces(m.symbol)) : "—"}
                </div>
              </button>

              {p?.stale ? (
                <span
                  className="rounded bg-warn/15 px-1 py-0.5 text-[9px] font-medium text-warn"
                  title={`Price is ${p.ageSeconds}s old; the program rejects anything over 60s`}
                >
                  STALE
                </span>
              ) : null}

              <button
                onClick={() => star(m.symbol)}
                title={starred ? "Remove from favourites" : "Add to favourites"}
                aria-label={
                  starred
                    ? `Remove ${m.symbol} from favourites`
                    : `Add ${m.symbol} to favourites`
                }
                aria-pressed={starred}
                className={`px-3 py-2.5 transition-opacity ${
                  starred
                    ? "text-brand"
                    : "text-ink-muted opacity-0 hover:text-ink group-hover:opacity-100 focus:opacity-100"
                }`}
              >
                <Star filled={starred} />
              </button>
            </div>
          );
        })}

        {retired.length > 0 ? (
          <>
            <div className="px-4 pt-5 pb-2 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
              Not trading
            </div>
            {retired.map((m) => (
              <div
                key={m.index}
                className="flex items-center justify-between px-4 py-2 text-xs text-ink-dim"
                title={`Market ${m.index} is ${m.status}`}
              >
                <span>{m.symbol}</span>
                <span className="text-[10px] uppercase">{m.status}</span>
              </div>
            ))}
          </>
        ) : null}
      </div>
    </aside>
  );
}
