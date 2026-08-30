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

/** Reads the connected wallet's protocol state. Undefined until a wallet is connected. */
export function useAccount(): AccountState {
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
  return { status, owner, loading, error, refresh };
}
