import { useEffect, useRef, useState } from "react";
import { CandlestickSeries, createChart, type IChartApi, type ISeriesApi } from "lightweight-charts";

import { fetchOhlc, toDisplay, TIMEFRAMES, type Ohlc, type Timeframe } from "@/lib/ohlc";
import { PYTHPRO_URL } from "@/config";
import type { LivePrice } from "@/lib/prices";
import { priceplaces } from "@/lib/format";

type Props = {
  symbol: string;
  live?: LivePrice | undefined;
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
export function PriceChart({ symbol, live }: Props) {
  const box = useRef<HTMLDivElement | null>(null);
  const chart = useRef<IChartApi | null>(null);
  const series = useRef<ISeriesApi<"Candlestick"> | null>(null);
  const last = useRef<Ohlc | undefined>(undefined);
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
      timeScale: { borderColor: "#4c4546", timeVisible: true, secondsVisible: false },
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
      const bars = await fetchOhlc(PYTHPRO_URL, symbol, tf).catch((): Ohlc[] => []);
      if (cancelled) return;
      if (bars.length === 0) {
        setState("empty");
        return;
      }
      last.current = bars[bars.length - 1];
      series.current?.setData(bars.map((b) => ({ ...b, time: b.time as never })));
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
        : { time: slot, open: prev?.close ?? price, high: price, low: price, close: price };

    last.current = bar;
    series.current?.update({ ...bar, time: bar.time as never });
  }, [live, state, tf]);

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
