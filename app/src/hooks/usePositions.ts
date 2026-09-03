import { useCallback, useEffect, useMemo, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import {
  loadPositions,
  repricePositions,
  type OpenPosition,
} from "@/lib/positions";
import { useRpc } from "@/hooks/useSolfx";

export function usePositions(
  marketIndexes: readonly number[],
  prices: Record<number, bigint | undefined>
) {
  const rpc = useRpc();
  const { wallet } = useWalletConnection();
  const owner = wallet?.account.address as Address | undefined;

  const [positions, setPositions] = useState<OpenPosition[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  const key = marketIndexes.join(",");

  useEffect(() => {
    if (!owner || marketIndexes.length === 0) {
      setPositions([]);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const p = await loadPositions(rpc, owner, marketIndexes, prices);
        if (!cancelled) {
          setPositions(p);
          setError(undefined);
        }
      } catch (e) {
        // Never swallow this. A caught-and-discarded failure here renders as "no open
        // positions", which is indistinguishable from the truth and sends a trader looking
        // for a position they were told does not exist. `useAccount` already reports its
        // errors; this one hid a real one until the chain was queried by hand.
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // `prices` changes every poll; positions are re-priced from it without re-reading chain
    // state, so it is deliberately not a dependency here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rpc, owner, key, nonce]);

  // Marked against the latest oracle on every poll. `loadPositions` only re-reads the chain
  // when the wallet or market set changes, so without this the unrealised column is frozen.
  const marked = useMemo(
    () => repricePositions(positions, prices),
    [positions, prices]
  );

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { positions: marked, loading, error, refresh };
}
