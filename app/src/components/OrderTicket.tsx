import { useMemo, useState } from "react";
import {
  Direction,
  SizingError,
  carryOver,
  carryRatePerHour,
  confBps,
  minCollateralFor,
  quoteOpen,
  resolveSize,
  sideForOpen,
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

const RATE_PRECISION = 1_000_000_000n; // 1e9

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
  onOpened,
}: {
  market: LoadedMarket;
  price?: LivePrice;
  priceAccount?: Address;
  /**
   * Called after a fill lands, so the positions table can re-read the chain.
   *
   * Without it the ticket refreshed only the *account* panel, and a new position stayed
   * invisible until a full page reload — the position was open and accruing the whole time,
   * which is the worst version of that bug.
   */
  onOpened?: () => void;
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

      // Priced the way `open_position` prices it, not off the oracle mid.
      //
      // The program fills at `execution_price` — the mid widened by base spread, confidence
      // and skew, against the trader — takes the notional of *that* with `mul_div_ceil`, and
      // then checks `div_ceil(notional, collateral) <= max_leverage`. Sizing margin off the
      // mid under-collateralises by a hair, which is invisible below the market's maximum
      // leverage and a hard `LeverageTooHigh` refusal at it. Measured on devnet: ETH/USD,
      // 10x on a 10x market, error 6047.
      //
      // Both sides are quoted because the ticket has no direction until submit. A buy fills
      // above the mid and a sell below, so the buy is always the wider of the two; taking the
      // larger margin means the figure shown here is never less than the figure charged.
      const spreadParams = {
        baseSpreadBps: market.baseSpreadBps,
        confSpreadMultiplierBps: d.confSpreadMultiplierBps,
        skewImpactBpsPerUnit: d.skewImpactBpsPerUnit,
        baseOiLong: d.baseOiLong,
        baseOiShort: d.baseOiShort,
      };
      const confidenceBps = confBps(price.price, price.conf);
      const long = quoteOpen({
        market: spreadParams,
        oraclePrice: price.price,
        confidenceBps,
        sizeBase,
        side: sideForOpen(Direction.Long),
      });
      const short = quoteOpen({
        market: spreadParams,
        oraclePrice: price.price,
        confidenceBps,
        sizeBase,
        side: sideForOpen(Direction.Short),
      });
      // `quoteOpen` returns undefined only past the 50% total-spread ceiling, where the
      // program errors too. Quoting a narrower fallback would be quoting a lie.
      if (!long || !short) {
        return {
          error: "spread is beyond the protocol's 50% ceiling" as const,
        };
      }
      const sides = { long, short };

      const notional = long.notional;
      const marginLong = minCollateralFor({
        notional: long.notional,
        leverage,
        imrBps: d.imrBps,
      });
      const marginShort = minCollateralFor({
        notional: short.notional,
        leverage,
        imrBps: d.imrBps,
      });
      const margin = marginLong > marginShort ? marginLong : marginShort;
      const fee = (notional * d.openFeeRate) / RATE_PRECISION;
      // What the spread actually costs on this fill, rather than base spread on the mid:
      // the gap between the buy and sell notionals is the full round-trip width.
      const spread = long.notional - short.notional;

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
        sides,
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
  }, [mode, value, leverage, price, market.symbol, market.baseSpreadBps, d]);

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
    if (await send([ix], OPEN_POSITION_CU)) {
      refreshAccount();
      onOpened?.();
    }
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
            {/* The fill, not the mid. A buy crosses up and a sell crosses down, and these
                are the prices the program will actually book. */}
            <Row
              label="Fill, long"
              value={fmtPrice(quote.sides.long.execPrice, places)}
            />
            <Row
              label="Fill, short"
              value={fmtPrice(quote.sides.short.execPrice, places)}
            />
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
