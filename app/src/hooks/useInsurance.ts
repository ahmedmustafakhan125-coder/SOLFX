import { useEffect, useState } from "react";

import { readInsuranceFund, type InsuranceState } from "@/lib/adl";
import { useRpc } from "@/hooks/useSolfx";

/**
 * The insurance fund, read once when the ADL tab is first opened.
 *
 * `wanted` latches in the caller for the same reason history does: re-reading on every
 * return to a tab spends the RPC budget the price poller needs.
 */
export function useInsurance(wanted: boolean) {
  const rpc = useRpc();
  const [fund, setFund] = useState<InsuranceState | undefined>(undefined);
  const [error, setError] = useState<string | undefined>(undefined);

  useEffect(() => {
    if (!wanted) return;
    let cancelled = false;
    void (async () => {
      try {
        const f = await readInsuranceFund(rpc);
        if (!cancelled) {
          setFund(f);
          setError(undefined);
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [rpc, wanted]);

  return { fund, error };
}
