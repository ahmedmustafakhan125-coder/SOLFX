import { useCallback, useEffect, useState } from "react";
import { Direction, TriggerKind } from "@solfx/client";
import type { Address } from "@solana/kit";

import {
  CANCEL_TRIGGER_CU,
  PLACE_TRIGGER_CU,
  buildCancelTrigger,
  buildPlaceTrigger,
  firstFreeOrderId,
  isPlaceable,
  readTriggers,
  requiredSide,
  type RestingTrigger,
} from "@/lib/triggers";
import type { OpenPosition } from "@/lib/positions";
import { useRpc } from "@/hooks/useSolfx";
import { useSend } from "@/hooks/useSend";
import { useSigner } from "@/hooks/useSigner";
import { fmtBase, fmtPrice } from "@/lib/format";

const PRICE_PRECISION = 1_000_000_000n;

/** A decimal price to `PRICE_PRECISION`, without ever touching a float. */
function toPrice(text: string): bigint | undefined {
  const t = text.trim();
  if (!/^\d+(\.\d{0,9})?$/.test(t)) return undefined;
  const [whole = "0", frac = ""] = t.split(".");
  const v =
    BigInt(whole) * PRICE_PRECISION + BigInt(frac.padEnd(9, "0") || "0");
  return v > 0n ? v : undefined;
}

type Props = {
  position: OpenPosition;
  symbol: string;
  places: number;
  /** Oracle mid at PRICE_PRECISION, or undefined while the price is unknown. */
  mark: bigint | undefined;
  priceUpdate: Address | undefined;
};

export function TriggerPanel({
  position,
  symbol,
  places,
  mark,
  priceUpdate,
}: Props) {
  const rpc = useRpc();
  const signer = useSigner();
  const { send, busy, error, logs, reset } = useSend();
  const [resting, setResting] = useState<RestingTrigger[]>([]);
  const [kind, setKind] = useState<TriggerKind>(TriggerKind.StopLoss);
  const [price, setPrice] = useState("");

  const refresh = useCallback(() => {
    void readTriggers(rpc, position.address as Address)
      .then(setResting)
      .catch(() => setResting([]));
  }, [rpc, position.address]);

  useEffect(refresh, [refresh]);

  const parsed = toPrice(price);
  const side = requiredSide(kind, position.data.direction);
  const wrongSide =
    parsed !== undefined && mark !== undefined
      ? !isPlaceable(kind, position.data.direction, parsed, mark)
      : false;

  async function place() {
    if (!signer || parsed === undefined || !priceUpdate) return;
    reset();
    const orderId = await firstFreeOrderId(rpc, position.address as Address);
    if (orderId === undefined) return;
    const ix = await buildPlaceTrigger({
      signer,
      marketIndex: position.marketIndex,
      position: position.address as Address,
      orderId,
      kind,
      triggerPrice: parsed,
      // Whole position. Scaling out is supported by the program (`size_base` may be less)
      // and is deliberately not exposed yet: a partial-close UI needs its own affordances.
      sizeBase: position.data.sizeBase,
      priceUpdate,
    });
    if (await send([ix], PLACE_TRIGGER_CU)) {
      setPrice("");
      refresh();
    }
  }

  async function cancel(t: RestingTrigger) {
    if (!signer) return;
    reset();
    if (await send([buildCancelTrigger(signer, t.address)], CANCEL_TRIGGER_CU))
      refresh();
  }

  return (
    <div className="space-y-3 bg-surface-high/40 px-5 py-3">
      <p className="max-w-2xl text-[11px] leading-relaxed text-ink-dim">
        A trigger fires when the <span className="text-ink">oracle</span>{" "}
        crosses your price, then closes at the execution price — which includes
        the spread, and in a gap may be well past the trigger. It is not a
        guaranteed fill. The order rests on chain, so anyone can execute it and
        you can verify it exists without trusting us.
      </p>

      {resting.length > 0 ? (
        <ul className="space-y-1">
          {resting.map((t) => (
            <li
              key={t.address}
              className="flex items-center justify-between gap-3 rounded border border-line-soft px-2.5 py-1.5 text-[11px]"
            >
              <span className="font-medium">
                {t.kind === TriggerKind.StopLoss ? "Stop loss" : "Take profit"}
              </span>
              <span className="tnum text-ink-dim">
                at {fmtPrice(t.triggerPrice, places)} · {fmtBase(t.sizeBase)}{" "}
                units
              </span>
              <button
                onClick={() => void cancel(t)}
                disabled={busy || !signer}
                className="rounded border border-line px-2 py-0.5 hover:border-short disabled:opacity-40"
              >
                Cancel
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-[11px] text-ink-dim">
          No resting orders on this position.
        </p>
      )}

      <div className="flex flex-wrap items-end gap-2">
        <div className="flex gap-1">
          {(
            [
              [TriggerKind.StopLoss, "Stop loss"],
              [TriggerKind.TakeProfit, "Take profit"],
            ] as const
          ).map(([k, label]) => (
            <button
              key={label}
              type="button"
              onClick={() => setKind(k)}
              className={
                "rounded px-2.5 py-1 text-[11px] font-medium transition-colors " +
                (kind === k
                  ? "bg-brand text-white"
                  : "text-ink-dim hover:text-ink")
              }
            >
              {label}
            </button>
          ))}
        </div>

        <label className="block">
          <span className="mb-1 block text-[10px] uppercase tracking-[0.14em] text-ink-dim">
            Trigger price ({symbol})
          </span>
          <input
            value={price}
            onChange={(e) => setPrice(e.target.value)}
            inputMode="decimal"
            placeholder={mark === undefined ? "" : fmtPrice(mark, places)}
            className="tnum w-40 rounded border border-line bg-surface px-2.5 py-1.5 text-xs outline-none focus:border-brand"
          />
        </label>

        <button
          onClick={() => void place()}
          disabled={
            busy || !signer || parsed === undefined || wrongSide || !priceUpdate
          }
          className="rounded bg-brand px-3 py-1.5 text-[11px] font-semibold text-white hover:bg-brand-dim disabled:opacity-40"
        >
          {busy ? "Confirming…" : "Place"}
        </button>
      </div>

      {wrongSide ? (
        <p className="text-[11px] text-warn">
          A {kind === TriggerKind.StopLoss ? "stop loss" : "take profit"} on a{" "}
          {position.data.direction === Direction.Long ? "long" : "short"} must
          sit {side} the current price. Placed on the other side it is already
          met, so it would fire on the next keeper pass — the program rejects it
          as <span className="tnum">TriggerAlreadyMet</span>.
        </p>
      ) : null}

      {error ? (
        <div className="space-y-1 rounded border border-short/40 bg-short/10 p-2">
          <div className="text-[11px] text-short">{error}</div>
          {logs?.length ? (
            <pre className="tnum max-h-32 overflow-auto whitespace-pre-wrap text-[10px] leading-snug text-ink-muted">
              {logs.join("\n")}
            </pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
