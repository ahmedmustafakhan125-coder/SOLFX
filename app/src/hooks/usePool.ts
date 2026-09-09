import { useCallback, useEffect, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import { readPoolStatus, type PoolStatus } from "@/lib/pool";
import { useRpc } from "@/hooks/useSolfx";

export type PoolState = {
  readonly status: PoolStatus | undefined;
  readonly owner: Address | undefined;
  readonly loading: boolean;
  readonly error: string | undefined;
  readonly refresh: () => void;
};

/**
 * The pool's state, and the connected wallet's position in it.
 *
 * Unlike `useAccount` this loads with no wallet: AUM, NAV per share and the fee schedule are
 * public, and a prospective LP should be able to see what they would be joining before they
 * connect anything.
 *
 * A read failure is reported as an error, never as an empty pool. That distinction cost this
 * project a day on the positions panel — "you have nothing" and "I could not find out" look
 * identical on screen and are opposite facts.
 */
export function usePool(): PoolState {
  const rpc = useRpc();
  const { wallet } = useWalletConnection();
  const owner = wallet?.account.address as Address | undefined;

  const [status, setStatus] = useState<PoolStatus | undefined>(undefined);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const s = await readPoolStatus(rpc, owner ?? null);
        if (!cancelled) {
          setStatus(s);
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
  }, [rpc, owner, nonce]);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { status, owner, loading, error, refresh };
}
