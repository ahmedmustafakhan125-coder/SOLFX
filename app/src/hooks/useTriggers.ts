import { useCallback, useEffect, useState } from "react";
import type { Address } from "@solana/kit";

import { readTriggers, type RestingTrigger } from "@/lib/triggers";
import { useRpc } from "@/hooks/useSolfx";

/** A resting order, tagged with the position it belongs to. */
export type PositionTrigger = RestingTrigger & {
  readonly position: Address;
  readonly marketIndex: number;
};

/**
 * Every resting take-profit and stop-loss on the given positions.
 *
 * The panel that places these already reads them per position when a row is expanded. The
 * chart needs them without anything being expanded: a stop-loss that is not drawn is a
 * stop-loss the trader has to remember, and the whole point of resting it on chain is that
 * they should not have to.
 *
 * Polled on the same cadence as positions, because a trigger disappears when it fires and a
 * line left on the chart for an order that no longer exists is worse than no line.
 */
export function useTriggers(
  positions: readonly {
    readonly address: Address;
    readonly marketIndex: number;
  }[],
  pollMs = 10_000
) {
  const rpc = useRpc();
  const [triggers, setTriggers] = useState<PositionTrigger[]>([]);
  const [nonce, setNonce] = useState(0);

  // The dependency is the address list, not the array identity: `positions` is rebuilt on
  // every price tick and depending on it directly would re-read the chain eight times a
  // minute for nothing.
  const key = positions.map((p) => `${p.address}:${p.marketIndex}`).join(",");

  useEffect(() => {
    if (positions.length === 0) {
      setTriggers([]);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const found = await Promise.all(
          positions.map(async (p) => {
            const rows = await readTriggers(rpc, p.address);
            return rows.map((t) => ({
              ...t,
              position: p.address,
              marketIndex: p.marketIndex,
            }));
          })
        );
        if (!cancelled) setTriggers(found.flat());
      } catch {
        // A chart without order lines is degraded, not broken, and `TriggerPanel` reports
        // its own read failures where a trader is actually managing the order. Blanking the
        // lines is the honest response: better no line than a line from a stale read.
        if (!cancelled) setTriggers([]);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rpc, key, nonce]);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    if (positions.length === 0) return;
    const id = setInterval(refresh, pollMs);
    return () => clearInterval(id);
  }, [positions.length, pollMs, refresh]);

  return { triggers, refresh };
}
