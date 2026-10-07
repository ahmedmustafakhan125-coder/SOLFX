import { useEffect } from "react";

import { BALANCES_CHANGED } from "@/lib/faucet";

/**
 * Run `refresh` when something outside this hook's component moved the wallet's balances —
 * today, a faucet claim. Without it a panel polls on its own schedule and shows the old figure
 * for up to its whole interval right after the person watched the tokens arrive.
 */
export function useBalancesChanged(refresh: () => void): void {
  useEffect(() => {
    window.addEventListener(BALANCES_CHANGED, refresh);
    return () => window.removeEventListener(BALANCES_CHANGED, refresh);
  }, [refresh]);
}
