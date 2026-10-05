import { useCallback, useEffect, useRef, useState } from "react";
import { useSolanaClient } from "@solana/react-hooks";
import type { Address, Rpc, SolanaRpcApi } from "@solana/kit";
import type { PriceAccountMap } from "@solfx/client";

import { legFeeds, loadMarkets, type LoadedMarket } from "@/lib/markets";
import { loadPriceMap, readPrice, type LivePrice } from "@/lib/prices";

export function useRpc(): Rpc<SolanaRpcApi> {
  const client = useSolanaClient();
  // The client types rpc as a union over clusters; the app is cluster-agnostic and
  // only uses calls common to all of them.
  return client.runtime.rpc as unknown as Rpc<SolanaRpcApi>;
}

type State = {
  markets: LoadedMarket[];
  prices: Record<string, LivePrice | undefined>;
  /** feed id (hex) -> the account actually being published to. */
  priceAccounts: Record<string, Address | undefined>;
  loading: boolean;
  error: string | undefined;
};

/**
 * Loads the venue once, then re-reads prices on an interval.
 *
 * Prices are polled rather than subscribed because the staleness gate is what matters here:
 * a price is only tradeable for 60 seconds after publication, so the interesting question is
 * "how old is the account right now", and a poll answers it directly.
 */
/**
 * `pollMs` is a rate-limit budget, not a freshness preference.
 *
 * The browser, the price poster and the keeper all share one RPC endpoint, and a free tier
 * allows about ten calls a second between them. Polling harder does not make a price fresher
 * — the poster decides that — it just spends budget that a *transaction* needs, and the
 * symptom is an `HTTP error (429)` on close rather than anything a trader can act on. Eight
 * seconds is well inside the 60-second staleness gate and leaves room for the send path.
 */
export function useSolfx(pollMs = 8_000): State & { refresh: () => void } {
  const rpc = useRpc();
  const [state, setState] = useState<State>({
    markets: [],
    prices: {},
    priceAccounts: {},
    loading: true,
    error: undefined,
  });
  const mapRef = useRef<PriceAccountMap | undefined>(undefined);

  const readPrices = useCallback(
    async (markets: LoadedMarket[]) => {
      const map = mapRef.current;
      if (!map) return {};
      // Every feed a market is priced from, not only its primary: a synthetic market needs its
      // second leg and a market not quoted in USD its conversion feed. Read once each, however
      // many markets share one.
      const feeds = allFeeds(markets);
      const entries = await Promise.all(
        feeds.map(async (feed) => {
          const account = map.forFeed(feed);
          if (!account) return [feed, undefined] as const;
          try {
            return [feed, await readPrice(rpc, account)] as const;
          } catch {
            return [feed, undefined] as const;
          }
        })
      );
      return Object.fromEntries(entries);
    },
    [rpc]
  );

  const refresh = useCallback(() => {
    let cancelled = false;
    void (async () => {
      try {
        if (!mapRef.current) {
          // A missing map is not fatal. It is gitignored and cluster-specific, so a fresh
          // checkout has none — markets still read fine, they just have no price until the
          // poster has run and the file is copied into public/.
          try {
            mapRef.current = await loadPriceMap();
          } catch {
            mapRef.current = undefined;
          }
        }
        const markets = await loadMarkets(rpc);
        const prices = await readPrices(markets);
        const map = mapRef.current;
        const priceAccounts = Object.fromEntries(
          allFeeds(markets).map((f) => [f, map?.forFeed(f)])
        );
        if (!cancelled) {
          setState({
            markets,
            prices,
            priceAccounts,
            loading: false,
            error: undefined,
          });
        }
      } catch (e) {
        if (!cancelled) {
          setState((s) => ({
            ...s,
            loading: false,
            error: e instanceof Error ? e.message : String(e),
          }));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [rpc, readPrices]);

  useEffect(refresh, [refresh]);

  // Only prices are re-read on the interval; market configuration changes by admin
  // transaction, not by the second, and re-reading nine accounts every tick is wasteful.
  useEffect(() => {
    const id = setInterval(() => {
      void (async () => {
        setState((s) => {
          if (s.markets.length === 0) return s;
          void readPrices(s.markets).then((prices) =>
            setState((cur) => ({ ...cur, prices }))
          );
          return s;
        });
      })();
    }, pollMs);
    return () => clearInterval(id);
  }, [pollMs, readPrices]);

  return { ...state, refresh: () => void refresh() };
}

/** Every feed id any market is priced from, each once. */
function allFeeds(markets: readonly LoadedMarket[]): string[] {
  const out = new Set<string>();
  for (const m of markets) {
    const l = legFeeds(m.data);
    out.add(l.primary);
    if (l.secondary) out.add(l.secondary);
    if (l.conversion) out.add(l.conversion);
  }
  return [...out];
}
