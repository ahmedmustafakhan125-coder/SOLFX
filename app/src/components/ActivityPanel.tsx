import { useCallback, useEffect, useState } from "react";
import type { Address } from "@solana/kit";

import { FloatingPanel } from "@/components/FloatingPanel";
import { clampRect, defaultRect, type Rect } from "@/lib/floating";

import { PositionsPanel } from "@/components/PositionsPanel";
import { HistoryPanel } from "@/components/HistoryPanel";
import { SummaryPanel } from "@/components/SummaryPanel";
import { AdlPanel } from "@/components/AdlPanel";
import { useHistory } from "@/hooks/useHistory";
import { EMPTY_TOTALS } from "@/lib/history";
import type { LoadedMarket } from "@/lib/markets";
import type { OpenPosition } from "@/lib/positions";

type Tab = "positions" | "history" | "summary" | "adl";

/**
 * Where the panel was left, if it was floated.
 *
 * A per-viewer convenience with nothing at stake, so it lives in `localStorage` and every
 * access is wrapped — the same reasoning as `favourites.ts`. A browser that refuses storage
 * gets the docked default, which is the layout everyone had before this existed.
 */
const LAYOUT_KEY = "solfx.activity-layout.v1";

type Layout = { readonly floating: boolean; readonly rect?: Rect };

function loadLayout(): Layout {
  try {
    const raw = localStorage.getItem(LAYOUT_KEY);
    if (!raw) return { floating: false };
    const p: unknown = JSON.parse(raw);
    if (typeof p !== "object" || p === null) return { floating: false };
    const o = p as Record<string, unknown>;
    const r = o["rect"] as Record<string, unknown> | undefined;
    const nums = ["x", "y", "w", "h"].every(
      (k) => typeof r?.[k] === "number" && Number.isFinite(r[k])
    );
    return {
      floating: o["floating"] === true,
      ...(nums ? { rect: clampRect(r as unknown as Rect) } : {}),
    };
  } catch {
    return { floating: false };
  }
}

function saveLayout(l: Layout): void {
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify(l));
  } catch {
    // The panel simply comes back docked next time.
  }
}

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

  // Floating state is read once, lazily — `loadLayout` touches `localStorage` and clamps
  // against the viewport, neither of which belongs in a render.
  const [layout, setLayout] = useState<Layout>(loadLayout);
  const setFloating = useCallback((floating: boolean) => {
    setLayout((l) => ({ ...l, floating, rect: l.rect ?? defaultRect() }));
  }, []);
  const setRect = useCallback((rect: Rect) => {
    setLayout((l) => ({ ...l, rect }));
  }, []);
  useEffect(() => saveLayout(layout), [layout]);

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
    // Not a control — there is nothing a trader can switch off. § 6.9 requires the venue to
    // say ADL exists in plain language, and a tab is where that lives alongside the numbers.
    { id: "adl", label: "ADL" },
  ];

  const body = (
    <>
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
        <div className="flex items-center gap-3">
          {tab === "positions" ? (
            <span className="hidden text-[10px] text-ink-dim lg:block">
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
          {layout.floating ? null : (
            <button
              onClick={() => setFloating(true)}
              title="Float this panel so it can be moved and resized"
              className="rounded border border-line px-2.5 py-1 text-[11px] text-ink-muted hover:border-brand hover:text-ink"
            >
              Pop out
            </button>
          )}
        </div>
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
      ) : tab === "summary" ? (
        <SummaryPanel
          totals={totals}
          positions={positions}
          truncated={truncated}
          loading={loading}
          loadError={error}
        />
      ) : (
        <AdlPanel
          positions={positions}
          markets={markets}
          active={tab === "adl"}
        />
      )}
    </>
  );

  if (!layout.floating) {
    return <div className="border-t border-line-soft">{body}</div>;
  }

  // Docked, the panel is a row in a column layout; floated, it leaves a hole. The strip
  // keeps the chart from jumping the full height of the table and, more importantly, is the
  // only way back for someone who has dragged the window somewhere they cannot see.
  return (
    <>
      <div className="flex items-center justify-between gap-3 border-t border-line-soft px-5 py-2 text-[11px] text-ink-dim">
        <span>Positions and activity are in a floating window.</span>
        <button
          onClick={() => setFloating(false)}
          className="rounded border border-line px-2.5 py-1 text-ink-muted hover:border-brand hover:text-ink"
        >
          Bring it back
        </button>
      </div>
      <FloatingPanel
        title={`Activity · ${positions.length} open`}
        rect={layout.rect ?? defaultRect()}
        onRectChange={setRect}
        onDock={() => setFloating(false)}
      >
        {body}
      </FloatingPanel>
    </>
  );
}
