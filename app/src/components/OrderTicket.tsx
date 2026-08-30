import { useMemo, useState } from "react";
import { SizingError, resolveSize, unitsPerLot, type Sizing } from "@solfx/client";

import type { LoadedMarket } from "@/lib/markets";
import type { LivePrice } from "@/lib/prices";
import { fmtBase, fmtPrice, fmtUsd, priceplaces } from "@/lib/format";

const NOTIONAL_DIVISOR = 1_000_000_000_000n; // 1e12
const RATE_PRECISION = 1_000_000_000n; // 1e9
const BPS = 10_000n;

type Mode = Sizing["mode"];

const MODES: { id: Mode; label: string; hint: string }[] = [
  { id: "lots", label: "Lots", hint: "1 lot = contract size for this asset class" },
  { id: "quantity", label: "Quantity", hint: "units of the base asset" },
  { id: "notional", label: "Notional", hint: "whole USD of exposure" },
];

export function OrderTicket({ market, price }: { market: LoadedMarket; price?: LivePrice }) {
  const [mode, setMode] = useState<Mode>("notional");
  const [value, setValue] = useState("1000");
  const [leverage, setLeverage] = useState(Math.min(10, market.maxLeverage));

  const places = priceplaces(market.symbol);
  const d = market.data;

  const quote = useMemo(() => {
    if (!price) return { error: "waiting for a price" as const };
    try {
      const sizing: Sizing =
        mode === "notional"
          ? { mode, usd: BigInt(value.trim() === "" ? "0" : value.trim()) }
          : { mode, value };
      const sizeBase = resolveSize(market.symbol, sizing, price.price);

      // The program's own formula: notional = size_base * price / NOTIONAL_DIVISOR.
      const notional = (sizeBase * price.price) / NOTIONAL_DIVISOR;
      const margin = notional / BigInt(leverage);
      const fee = (notional * d.openFeeRate) / RATE_PRECISION;
      const spread = (notional * BigInt(d.baseSpreadBps)) / BPS;

      const belowMin = sizeBase < d.minPositionSize;
      const aboveMax = sizeBase > d.maxPositionSize;
      // MIN_NOTIONAL_QUOTE is $1.00 and the floor is inclusive.
      const belowNotional = notional < 1_000_000n;

      return { sizeBase, notional, margin, fee, spread, belowMin, aboveMax, belowNotional };
    } catch (e) {
      return { error: e instanceof SizingError ? e.message : String(e) };
    }
  }, [mode, value, leverage, price, market.symbol, d]);

  const blocked =
    "error" in quote ||
    quote.belowMin ||
    quote.aboveMax ||
    quote.belowNotional ||
    !market.tradeable ||
    price?.stale === true;

  return (
    <div className="flex w-80 shrink-0 flex-col gap-4 border-l border-line-soft p-4">
      <div className="grid grid-cols-3 gap-1 rounded-md bg-surface-high p-1">
        {MODES.map((m) => (
          <button
            key={m.id}
            title={m.hint}
            onClick={() => {
              setMode(m.id);
              setValue(m.id === "notional" ? "1000" : m.id === "lots" ? "0.01" : "1000");
            }}
            className={`rounded px-2 py-1.5 text-xs font-medium transition-colors ${
              mode === m.id ? "bg-brand text-white" : "text-ink-muted hover:text-ink"
            }`}
          >
            {m.label}
          </button>
        ))}
      </div>

      <label className="block">
        <div className="mb-1 flex items-baseline justify-between">
          <span className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
            {mode === "notional" ? "Exposure" : mode === "lots" ? "Lots" : "Quantity"}
          </span>
          <span className="text-[10px] text-ink-dim">
            {mode === "notional"
              ? "USD"
              : mode === "lots"
                ? `1 lot = ${unitsPerLot(market.symbol).toString()}`
                : market.symbol.slice(0, 3)}
          </span>
        </div>
        <input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          inputMode="decimal"
          className="tnum w-full rounded-md border border-line bg-surface px-3 py-2 text-lg outline-none focus:border-brand"
        />
      </label>

      <div>
        <div className="mb-1 flex items-baseline justify-between">
          <span className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">Leverage</span>
          <span className="tnum text-xs text-brand-soft">{leverage}x</span>
        </div>
        <input
          type="range"
          min={1}
          max={market.maxLeverage}
          value={leverage}
          onChange={(e) => setLeverage(Number(e.target.value))}
          className="w-full accent-[var(--sf-purple)]"
        />
        <div className="tnum mt-1 flex justify-between text-[10px] text-ink-dim">
          <span>1x</span>
          <span>max {market.maxLeverage}x</span>
        </div>
      </div>

      <div className="space-y-1.5 rounded-md border border-line-soft bg-surface-high p-3 text-xs">
        {"error" in quote ? (
          <div className="text-warn">{quote.error}</div>
        ) : (
          <>
            <Row label="Size" value={`${fmtBase(quote.sizeBase)} ${market.symbol.slice(0, 3)}`} />
            <Row label="Notional" value={`$${fmtUsd(quote.notional)}`} />
            <Row label="Margin required" value={`$${fmtUsd(quote.margin)}`} />
            <Row label="Open fee" value={`$${fmtUsd(quote.fee, 6)}`} />
            <Row label="Spread cost" value={`$${fmtUsd(quote.spread, 6)}`} />
            {price ? (
              <Row label="Oracle" value={fmtPrice(price.price, places)} />
            ) : null}
          </>
        )}
      </div>

      {"error" in quote ? null : (
        <Warnings
          belowMin={quote.belowMin}
          aboveMax={quote.aboveMax}
          belowNotional={quote.belowNotional}
          tradeable={market.tradeable}
          stale={price?.stale === true}
          status={market.status}
        />
      )}

      <div className="grid grid-cols-2 gap-2">
        <button
          disabled={blocked}
          className="rounded-md bg-long/90 py-2.5 text-sm font-semibold text-black hover:bg-long disabled:cursor-not-allowed disabled:opacity-30"
        >
          Long / Buy
        </button>
        <button
          disabled={blocked}
          className="rounded-md bg-short/90 py-2.5 text-sm font-semibold text-black hover:bg-short disabled:cursor-not-allowed disabled:opacity-30"
        >
          Short / Sell
        </button>
      </div>

      <p className="text-[10px] leading-relaxed text-ink-dim">
        Every figure above is computed with the program's own formulas from live account data.
        Order submission is not wired yet.
      </p>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between">
      <span className="text-ink-dim">{label}</span>
      <span className="tnum">{value}</span>
    </div>
  );
}

function Warnings(p: {
  belowMin: boolean;
  aboveMax: boolean;
  belowNotional: boolean;
  tradeable: boolean;
  stale: boolean;
  status: string;
}) {
  const items: string[] = [];
  if (!p.tradeable) items.push(`Market is ${p.status}; it does not permit opening.`);
  if (p.stale) items.push("Oracle is older than 60s — the program would reject this.");
  if (p.belowNotional) items.push("Below the $1.00 minimum notional.");
  if (p.belowMin) items.push("Below this market's minimum position size.");
  if (p.aboveMax) items.push("Above this market's maximum position size.");
  if (items.length === 0) return null;
  return (
    <ul className="space-y-1 rounded-md border border-warn/30 bg-warn/10 p-2 text-[11px] text-warn">
      {items.map((t) => (
        <li key={t}>{t}</li>
      ))}
    </ul>
  );
}
