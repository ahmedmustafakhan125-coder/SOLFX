import { Fragment, useState } from "react";
import {
  Direction,
  liquidationPrice,
  liquidationPriceIsExact,
  maintenanceMargin,
  pipsTenths,
} from "@solfx/client";
import type { Address } from "@solana/kit";

import type { LoadedMarket } from "@/lib/markets";
import type { OpenPosition } from "@/lib/positions";
import { CLOSE_POSITION_CU, buildClosePosition } from "@/lib/positions";
import { TriggerPanel } from "@/components/TriggerPanel";
import { useSend } from "@/hooks/useSend";
import { useSigner } from "@/hooks/useSigner";
import { fmtBase, fmtPrice, fmtUsd, priceplaces } from "@/lib/format";

type Props = {
  positions: OpenPosition[];
  markets: LoadedMarket[];
  prices: Record<number, bigint | undefined>;
  priceAccounts: Record<number, Address | undefined>;
  onClosed: () => void;
};

export function PositionsPanel({
  positions,
  markets,
  prices,
  priceAccounts,
  onClosed,
}: Props) {
  const signer = useSigner();
  const { send, busy, error, logs, reset } = useSend();
  const [closing, setClosing] = useState<string | undefined>(undefined);
  const [expanded, setExpanded] = useState<string | undefined>(undefined);

  async function close(p: OpenPosition) {
    const price = prices[p.marketIndex];
    const priceUpdate = priceAccounts[p.marketIndex];
    if (!signer || price === undefined || !priceUpdate) return;
    reset();
    setClosing(p.address);
    try {
      const ix = await buildClosePosition({
        signer,
        marketIndex: p.marketIndex,
        nonce: p.nonce,
        direction: p.data.direction,
        price,
        slippageBps: 100,
        priceUpdate,
      });
      if (await send([ix], CLOSE_POSITION_CU)) onClosed();
    } finally {
      setClosing(undefined);
    }
  }

  return (
    <div className="border-t border-line-soft">
      <div className="flex items-baseline gap-3 px-5 py-2.5">
        <span className="text-xs font-medium">
          Open Positions ({positions.length})
        </span>
        <span className="text-[10px] text-ink-dim">
          unrealised is marked at the oracle mid; a close fills after the spread
        </span>
      </div>

      {positions.length === 0 ? (
        <div className="px-5 pb-4 text-xs text-ink-dim">No open positions.</div>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[720px] text-xs">
            <thead className="text-[10px] uppercase tracking-[0.1em] text-ink-dim">
              <tr className="border-b border-line-soft">
                {[
                  "Market",
                  "Side",
                  "Size",
                  "Entry",
                  "Mark",
                  "Collateral",
                  "Unrealised",
                  "",
                ].map((h) => (
                  <th key={h} className="px-3 py-2 text-left font-medium">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {positions.map((p) => {
                const m = markets.find((x) => x.index === p.marketIndex);
                const symbol = m?.symbol ?? `#${p.marketIndex}`;
                const places = priceplaces(symbol);
                const mark = prices[p.marketIndex];
                const long = p.data.direction === Direction.Long;
                const up = p.unrealised >= 0n;

                // The number a trader actually watches. Computed with zero accrued costs,
                // which is the optimistic end: carry and funding only ever pull it toward
                // entry, so the figure shown is never closer than the truth.
                const liq =
                  m === undefined
                    ? undefined
                    : liquidationPrice({
                        sizeBase: p.data.sizeBase,
                        entryPrice: p.data.entryPrice,
                        collateral: p.data.collateral,
                        maintenanceMargin: maintenanceMargin(
                          p.notionalNow,
                          m.data.mmrBps
                        ),
                        costs: { carry: 0n, funding: 0n, closeFee: 0n },
                        direction: p.data.direction,
                      });
                const liqExact = liquidationPriceIsExact(symbol);

                // Move in the trader's favour, in tenths of a pip. Brokers quote P&L this way
                // and a trader compares it against the one they already use.
                const move =
                  mark === undefined
                    ? undefined
                    : (mark - p.data.entryPrice) * (long ? 1n : -1n);
                const tenths =
                  move === undefined ? undefined : pipsTenths(symbol, move);
                return (
                  <Fragment key={p.address}>
                    <tr className="border-b border-line-soft/60">
                      <td className="px-3 py-2 font-semibold">{symbol}</td>
                      <td
                        className={`px-3 py-2 font-medium ${long ? "text-long" : "text-short"}`}
                      >
                        {long ? "Long" : "Short"}
                      </td>
                      <td className="tnum px-3 py-2">
                        {fmtBase(p.data.sizeBase)}
                      </td>
                      <td className="tnum px-3 py-2">
                        {fmtPrice(p.data.entryPrice, places)}
                      </td>
                      <td className="tnum px-3 py-2">
                        {mark === undefined ? "—" : fmtPrice(mark, places)}
                      </td>
                      <td
                        className="tnum px-3 py-2 text-warn"
                        title={
                          liqExact
                            ? "Before accrued carry and funding, which move it toward entry."
                            : `${symbol} is not USD-quoted, so its liquidation price also moves with the conversion rate. This holds that rate constant.`
                        }
                      >
                        {liq === undefined ? "—" : fmtPrice(liq, places)}
                        {liq !== undefined && !liqExact ? "*" : ""}
                      </td>
                      <td className="tnum px-3 py-2">
                        ${fmtUsd(p.data.collateral)}
                      </td>
                      <td
                        className={`tnum px-3 py-2 ${up ? "text-long" : "text-short"}`}
                      >
                        <div>
                          {up ? "+" : ""}${fmtUsd(p.unrealised)}
                        </div>
                        {tenths === undefined ? null : (
                          <div className="text-[10px] opacity-80">
                            {tenths >= 0n ? "+" : "\u2212"}
                            {(tenths < 0n ? -tenths : tenths) / 10n}.
                            {(tenths < 0n ? -tenths : tenths) % 10n} pips
                          </div>
                        )}
                      </td>
                      <td className="px-3 py-2 text-right">
                        <div className="flex justify-end gap-1.5">
                          <button
                            onClick={() =>
                              setExpanded(
                                expanded === p.address ? undefined : p.address
                              )
                            }
                            className="rounded border border-line px-2.5 py-1 text-[11px] hover:border-brand"
                          >
                            {expanded === p.address ? "Hide SL/TP" : "SL/TP"}
                          </button>
                          <button
                            onClick={() => void close(p)}
                            disabled={busy || !signer || mark === undefined}
                            className="rounded border border-line px-2.5 py-1 text-[11px] hover:border-brand disabled:opacity-40"
                          >
                            {closing === p.address ? "Closing…" : "Close"}
                          </button>
                        </div>
                      </td>
                    </tr>
                    {expanded === p.address ? (
                      <tr className="border-b border-line-soft/60">
                        <td colSpan={9} className="p-0">
                          <TriggerPanel
                            position={p}
                            symbol={symbol}
                            places={places}
                            mark={mark}
                            priceUpdate={priceAccounts[p.marketIndex]}
                          />
                        </td>
                      </tr>
                    ) : null}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {error ? (
        <div className="mx-5 mb-4 space-y-1 rounded border border-short/40 bg-short/10 p-2">
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
