import { useState } from "react";
import {
  Direction,
  pnlOnClosedPortion,
  previewReduce,
  reduceIsFullClose,
  sizeForPercent,
} from "@solfx/client";
import type { Address } from "@solana/kit";

import type { OpenPosition } from "@/lib/positions";
import {
  CLOSE_POSITION_CU,
  DECREASE_POSITION_CU,
  buildClosePosition,
  buildDecreasePosition,
} from "@/lib/positions";
import { useSend } from "@/hooks/useSend";
import { useSigner } from "@/hooks/useSigner";
import { fmtBase, fmtUsd } from "@/lib/format";

const PRESETS = [25, 50, 75, 100] as const;

type Props = {
  position: OpenPosition;
  symbol: string;
  /** The current oracle mark, or undefined when the feed is not readable. */
  mark: bigint | undefined;
  priceUpdate: Address | undefined;
  onDone: () => void;
};

/**
 * Close part of a position.
 *
 * # The one thing this component exists to get right
 *
 * `decrease_position` requires `size_delta < size_base` **strictly**. So "close 100%" is not
 * a large reduction, it is a different instruction — `close_position`, which also reclaims
 * the position account's rent. The panel routes on `reduceIsFullClose` and says which one it
 * is about to send, because a trader who picks 100% and gets `ReductionExceedsSize` has no
 * way to know why.
 *
 * Every figure shown is `@solfx/client`'s port of the program's own proportional split:
 * collateral and entry notional are floored, and the odd unit stays with the surviving
 * position rather than coming back to the trader.
 */
export function ReducePanel({
  position,
  symbol,
  mark,
  priceUpdate,
  onDone,
}: Props) {
  const signer = useSigner();
  const { send, busy, error, logs, reset } = useSend();
  const [percent, setPercent] = useState(50);

  const reducible = {
    sizeBase: position.data.sizeBase,
    collateral: position.data.collateral,
    entryNotional: position.data.entryNotional,
  };

  const sizeDelta = sizeForPercent(reducible, percent);
  const valid = sizeDelta > 0n;
  const preview = valid ? previewReduce(reducible, sizeDelta) : undefined;
  const full = valid && reduceIsFullClose(sizeDelta, reducible);
  const pnl =
    preview && mark !== undefined
      ? pnlOnClosedPortion(
          preview,
          mark,
          position.data.direction === Direction.Long
        )
      : undefined;

  async function submit() {
    if (!signer || !preview || mark === undefined || !priceUpdate) return;
    reset();
    const common = {
      signer,
      marketIndex: position.marketIndex,
      nonce: position.nonce,
      direction: position.data.direction,
      price: mark,
      slippageBps: 100,
      priceUpdate,
    };
    const ix = full
      ? await buildClosePosition(common)
      : await buildDecreasePosition({
          ...common,
          sizeDelta: preview.sizeDelta,
        });
    if (await send([ix], full ? CLOSE_POSITION_CU : DECREASE_POSITION_CU)) {
      onDone();
    }
  }

  return (
    <div className="space-y-3 border-t border-line-soft bg-surface-high/40 px-5 py-4">
      <div className="flex items-center gap-3">
        <span className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
          Reduce {symbol}
        </span>
        <div className="flex gap-1">
          {PRESETS.map((p) => (
            <button
              key={p}
              type="button"
              onClick={() => setPercent(p)}
              className={
                "rounded px-2 py-0.5 text-[11px] font-semibold transition-colors " +
                (percent === p
                  ? "bg-brand text-white"
                  : "border border-line text-ink-dim hover:text-ink")
              }
            >
              {p === 100 ? "All" : `${p}%`}
            </button>
          ))}
        </div>
      </div>

      <input
        type="range"
        min={1}
        max={100}
        value={percent}
        onChange={(e) => setPercent(Number(e.target.value))}
        className="w-full accent-brand"
        aria-label="Percentage of the position to close"
      />

      {preview ? (
        <div className="tnum grid gap-x-6 gap-y-1 rounded border border-line-soft bg-surface p-2 text-[11px] text-ink-dim sm:grid-cols-2">
          <div className="flex justify-between">
            <span>Closing</span>
            <span className="text-ink">{fmtBase(preview.sizeDelta)}</span>
          </div>
          <div className="flex justify-between">
            <span>Left open</span>
            <span className="text-ink">{fmtBase(preview.remainingSize)}</span>
          </div>
          <div className="flex justify-between">
            <span>Collateral returned</span>
            <span className="text-ink">
              ${fmtUsd(preview.releasedCollateral)}
            </span>
          </div>
          <div className="flex justify-between">
            <span>Collateral still posted</span>
            <span className="text-ink">
              ${fmtUsd(preview.remainingCollateral)}
            </span>
          </div>
          {pnl === undefined ? null : (
            <div className="flex justify-between sm:col-span-2">
              <span>P&amp;L on the closed part</span>
              <span className={pnl >= 0n ? "text-long" : "text-short"}>
                {pnl >= 0n ? "+" : "−"}${fmtUsd(pnl < 0n ? -pnl : pnl)}
              </span>
            </div>
          )}
        </div>
      ) : null}

      <p className="text-[11px] leading-relaxed text-ink-muted">
        {full
          ? "Closing the whole position, which returns its rent as well. That is close_position, not a reduction."
          : "Collateral and entry are split in proportion and floored, so the odd unit stays with the position that remains."}{" "}
        P&amp;L is at the current mark, before the spread, the close fee and
        accrued carry.
      </p>

      <button
        onClick={() => void submit()}
        disabled={
          busy || !signer || !preview || mark === undefined || !priceUpdate
        }
        className="w-full rounded-md bg-brand py-2 text-xs font-semibold text-white hover:bg-brand-dim disabled:opacity-50"
      >
        {busy
          ? "Confirming…"
          : full
            ? "Close the position"
            : `Close ${percent}%`}
      </button>

      {error ? (
        <div className="space-y-1 rounded border border-short/40 bg-short/10 p-2">
          <div className="text-[11px] text-short">{error}</div>
          {logs?.length ? (
            <pre className="tnum max-h-40 overflow-auto whitespace-pre-wrap text-[10px] leading-snug text-ink-muted">
              {logs.join("\n")}
            </pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
