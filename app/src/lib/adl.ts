/**
 * The auto-deleveraging waterfall, as a trader can see it.
 *
 * `Market.pending_adl_debt` is a shortfall a liquidation could not cover and that ADL has not
 * yet drawn down; `InsuranceFund.balance` is what stands between a shortfall and a trader's
 * profit. Both are plain public state — this reads them and nothing else, because there is no
 * ADL action a trader can take.
 */
import { fetchInsuranceFund, findInsuranceFundPda } from "@solfx/client";
import type { Rpc, SolanaRpcApi } from "@solana/kit";

import type { LoadedMarket } from "@/lib/markets";

export type InsuranceState = {
  readonly balance: bigint;
  readonly targetBalance: bigint;
  /** Bad debt the fund has absorbed over its life — the number that says it has been used. */
  readonly totalBadDebtCovered: bigint;
};

export async function readInsuranceFund(
  rpc: Rpc<SolanaRpcApi>
): Promise<InsuranceState> {
  const [pda] = await findInsuranceFundPda();
  const f = await fetchInsuranceFund(rpc, pda);
  return {
    balance: f.data.balance,
    targetBalance: f.data.targetBalance,
    totalBadDebtCovered: f.data.totalBadDebtCovered,
  };
}

/** Markets carrying an outstanding shortfall, worst first. Empty is the normal state. */
export function marketsWithPendingDebt(
  markets: readonly LoadedMarket[]
): LoadedMarket[] {
  return markets
    .filter((m) => m.data.pendingAdlDebt > 0n)
    .sort((a, b) => (b.data.pendingAdlDebt > a.data.pendingAdlDebt ? 1 : -1));
}

/** Total shortfall awaiting deleveraging across every market. */
export function totalPendingDebt(markets: readonly LoadedMarket[]): bigint {
  return markets.reduce((sum, m) => sum + m.data.pendingAdlDebt, 0n);
}
