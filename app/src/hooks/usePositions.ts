import { useCallback, useEffect, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import { loadPositions, type OpenPosition } from "@/lib/positions";
import { useRpc } from "@/hooks/useSolfx";

export function usePositions(
  marketIndexes: readonly number[],
  prices: Record<number, bigint | undefined>,
) {
  const rpc = useRpc();
  const { wallet } = useWalletConnection();
  const owner = wallet?.account.address as Address | undefined;

  const [positions, setPositions] = useState<OpenPosition[]>([]);
  const [loading, setLoading] = useState(false);
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
        if (!cancelled) setPositions(p);
      } catch {
        if (!cancelled) setPositions([]);
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

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { positions, loading, refresh };
}
