import { useState } from "react";
import type { Address } from "@solana/kit";

import { PositionsPanel } from "@/components/PositionsPanel";
import { HistoryPanel } from "@/components/HistoryPanel";
import { SummaryPanel } from "@/components/SummaryPanel";
import { useHistory } from "@/hooks/useHistory";
import { EMPTY_TOTALS } from "@/lib/history";
import type { LoadedMarket } from "@/lib/markets";
import type { OpenPosition } from "@/lib/positions";

type Tab = "positions" | "history" | "summary";

type Props = {
  positions: OpenPosition[];
  markets: LoadedMarket[];
  prices: Record<number, bigint | undefined>;
  priceAccounts: Record<number, Address | undefined>;
  positionsError?: string | undefined;
  onClosed: () => void;
};

/**
 * Everything about a trader's own activity, in one place: what is open, what has happened,
 * and what it all came to.
 *
 * Tabs rather than three panels stacked down the page, because this sits under the chart in
 * a fixed-height terminal — a second table below the first would push the chart off screen,
 * and the three are read one at a time anyway.
 */
export function ActivityPanel({
  positions,
  markets,
  prices,
  priceAccounts,
  positionsError,
  onClosed,
}: Props) {
  const [tab, setTab] = useState<Tab>("positions");

  // History is fetched the first time it is asked for and then kept. Latched rather than
  // passed `tab !== "positions"` directly, because that flips back to false on every return
  // to the positions tab and would re-read the whole log each time a trader switched back.
  const [wanted, setWanted] = useState(false);
  const { history, loading, error, refresh } = useHistory(wanted);

  function show(next: Tab) {
    setTab(next);
    if (next !== "positions") setWanted(true);
  }

  const totals = history?.totals ?? EMPTY_TOTALS;
  const truncated = history?.truncated ?? false;

  const tabs: { id: Tab; label: string }[] = [
    { id: "positions", label: `Positions (${positions.length})` },
    { id: "history", label: "History" },
    { id: "summary", label: "Summary" },
  ];

  return (
    <div className="border-t border-line-soft">
      <div className="flex items-center justify-between gap-3 px-5">
        <div className="flex">
          {tabs.map((t) => (
            <button
              key={t.id}
              onClick={() => show(t.id)}
              // The active tab is marked by a brand underline *and* by ink weight, so it is
              // never carried by colour alone.
              className={`-mb-px border-b-2 px-3 py-2.5 text-xs transition-colors ${
                tab === t.id
                  ? "border-brand font-medium text-ink"
                  : "border-transparent text-ink-dim hover:text-ink-muted"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>
        {tab === "positions" ? (
          <span className="hidden text-[10px] text-ink-dim sm:block">
            unrealised is marked at the oracle mid; a close fills after the
            spread
          </span>
        ) : (
          <button
            onClick={refresh}
            disabled={loading}
            className="rounded border border-line px-2.5 py-1 text-[11px] hover:border-brand disabled:opacity-40"
          >
            {loading ? "Reading…" : "Refresh"}
          </button>
        )}
      </div>

      {tab === "positions" ? (
        <PositionsPanel
          positions={positions}
          markets={markets}
          prices={prices}
          priceAccounts={priceAccounts}
          loadError={positionsError}
          onClosed={onClosed}
        />
      ) : tab === "history" ? (
        <HistoryPanel
          rows={history?.rows ?? []}
          markets={markets}
          truncated={truncated}
          loading={loading}
          loadError={error}
        />
      ) : (
        <SummaryPanel
          totals={totals}
          positions={positions}
          truncated={truncated}
          loading={loading}
          loadError={error}
        />
      )}
    </div>
  );
}
