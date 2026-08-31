import { useEffect, useRef, useState } from "react";
import { AreaSeries, createChart, type IChartApi, type ISeriesApi } from "lightweight-charts";

import { fetchHistory, toDisplay, type Candle } from "@/lib/history";
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
  const series = useRef<ISeriesApi<"Area"> | null>(null);
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
    const s = c.addSeries(AreaSeries, {
      lineColor: "#9945ff",
      topColor: "rgba(153,69,255,0.28)",
      bottomColor: "rgba(153,69,255,0.02)",
      lineWidth: 2,
      priceFormat: { type: "price", precision: priceplaces(symbol), minMove: 1e-5 },
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
      const bars = await fetchHistory(
        { hermesUrl: HERMES_URL, token: HERMES_TOKEN },
        feedIdHex,
      ).catch((): Candle[] => []);
      if (cancelled) return;
      if (bars.length === 0) {
        setState("empty");
        return;
      }
      series.current?.setData(
        bars.map((b) => ({ time: b.time as never, value: b.value })),
      );
      chart.current?.timeScale().fitContent();
      setState("ready");
    })();
    return () => {
      cancelled = true;
    };
  }, [feedIdHex]);

  // Append the live poll. `update` requires a time >= the last bar, which the poll satisfies.
  useEffect(() => {
    if (!live || state !== "ready") return;
    series.current?.update({
      time: Number(live.publishTime) as never,
      value: toDisplay(live.price),
    });
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
