import { useEffect, useRef, useState } from "react";
import {
  CandlestickSeries,
  createChart,
  type IChartApi,
  type IPriceLine,
  type ISeriesApi,
} from "lightweight-charts";

import {
  fetchOhlc,
  toDisplay,
  TIMEFRAMES,
  type Ohlc,
  type Timeframe,
} from "@/lib/ohlc";
import { PYTHPRO_URL } from "@/config";
import type { LivePrice } from "@/lib/prices";
import { priceplaces } from "@/lib/format";

/** One horizontal line to draw on the chart: an entry, or the price it liquidates at. */
export type ChartLevel = {
  readonly price: bigint;
  readonly label: string;
  readonly kind: "long" | "short" | "liquidation" | "take-profit" | "stop-loss";
};

const LEVEL_COLOUR: Record<ChartLevel["kind"], string> = {
  long: "#3fb950",
  short: "#f85149",
  liquidation: "#e0a33e",
  // A take-profit reads as the good outcome and a stop-loss as the bad one, matching the
  // long/short greens and reds rather than introducing two more hues to learn.
  "take-profit": "#2dd4bf",
  "stop-loss": "#fb7185",
};

type Props = {
  symbol: string;
  live?: LivePrice | undefined;
  /** Entry, liquidation and resting-order levels for open positions on this market. */
  levels?: readonly ChartLevel[];
};

/**
 * The oracle's own price history.
 *
 * Deliberately not a third-party feed. A chart drawn from an exchange API would disagree with
 * the price the program fills at, and a trader comparing the two would be right to distrust
 * the venue. These are Pyth's numbers — the same feed the poster publishes.
 *
 * The candles are Pyth's too, from the Pro History API, rather than bucketed from samples we
 * took ourselves. That distinction matters for the wicks: a sampled high is only the highest
 * price we happened to *ask* for, so it understates the real range by however much we missed
 * between polls. These are the true extremes of each interval.
 */
export function PriceChart({ symbol, live, levels }: Props) {
  const box = useRef<HTMLDivElement | null>(null);
  const chart = useRef<IChartApi | null>(null);
  const series = useRef<ISeriesApi<"Candlestick"> | null>(null);
  const last = useRef<Ohlc | undefined>(undefined);
  const lines = useRef<IPriceLine[]>([]);
  const [tf, setTf] = useState<Timeframe>(TIMEFRAMES[2]); // 1H
  const [state, setState] = useState<"loading" | "ready" | "empty">("loading");

  // Create the chart once.
  useEffect(() => {
    if (!box.current) return;
    const c = createChart(box.current, {
      layout: {
        background: { color: "transparent" },
        textColor: "#8b8b93",
        fontFamily: "'JetBrains Mono', ui-monospace, monospace",
        fontSize: 10,
      },
      grid: {
        vertLines: { color: "rgba(76,69,70,0.35)" },
        horzLines: { color: "rgba(76,69,70,0.35)" },
      },
      rightPriceScale: { borderColor: "#4c4546" },
      timeScale: {
        borderColor: "#4c4546",
        timeVisible: true,
        secondsVisible: false,
      },
      crosshair: { mode: 1 },
      autoSize: true,
    });
    const s = c.addSeries(CandlestickSeries, {
      // Purple up, white down.
      upColor: "#9843fe",
      wickUpColor: "#9843fe",
      borderUpColor: "#9843fe",
      downColor: "#e5e2e1",
      wickDownColor: "#e5e2e1",
      borderDownColor: "#e5e2e1",
      priceFormat: {
        type: "price",
        precision: priceplaces(symbol),
        minMove: 10 ** -priceplaces(symbol),
      },
    });
    chart.current = c;
    series.current = s;
    return () => {
      c.remove();
      chart.current = null;
      series.current = null;
    };
  }, [symbol]);

  // Reload whenever the market or the timeframe changes.
  useEffect(() => {
    let cancelled = false;
    setState("loading");
    void (async () => {
      const bars = await fetchOhlc(PYTHPRO_URL, symbol, tf).catch(
        (): Ohlc[] => []
      );
      if (cancelled) return;
      if (bars.length === 0) {
        setState("empty");
        return;
      }
      last.current = bars[bars.length - 1];
      series.current?.setData(
        bars.map((b) => ({ ...b, time: b.time as never }))
      );
      chart.current?.timeScale().fitContent();
      setState("ready");
    })();
    return () => {
      cancelled = true;
    };
  }, [symbol, tf]);

  // Fold each poll into the bar in progress, opening a new one when the interval rolls over.
  // `update` requires a time >= the last bar's, so the bar being extended keeps its original
  // stamp rather than taking the tick's.
  useEffect(() => {
    if (!live || state !== "ready") return;
    const price = toDisplay(live.price);
    const slot = Math.floor(Number(live.publishTime) / tf.seconds) * tf.seconds;
    const prev = last.current;
    if (prev && slot < prev.time) return; // a stale poll must not rewind the series

    const bar: Ohlc =
      prev && prev.time === slot
        ? {
            time: slot,
            open: prev.open,
            high: Math.max(prev.high, price),
            low: Math.min(prev.low, price),
            close: price,
          }
        : {
            time: slot,
            open: prev?.close ?? price,
            high: price,
            low: price,
            close: price,
          };

    last.current = bar;
    series.current?.update({ ...bar, time: bar.time as never });
  }, [live, state, tf]);

  // Draw where each position was entered and where it dies.
  //
  // Price lines rather than markers on a candle: an entry does not belong to a bar, it is a
  // level that stays relevant at every timeframe, and it survives the series being replaced
  // when the timeframe changes. Every line is torn down and redrawn when the levels change,
  // because lightweight-charts has no way to reconcile them by identity.
  useEffect(() => {
    const series_ = series.current;
    if (!series_) return;
    for (const line of lines.current) series_.removePriceLine(line);
    lines.current = [];
    if (state !== "ready") return;

    for (const level of levels ?? []) {
      lines.current.push(
        series_.createPriceLine({
          price: toDisplay(level.price),
          color: LEVEL_COLOUR[level.kind],
          lineWidth: 1,
          // Dashed for anything that has not happened yet — a liquidation price and a
          // resting order are both hypothetical, while an entry is a fact. Shape carries
          // that distinction so it does not rest on colour alone.
          lineStyle: level.kind === "long" || level.kind === "short" ? 0 : 2,
          axisLabelVisible: true,
          title: level.label,
        })
      );
    }
  }, [levels, state, tf, symbol]);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div className="flex items-center gap-1 border-b border-[var(--sf-border)] px-2 py-1">
        {TIMEFRAMES.map((t) => (
          <button
            key={t.label}
            type="button"
            onClick={() => setTf(t)}
            className={
              "px-2 py-0.5 font-mono text-[10px] uppercase tracking-wide transition-colors " +
              (t.label === tf.label
                ? "bg-[var(--sf-purple)] text-white"
                : "text-ink-dim hover:text-ink")
            }
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="relative min-h-0 flex-1">
        <div ref={box} className="absolute inset-0" />
        {state !== "ready" ? (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
            <div className="max-w-sm text-center text-xs text-ink-dim">
              {state === "loading"
                ? "Loading oracle history…"
                : `No ${tf.label} history for ${symbol}.`}
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
