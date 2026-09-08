import { useCallback, useEffect, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import { loadHistory, type History } from "@/lib/history";
import { useRpc } from "@/hooks/useSolfx";

/**
 * Trade history, fetched when it is actually looked at.
 *
 * `enabled` is the whole point of the signature. Reading history costs one
 * `getSignaturesForAddress` plus a `getTransaction` for every signature it returns, against
 * the same endpoint the poster and the keeper are using — see `pollMs` in `useSolfx` for why
 * that budget is tight. Positions are polled because they are what a trader watches move;
 * history does not move, so it loads when its tab is opened and then only on `refresh`.
 */
export function useHistory(enabled: boolean) {
  const rpc = useRpc();
  const { wallet } = useWalletConnection();
  const owner = wallet?.account.address as Address | undefined;

  const [history, setHistory] = useState<History | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    if (!enabled || !owner) return;
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const h = await loadHistory(rpc, owner);
        if (!cancelled) {
          setHistory(h);
          setError(undefined);
        }
      } catch (e) {
        // Same rule as `usePositions`: an empty history and an unreadable one look identical
        // on screen and only one of them is the truth. Say which this is.
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [rpc, owner, enabled, nonce]);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { history, loading, error, refresh };
}
