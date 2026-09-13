import { useCallback, useEffect, useMemo, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import {
  loadPositions,
  repricePositions,
  type OpenPosition,
} from "@/lib/positions";
import { useRpc } from "@/hooks/useSolfx";

/**
 * `pollMs` exists because not every close is one the browser made.
 *
 * A take-profit, a stop-loss and a liquidation are all executed by the keeper. Nothing in
 * this tab initiates them, so `onClosed`/`onOpened` never fire for them and, without a poll,
 * the row for a position the keeper closed minutes ago stays on screen until the wallet or
 * the market set happens to change. That was read as "the take-profit did not work" when the
 * take-profit had in fact fired on chain — the worst kind of display bug, because it makes a
 * working venue look broken.
 *
 * Ten seconds against one `getMultipleAccounts` is cheap next to the eight-second price poll
 * that already runs.
 */
export function usePositions(
  marketIndexes: readonly number[],
  prices: Record<number, bigint | undefined>,
  pollMs = 10_000
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

  useEffect(() => {
    if (!owner || marketIndexes.length === 0) return;
    const id = setInterval(refresh, pollMs);
    return () => clearInterval(id);
  }, [owner, marketIndexes.length, pollMs, refresh]);

  return { positions: marked, loading, error, refresh };
}
