import { useCallback, useEffect, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import type { Address } from "@solana/kit";

import { readAccountStatus, type AccountStatus } from "@/lib/account";
import { useRpc } from "@/hooks/useSolfx";

export type AccountState = {
  readonly status: AccountStatus | undefined;
  readonly owner: Address | undefined;
  readonly loading: boolean;
  readonly error: string | undefined;
  readonly refresh: () => void;
};

/**
 * Reads the connected wallet's protocol state. Undefined until a wallet is connected.
 *
 * Polled for the same reason `usePositions` is: a keeper-executed take-profit, stop-loss or
 * liquidation returns collateral without this tab doing anything, so free collateral would
 * otherwise sit at a stale figure until the trader acted. Slower than the position poll —
 * this costs two reads, and a wrong balance is less alarming than a phantom position.
 */
export function useAccount(pollMs = 20_000): AccountState {
  const rpc = useRpc();
  const { wallet } = useWalletConnection();
  const owner = wallet?.account.address as Address | undefined;

  const [status, setStatus] = useState<AccountStatus | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    if (!owner) {
      setStatus(undefined);
      setError(undefined);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const s = await readAccountStatus(rpc, owner);
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

  useEffect(() => {
    if (!owner) return;
    const id = setInterval(refresh, pollMs);
    return () => clearInterval(id);
  }, [owner, pollMs, refresh]);

  return { status, owner, loading, error, refresh };
}
