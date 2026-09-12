import type { LoadedMarket } from "@/lib/markets";
import type { LivePrice } from "@/lib/prices";
import { MAX_STALENESS_SECONDS } from "@/lib/prices";
import {
  fmtBps,
  fmtPrice,
  fmtRateBps,
  fmtUsd,
  priceplaces,
} from "@/lib/format";

function Stat({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: string;
}) {
  return (
    <div>
      <div className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
        {label}
      </div>
      <div className={`tnum text-sm ${tone ?? "text-ink"}`}>{value}</div>
    </div>
  );
}

export function MarketPanel({
  market,
  price,
}: {
  market: LoadedMarket;
  price?: LivePrice;
}) {
  const places = priceplaces(market.symbol);
  const d = market.data;

  return (
    <div className="border-b border-line-soft px-5 py-4">
      <div className="flex flex-wrap items-baseline gap-x-8 gap-y-3">
        <div>
          <div className="flex items-center gap-2">
            <h1 className="text-xl font-bold tracking-tight">
              {market.symbol}
            </h1>
            <span className="tnum rounded border border-brand/40 px-1.5 py-0.5 text-[10px] text-brand-soft">
              max {market.maxLeverage}x
            </span>
          </div>
          <div className="text-[11px] text-ink-dim">
            market #{market.index} · {market.status}
          </div>
        </div>

        <div>
          <div className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
            Oracle
          </div>
          <div className="tnum text-2xl font-semibold">
            {price ? fmtPrice(price.price, places) : "—"}
          </div>
        </div>

        <Stat
          label="Confidence"
          value={price ? `± ${fmtPrice(price.conf, places)}` : "—"}
        />
        <Stat
          label="Age"
          value={
            price ? `${price.ageSeconds}s / ${MAX_STALENESS_SECONDS}s` : "—"
          }
          tone={price?.stale ? "text-warn" : "text-ink"}
        />
        <Stat label="Open fee" value={fmtRateBps(d.openFeeRate)} />
        <Stat label="Base spread" value={fmtBps(d.baseSpreadBps)} />
        <Stat label="Init margin" value={fmtBps(d.imrBps)} />
        <Stat label="Maint margin" value={fmtBps(d.mmrBps)} />
        <Stat label="OI long" value={`$${fmtUsd(d.oiLong, 0)}`} />
        <Stat label="OI short" value={`$${fmtUsd(d.oiShort, 0)}`} />
      </div>

      {!price ? (
        <p className="mt-3 rounded border border-line bg-surface-high px-3 py-2 text-xs text-ink-muted">
          No price account for this feed on this cluster. Run{" "}
          <span className="tnum">price-poster</span>, then copy{" "}
          <span className="tnum">price-accounts.json</span> into{" "}
          <span className="tnum">app/public/</span>.
        </p>
      ) : price.stale ? (
        <p className="mt-3 rounded border border-warn/30 bg-warn/10 px-3 py-2 text-xs text-warn">
          This price is {price.ageSeconds}s old. The program refuses any trade
          against an oracle older than {MAX_STALENESS_SECONDS}s, so an order
          here would be rejected with
          <span className="tnum"> OracleStale</span>. Run the price poster
          against this cluster.
        </p>
      ) : null}
    </div>
  );
}
