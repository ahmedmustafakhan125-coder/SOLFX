import type { LoadedMarket } from "@/lib/markets";
import type { LivePrice } from "@/lib/prices";
import { fmtPrice, priceplaces } from "@/lib/format";

type Props = {
  markets: LoadedMarket[];
  prices: Record<string, LivePrice | undefined>;
  selected: number | undefined;
  onSelect: (index: number) => void;
};

export function MarketList({ markets, prices, selected, onSelect }: Props) {
  const tradeable = markets.filter((m) => m.tradeable);
  const retired = markets.filter((m) => !m.tradeable);

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-line-soft">
      <div className="px-4 py-3 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
        Markets · {tradeable.length} live
      </div>

      <div className="flex-1 overflow-y-auto">
        {tradeable.map((m) => {
          const p = prices[m.feedIdHex];
          const active = m.index === selected;
          return (
            <button
              key={m.index}
              onClick={() => onSelect(m.index)}
              className={`flex w-full items-center gap-2 border-l-2 px-4 py-2.5 text-left transition-colors ${
                active
                  ? "border-brand bg-surface-high"
                  : "border-transparent hover:bg-surface-high/60"
              }`}
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5">
                  <span className="text-sm font-semibold">{m.symbol}</span>
                  <span className="tnum rounded border border-line px-1 text-[9px] text-ink-dim">
                    {m.maxLeverage}x
                  </span>
                </div>
                <div className="tnum text-xs text-ink-muted">
                  {p ? fmtPrice(p.price, priceplaces(m.symbol)) : "—"}
                </div>
              </div>
              {p?.stale ? (
                <span
                  className="rounded bg-warn/15 px-1 py-0.5 text-[9px] font-medium text-warn"
                  title={`Price is ${p.ageSeconds}s old; the program rejects anything over 60s`}
                >
                  STALE
                </span>
              ) : null}
            </button>
          );
        })}

        {retired.length > 0 ? (
          <>
            <div className="px-4 pt-5 pb-2 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
              Not trading
            </div>
            {retired.map((m) => (
              <div
                key={m.index}
                className="flex items-center justify-between px-4 py-2 text-xs text-ink-dim"
                title={`Market ${m.index} is ${m.status}`}
              >
                <span>{m.symbol}</span>
                <span className="text-[10px] uppercase">{m.status}</span>
              </div>
            ))}
          </>
        ) : null}
      </div>
    </aside>
  );
}
