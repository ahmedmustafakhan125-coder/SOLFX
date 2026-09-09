import { afterEach, describe, expect, it, vi } from "vitest";

import type { LoadedMarket } from "@/lib/markets";
import {
  loadFavourites,
  matchesQuery,
  orderByFavourite,
  saveFavourites,
  toggleFavourite,
} from "@/lib/favourites";

/** Only the fields these functions read. The rest of `LoadedMarket` is irrelevant here. */
function market(index: number, symbol: string): LoadedMarket {
  return { index, symbol } as LoadedMarket;
}

const MARKETS = [
  market(0, "EUR/USD"),
  market(1, "EUR/JPY"),
  market(2, "USD/INR"),
  market(3, "XAU/USD"),
  market(5, "BTC/USD"),
  market(8, "XAG/USD"),
];

describe("matchesQuery", () => {
  it("matches everything on an empty query", () => {
    expect(MARKETS.filter((m) => matchesQuery(m, ""))).toHaveLength(
      MARKETS.length
    );
    expect(MARKETS.filter((m) => matchesQuery(m, "   "))).toHaveLength(
      MARKETS.length
    );
  });

  it("ignores case and the slash, because traders type the pair as one word", () => {
    expect(matchesQuery(market(0, "EUR/USD"), "eurusd")).toBe(true);
    expect(matchesQuery(market(0, "EUR/USD"), "EUR/USD")).toBe(true);
    expect(matchesQuery(market(0, "EUR/USD"), "eur usd")).toBe(true);
    expect(matchesQuery(market(0, "EUR/USD"), "EurUsd")).toBe(true);
  });

  it("matches on either side of the pair", () => {
    const jpy = MARKETS.filter((m) => matchesQuery(m, "jpy"));
    expect(jpy.map((m) => m.symbol)).toStrictEqual(["EUR/JPY"]);

    const usd = MARKETS.filter((m) => matchesQuery(m, "usd"));
    expect(usd.map((m) => m.symbol)).toStrictEqual([
      "EUR/USD",
      "USD/INR",
      "XAU/USD",
      "BTC/USD",
      "XAG/USD",
    ]);
  });

  it("does not match a pair that merely shares letters out of order", () => {
    // "usdeur" is not a substring of "eurusd", and inventing a fuzzy match here would put
    // the wrong instrument under the cursor.
    expect(matchesQuery(market(0, "EUR/USD"), "usdeur")).toBe(false);
    expect(matchesQuery(market(0, "EUR/USD"), "gbp")).toBe(false);
  });

  it("distinguishes the metals, which differ by one letter", () => {
    expect(matchesQuery(market(3, "XAU/USD"), "xau")).toBe(true);
    expect(matchesQuery(market(3, "XAU/USD"), "xag")).toBe(false);
  });
});

describe("orderByFavourite", () => {
  it("puts favourites first and keeps listing order inside each group", () => {
    const favourites = new Set(["BTC/USD", "EUR/USD"]);
    const ordered = orderByFavourite(MARKETS, favourites);
    expect(ordered.map((m) => m.symbol)).toStrictEqual([
      // Both starred, in the order they are listed — not the order they were starred.
      "EUR/USD",
      "BTC/USD",
      "EUR/JPY",
      "USD/INR",
      "XAU/USD",
      "XAG/USD",
    ]);
  });

  it("leaves the list untouched when nothing is starred", () => {
    expect(
      orderByFavourite(MARKETS, new Set()).map((m) => m.symbol)
    ).toStrictEqual(MARKETS.map((m) => m.symbol));
  });

  it("ignores a favourite for a market that is no longer listed", () => {
    // A symbol can be delisted while a star for it survives in storage.
    const ordered = orderByFavourite(MARKETS, new Set(["GBP/JPY"]));
    expect(ordered).toHaveLength(MARKETS.length);
    expect(ordered[0]!.symbol).toBe("EUR/USD");
  });

  it("does not mutate the array it was given", () => {
    const before = MARKETS.map((m) => m.symbol);
    orderByFavourite(MARKETS, new Set(["XAG/USD"]));
    expect(MARKETS.map((m) => m.symbol)).toStrictEqual(before);
  });
});

describe("toggleFavourite", () => {
  it("adds, removes, and never mutates the set it was given", () => {
    const empty: ReadonlySet<string> = new Set();
    const one = toggleFavourite(empty, "BTC/USD");
    expect([...one]).toStrictEqual(["BTC/USD"]);
    expect(empty.size).toBe(0);

    const none = toggleFavourite(one, "BTC/USD");
    expect(none.size).toBe(0);
    expect(one.size).toBe(1);
  });
});

/**
 * Storage can throw rather than return nothing — a private window, blocked site data, or a
 * thumbnail context. A market list that fails to render because it could not remember a star
 * is worse than one that forgets, so both accessors swallow and carry on.
 */
describe("storage is never allowed to break the list", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("returns no favourites when localStorage throws", () => {
    vi.stubGlobal("localStorage", {
      getItem() {
        throw new Error("SecurityError: storage is blocked");
      },
      setItem() {
        throw new Error("SecurityError: storage is blocked");
      },
    });
    expect(loadFavourites().size).toBe(0);
    expect(() => saveFavourites(new Set(["BTC/USD"]))).not.toThrow();
  });

  it("survives stored rubbish rather than throwing on JSON.parse", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => "{not json",
      setItem: () => undefined,
    });
    expect(loadFavourites().size).toBe(0);
  });

  it("ignores a stored value of the wrong shape", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => JSON.stringify({ btc: true }),
      setItem: () => undefined,
    });
    expect(loadFavourites().size).toBe(0);
  });

  it("drops non-string entries but keeps the rest", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => JSON.stringify(["BTC/USD", 42, null, "XAU/USD"]),
      setItem: () => undefined,
    });
    expect([...loadFavourites()]).toStrictEqual(["BTC/USD", "XAU/USD"]);
  });

  it("round-trips through a working store", () => {
    let held: string | null = null;
    vi.stubGlobal("localStorage", {
      getItem: () => held,
      setItem: (_k: string, v: string) => {
        held = v;
      },
    });
    saveFavourites(new Set(["EUR/USD", "BTC/USD"]));
    expect([...loadFavourites()]).toStrictEqual(["EUR/USD", "BTC/USD"]);
  });
});
