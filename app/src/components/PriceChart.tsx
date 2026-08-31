import { useEffect, useRef, useState } from "react";
import { CandlestickSeries, createChart, type IChartApi, type ISeriesApi } from "lightweight-charts";

import { fetchBars, toDisplay, type Bar } from "@/lib/history";
import { HERMES_TOKEN, HERMES_URL } from "@/config";
import type { LivePrice } from "@/lib/prices";
import { priceplaces } from "@/lib/format";

type Props = {
  feedIdHex: string;
  symbol: string;
  live?: LivePrice | undefined;
};

/**
 * The oracle's own price history.
 *
 * Deliberately not a third-party feed. A chart drawn from an exchange API would disagree
 * with the price the program fills at, and a trader comparing the two would be right to
 * distrust the venue. These are Pyth's numbers — the same feed the poster publishes.
 */
export function PriceChart({ feedIdHex, symbol, live }: Props) {
  const box = useRef<HTMLDivElement | null>(null);
  const chart = useRef<IChartApi | null>(null);
  const series = useRef<ISeriesApi<"Candlestick"> | null>(null);
  const last = useRef<Bar | undefined>(undefined);
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
        vertLines: { color: "rgba(42,42,48,0.4)" },
        horzLines: { color: "rgba(42,42,48,0.4)" },
      },
      rightPriceScale: { borderColor: "#2a2a30" },
      timeScale: { borderColor: "#2a2a30", timeVisible: true, secondsVisible: false },
      crosshair: { mode: 1 },
      autoSize: true,
    });
    const s = c.addSeries(CandlestickSeries, {
      // Purple up, white down.
      upColor: "#9945ff",
      wickUpColor: "#9945ff",
      borderUpColor: "#9945ff",
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

  // Load history whenever the market changes.
  useEffect(() => {
    let cancelled = false;
    setState("loading");
    void (async () => {
      const bars = await fetchBars(
        { hermesUrl: HERMES_URL, token: HERMES_TOKEN },
        feedIdHex,
      ).catch((): Bar[] => []);
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
  }, [feedIdHex]);

  // Fold each poll into the current bar, opening a new one when the minute rolls over.
  // `update` requires a time >= the last bar's, so the bar being extended must keep its
  // original stamp rather than take the tick's.
  useEffect(() => {
    if (!live || state !== "ready") return;
    const price = toDisplay(live.price);
    const slot = Math.floor(Number(live.publishTime) / 60) * 60;
    const prev = last.current;

    const bar: Bar =
      prev && prev.time === slot
        ? {
            time: slot,
            open: prev.open,
            high: Math.max(prev.high, price),
            low: Math.min(prev.low, price),
            close: price,
          }
        : { time: slot, open: prev?.close ?? price, high: price, low: price, close: price };

    if (prev && slot < prev.time) return; // a stale poll must not rewind the series
    last.current = bar;
    series.current?.update({ ...bar, time: bar.time as never });
  }, [live, state]);

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={box} className="absolute inset-0" />
      {state !== "ready" ? (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <div className="max-w-sm text-center text-xs text-ink-dim">
            {state === "loading" ? (
              "Loading oracle history…"
            ) : (
              <>
                No history for {symbol}. Pyth serves history per timestamp and rate-limits, so
                a closed market or a throttled response leaves this empty. The live figures
                above are read straight from the price account.
              </>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
