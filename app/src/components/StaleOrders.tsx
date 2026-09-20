import { useCallback, useEffect, useState } from "react";
import { TriggerKind } from "@solfx/client";
import type { Address } from "@solana/kit";

import { useRpc } from "@/hooks/useSolfx";
import { useSigner } from "@/hooks/useSigner";
import { useSend } from "@/hooks/useSend";
import {
  CANCEL_TRIGGER_CU,
  buildCancelTrigger,
  loadStaleTriggers,
  type StaleTrigger,
} from "@/lib/triggers";
import type { LoadedMarket } from "@/lib/markets";
import { fmtBase, fmtPrice, priceplaces } from "@/lib/format";

/**
 * Resting orders that outlived the position they were attached to.
 *
 * These have no row in the positions table — that is the whole problem with them, and why
 * this is a banner rather than a column. An orphan holds rent nobody can reclaim through the
 * normal UI, and a `predates` one is worse: it is armed against a position the trader never
 * pointed it at, and it can fire the instant that position opens.
 *
 * `cancel_trigger_order` takes only the authority and the order account, so cancelling works
 * even when the position is long gone.
 */
export function StaleOrders({
  owner,
  markets,
  refreshToken,
  onCancelled,
}: {
  readonly owner: Address | undefined;
  readonly markets: readonly LoadedMarket[];
  /** Bumped by the caller after any position change, so the list re-classifies. */
  readonly refreshToken: number;
  readonly onCancelled: () => void;
}) {
  const rpc = useRpc();
  const signer = useSigner();
  const { send, busy } = useSend();
  const [stale, setStale] = useState<StaleTrigger[]>([]);
  const [working, setWorking] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    if (!owner) {
      setStale([]);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const found = await loadStaleTriggers(rpc, owner);
        if (!cancelled) setStale(found);
      } catch {
        // A failed scan must not blank a warning the trader is acting on, but it also must
        // not invent one. Leaving the previous list is the least wrong of the three.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [rpc, owner, refreshToken, nonce]);

  const cancel = useCallback(
    async (t: StaleTrigger) => {
      if (!signer) return;
      setWorking(t.address);
      try {
        if (
          await send([buildCancelTrigger(signer, t.address)], CANCEL_TRIGGER_CU)
        ) {
          setNonce((n) => n + 1);
          onCancelled();
        }
      } finally {
        setWorking(undefined);
      }
    },
    [signer, send, onCancelled]
  );

  if (stale.length === 0) return null;

  const armed = stale.some((t) => t.reason === "predates");

  return (
    <div
      className={`mx-5 mb-3 rounded border p-3 text-[11px] ${
        armed
          ? "border-short/50 bg-short/10 text-short"
          : "border-warn/40 bg-warn/10 text-warn"
      }`}
    >
      <div className="mb-2 font-medium">
        {armed
          ? "Armed orders from a closed position. These can close a new position the moment you open it."
          : "Orders left over from a closed position. They cannot fire, but they hold rent."}
      </div>
      <p className="mb-2 opacity-80">
        A position's address is derived from its market and nonce, and the nonce
        is reused after a close, so an order left behind attaches itself to
        whatever opens next, with the old direction's meaning and the old size.
        Cancel them.
      </p>
      <div className="flex flex-col gap-1.5">
        {stale.map((t) => {
          const m = markets.find((x) => x.index === t.marketIndex);
          const symbol = m?.symbol ?? `#${t.marketIndex}`;
          return (
            <div
              key={t.address}
              className="flex items-center justify-between gap-3 rounded bg-black/20 px-2 py-1.5"
            >
              <span className="tnum">
                {symbol} · {t.kind === TriggerKind.TakeProfit ? "TP" : "SL"} @{" "}
                {fmtPrice(t.triggerPrice, priceplaces(symbol))} · size{" "}
                {fmtBase(t.sizeBase)} ·{" "}
                {t.reason === "predates"
                  ? "armed on a newer position"
                  : "position closed"}
              </span>
              <button
                onClick={() => void cancel(t)}
                disabled={busy || working === t.address || !signer}
                className="shrink-0 rounded border border-current px-2 py-0.5 hover:opacity-80 disabled:opacity-40"
              >
                {working === t.address ? "Cancelling…" : "Cancel"}
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
