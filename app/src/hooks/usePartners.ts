import { useCallback, useEffect, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import { readPartnersStatus, type PartnersStatus } from "@/lib/partners";
import { useRpc } from "@/hooks/useSolfx";

export type PartnersState = {
  readonly status: PartnersStatus | undefined;
  readonly owner: Address | undefined;
  readonly loading: boolean;
  readonly error: string | undefined;
  readonly refresh: () => void;
};

/**
 * The referral pool and, when a wallet is connected, that wallet's standing on both sides
 * of it.
 *
 * Loads without a wallet: the pool's accrual is public, and an IB deciding whether to join
 * should be able to see what the programme has actually paid before connecting anything.
 */
export function usePartners(): PartnersState {
  const rpc = useRpc();
  const { wallet } = useWalletConnection();
  const owner = wallet?.account.address as Address | undefined;

  const [status, setStatus] = useState<PartnersStatus | undefined>(undefined);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const s = await readPartnersStatus(rpc, owner ?? null);
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
