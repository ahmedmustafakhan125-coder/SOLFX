import { useMemo, useState } from "react";
import {
  Direction,
  SizingError,
  carryOver,
  carryRatePerHour,
  resolveSize,
  unitsPerLot,
  type Sizing,
} from "@solfx/client";
import type { Address } from "@solana/kit";

import type { LoadedMarket } from "@/lib/markets";
import type { LivePrice } from "@/lib/prices";
import { fmtBase, fmtPrice, fmtUsd, priceplaces } from "@/lib/format";
import { useAccount } from "@/hooks/useAccount";
import { useSigner } from "@/hooks/useSigner";
import { useSend } from "@/hooks/useSend";
import { useRpc } from "@/hooks/useSolfx";
import {
  OPEN_POSITION_CU,
  buildOpenPosition,
  firstFreeNonce,
} from "@/lib/trade";

const NOTIONAL_DIVISOR = 1_000_000_000_000n; // 1e12
const RATE_PRECISION = 1_000_000_000n; // 1e9
const BPS = 10_000n;

type Mode = Sizing["mode"];

const MODES: { id: Mode; label: string; hint: string }[] = [
  {
    id: "lots",
    label: "Lots",
    hint: "1 lot = contract size for this asset class",
  },
  { id: "quantity", label: "Quantity", hint: "units of the base asset" },
  { id: "notional", label: "Notional", hint: "whole USD of exposure" },
];

export function OrderTicket({
  market,
  price,
  priceAccount,
}: {
  market: LoadedMarket;
  price?: LivePrice;
  priceAccount?: Address;
}) {
  const [mode, setMode] = useState<Mode>("notional");
  const [value, setValue] = useState("1000");
  const [leverage, setLeverage] = useState(Math.min(10, market.maxLeverage));
  const { status: account, refresh: refreshAccount } = useAccount();
  const signer = useSigner();
  const rpc = useRpc();
  const { send, busy, signature, error: sendError, logs, reset } = useSend();
  const [slippageBps, setSlippageBps] = useState(100);

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

      // Carry, per side, over a day. Shown for both directions because the ticket has no
      // direction until submit — and because a swap table with both sides is what a trader
      // expects anyway. The markup is kept separate from the interest differential: that
      // split is the whole reason `Market` stores them as two fields (§ 6.7), and quoting one
      // blended number is the opacity we are supposed to be improving on.
      const rates = {
        long: carryRatePerHour({
          baseAnnual: d.rateBaseAnnual,
          quoteAnnual: d.rateQuoteAnnual,
          markupPerHour: d.carryRatePerHour,
          direction: Direction.Long,
        }),
        short: carryRatePerHour({
          baseAnnual: d.rateBaseAnnual,
          quoteAnnual: d.rateQuoteAnnual,
          markupPerHour: d.carryRatePerHour,
          direction: Direction.Short,
        }),
      };
      const carry = {
        long: carryOver(notional, rates.long.total, 24n),
        short: carryOver(notional, rates.short.total, 24n),
        markup: carryOver(notional, rates.long.markup, 24n),
      };

      return {
        sizeBase,
        notional,
        margin,
        fee,
        spread,
        carry,
        belowMin,
        aboveMax,
        belowNotional,
      };
    } catch (e) {
      return { error: e instanceof SizingError ? e.message : String(e) };
    }
  }, [mode, value, leverage, price, market.symbol, d]);

  const free = account?.freeCollateral;
  const underfunded =
    !("error" in quote) && free !== undefined && quote.margin > free;

  const blocked =
    "error" in quote ||
    quote.belowMin ||
    quote.aboveMax ||
    quote.belowNotional ||
    underfunded ||
    !market.tradeable ||
    price?.stale === true;

  async function submit(direction: Direction) {
    if (!signer || !account || !priceAccount || !price || "error" in quote)
      return;
    reset();
    const nonce = await firstFreeNonce(
      rpc,
      account.userAccountPda,
      market.index
    );
    if (nonce === undefined) return;
    const ix = await buildOpenPosition({
      signer,
      marketIndex: market.index,
      direction,
      sizeBase: quote.sizeBase,
      collateral: quote.margin,
      price: price.price,
      slippageBps,
      priceUpdate: priceAccount,
      nonce,
    });
    if (await send([ix], OPEN_POSITION_CU)) refreshAccount();
  }

  return (
    <div className="flex flex-col gap-4 p-4">
      <div className="grid grid-cols-3 gap-1 rounded-md bg-surface-high p-1">
        {MODES.map((m) => (
          <button
            key={m.id}
            title={m.hint}
            onClick={() => {
              setMode(m.id);
              setValue(
                m.id === "notional" ? "1000" : m.id === "lots" ? "0.01" : "1000"
              );
            }}
            className={`rounded px-2 py-1.5 text-xs font-medium transition-colors ${
              mode === m.id
                ? "bg-brand text-white"
                : "text-ink-muted hover:text-ink"
            }`}
          >
            {m.label}
          </button>
        ))}
      </div>

      <label className="block">
        <div className="mb-1 flex items-baseline justify-between">
          <span className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
            {mode === "notional"
              ? "Exposure"
              : mode === "lots"
                ? "Lots"
                : "Quantity"}
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
          <span className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
            Leverage
          </span>
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

      <div>
        <div className="mb-1 flex items-baseline justify-between">
          <span className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
            Slippage
          </span>
          <span className="tnum text-xs text-ink-muted">
            {(slippageBps / 100).toFixed(2)}%
          </span>
        </div>
        <div className="grid grid-cols-4 gap-1">
          {[10, 50, 100, 300].map((b) => (
            <button
              key={b}
              onClick={() => setSlippageBps(b)}
              className={`tnum rounded px-1 py-1 text-[11px] ${
                slippageBps === b
                  ? "bg-brand text-white"
                  : "bg-surface-high text-ink-muted"
              }`}
            >
              {(b / 100).toFixed(b < 100 ? 1 : 0)}%
            </button>
          ))}
        </div>
        <p className="mt-1 text-[10px] leading-snug text-ink-dim">
          A bound, not a preference — the program always compares, so there is
          no way to disable it.
        </p>
      </div>

      <div className="space-y-1.5 rounded-md border border-line-soft bg-surface-high p-3 text-xs">
        {"error" in quote ? (
          <div className="text-warn">{quote.error}</div>
        ) : (
          <>
            <Row
              label="Size"
              value={`${fmtBase(quote.sizeBase)} ${market.symbol.slice(0, 3)}`}
            />
            <Row label="Notional" value={`$${fmtUsd(quote.notional)}`} />
            <Row label="Margin required" value={`$${fmtUsd(quote.margin)}`} />
            <Row label="Open fee" value={`$${fmtUsd(quote.fee, 6)}`} />
            <Row label="Spread cost" value={`$${fmtUsd(quote.spread, 6)}`} />
            <Row
              label="Carry / day, long"
              value={`$${fmtUsd(quote.carry.long, 6)}`}
            />
            <Row
              label="Carry / day, short"
              value={`$${fmtUsd(quote.carry.short, 6)}`}
            />
            <Row
              label="…of which SolFX markup"
              value={`$${fmtUsd(quote.carry.markup, 6)}`}
            />
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
          underfunded={underfunded}
          free={free}
          tradeable={market.tradeable}
          stale={price?.stale === true}
          status={market.status}
        />
      )}

      <div className="grid grid-cols-2 gap-2">
        <button
          onClick={() => void submit(Direction.Long)}
          disabled={blocked || busy || !signer || !priceAccount}
          className="rounded-md bg-long/90 py-2.5 text-sm font-semibold text-black hover:bg-long disabled:cursor-not-allowed disabled:opacity-30"
        >
          {busy ? "Confirming…" : "Long / Buy"}
        </button>
        <button
          onClick={() => void submit(Direction.Short)}
          disabled={blocked || busy || !signer || !priceAccount}
          className="rounded-md bg-short/90 py-2.5 text-sm font-semibold text-black hover:bg-short disabled:cursor-not-allowed disabled:opacity-30"
        >
          {busy ? "Confirming…" : "Short / Sell"}
        </button>
      </div>

      {signature ? (
        <a
          href={`https://explorer.solana.com/tx/${signature}?cluster=devnet`}
          target="_blank"
          rel="noreferrer"
          className="block truncate rounded border border-long/30 bg-long/10 p-2 text-[11px] text-long underline"
        >
          filled · {signature.slice(0, 20)}…
        </a>
      ) : null}

      {sendError ? (
        <div className="space-y-1 rounded border border-short/40 bg-short/10 p-2">
          <div className="text-[11px] text-short">{sendError}</div>
          {logs?.length ? (
            <pre className="tnum max-h-40 overflow-auto whitespace-pre-wrap text-[10px] leading-snug text-ink-muted">
              {logs.join("\n")}
            </pre>
          ) : null}
        </div>
      ) : null}

      <p className="text-[10px] leading-relaxed text-ink-dim">
        Every figure above is computed with the program's own formulas from live
        account data.
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
  underfunded: boolean;
  free: bigint | undefined;
  tradeable: boolean;
  stale: boolean;
  status: string;
}) {
  const items: string[] = [];
  if (!p.tradeable)
    items.push(`Market is ${p.status}; it does not permit opening.`);
  if (p.underfunded) {
    items.push(
      `Margin exceeds your free collateral${p.free === undefined ? "" : ` of $${fmtUsd(p.free)}`}. Deposit more above.`
    );
  }
  if (p.stale)
    items.push("Oracle is older than 60s — the program would reject this.");
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
