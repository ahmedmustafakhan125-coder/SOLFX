/**
 * Market search and favourites — the two things that stop a market list being useless at 33
 * markets.
 *
 * Both are per-viewer conveniences with nothing at stake, so favourites live in
 * `localStorage` rather than on chain. Every access is wrapped: a private window, cleared
 * site data, or a browser set to block storage makes the accessor itself throw, and a
 * market list that fails to render because it could not remember a star is worse than one
 * that forgets.
 *
 * The functions here are pure over their inputs so the ordering rules can be tested without
 * a DOM. Only [`loadFavourites`] and [`saveFavourites`] touch the browser.
 */
import type { LoadedMarket } from "@/lib/markets";

const STORAGE_KEY = "solfx.favourites";

/** Favourites, by market symbol rather than index — an index is a listing-order accident. */
export function loadFavourites(): ReadonlySet<string> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((s): s is string => typeof s === "string"));
  } catch {
    // Unreadable storage is indistinguishable from no favourites, and both are fine.
    return new Set();
  }
}

export function saveFavourites(favourites: ReadonlySet<string>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...favourites]));
  } catch {
    // Nothing to do and nothing worth telling the user: they starred a market, and it will
    // simply not be there next time.
  }
}

export function toggleFavourite(
  favourites: ReadonlySet<string>,
  symbol: string
): ReadonlySet<string> {
  const next = new Set(favourites);
  if (!next.delete(symbol)) next.add(symbol);
  return next;
}

/**
 * Does `market` match what was typed?
 *
 * Case-insensitive, and it ignores the slash so that `eurusd` finds `EUR/USD` — traders type
 * the pair as one word far more often than they reach for the separator. An empty query
 * matches everything rather than nothing.
 */
export function matchesQuery(market: LoadedMarket, query: string): boolean {
  const q = normalise(query);
  if (q === "") return true;
  return normalise(market.symbol).includes(q);
}

function normalise(s: string): string {
  return s.toLowerCase().replace(/[/\s]/g, "");
}

/**
 * Markets in display order: favourites first, then the rest, each group keeping its original
 * listing order.
 *
 * Stable within a group on purpose. Re-sorting the list every time a price ticks would move
 * the row a trader is reaching for, which is the kind of interface that causes the wrong
 * order to be sent.
 */
export function orderByFavourite(
  markets: readonly LoadedMarket[],
  favourites: ReadonlySet<string>
): LoadedMarket[] {
  const starred: LoadedMarket[] = [];
  const rest: LoadedMarket[] = [];
  for (const m of markets) {
    (favourites.has(m.symbol) ? starred : rest).push(m);
  }
  return [...starred, ...rest];
}
