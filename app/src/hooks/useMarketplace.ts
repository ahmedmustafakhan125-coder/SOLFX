import { useCallback, useEffect, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import { readMarketplace, type Marketplace } from "@/lib/nox";
import { loadMarkets, type LoadedMarket } from "@/lib/markets";
import { useRpc } from "@/hooks/useSolfx";

export type MarketplaceState = {
  readonly market: Marketplace | undefined;
  /** SolFX's markets, for naming the indices in a `u128` market bitmap. Loaded once. */
  readonly markets: readonly LoadedMarket[];
  readonly me: Address | undefined;
  readonly loading: boolean;
  readonly error: string | undefined;
  readonly refresh: () => void;
};

/**
 * The NOXFUNDS marketplace, re-read every 30 seconds and on demand.
 *
 * Thirty seconds, not eight like prices: this is one `getProgramAccounts` against an RPC the
 * price poster also depends on, and an inbox that is half a minute behind costs nobody money
 * where a stale price does. Every send calls `refresh`, so your own actions show at once.
 *
 * Loads without a wallet — the marketplace is public, and a trader deciding whether to list
 * should be able to read it before connecting anything.
 */
export function useMarketplace(pollMs = 30_000): MarketplaceState {
  const rpc = useRpc();
  const { wallet } = useWalletConnection();
  const me = wallet?.account.address as Address | undefined;

  const [market, setMarket] = useState<Marketplace | undefined>(undefined);
  const [markets, setMarkets] = useState<readonly LoadedMarket[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    void loadMarkets(rpc)
      .then((m) => {
        if (!cancelled) setMarkets(m);
      })
      .catch(() => {
        // Names are a nicety; the marketplace still works with bare indices.
      });
    return () => {
      cancelled = true;
    };
  }, [rpc]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const m = await readMarketplace(rpc);
        if (!cancelled) {
          setMarket(m);
          setError(undefined);
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [rpc, nonce]);

  useEffect(() => {
    const id = setInterval(() => setNonce((n) => n + 1), pollMs);
    return () => clearInterval(id);
  }, [pollMs]);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { market, markets, me, loading, error, refresh };
}
