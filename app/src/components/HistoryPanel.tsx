import type { HistoryRow } from "@/lib/history";
import type { LoadedMarket } from "@/lib/markets";
import { fmtBase, fmtPrice, fmtUsd, priceplaces } from "@/lib/format";

type Props = {
  rows: readonly HistoryRow[];
  markets: LoadedMarket[];
  truncated: boolean;
  loading: boolean;
  loadError?: string | undefined;
};

const KIND_STYLE: Record<HistoryRow["kind"], string> = {
  Opened: "text-ink-muted",
  Increased: "text-ink-muted",
  Reduced: "text-ink-muted",
  Closed: "text-ink",
  // A liquidation is the one row a trader must never scroll past, so it is the one row that
  // wears a status colour for its own sake rather than for the sign of its number.
  Liquidated: "text-warn",
};

function when(ts: number): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function HistoryPanel({
  rows,
  markets,
  truncated,
  loading,
  loadError,
}: Props) {
  if (loadError) {
    return (
      <div className="mx-5 my-4 rounded border border-short/40 bg-short/10 p-2 text-[11px] text-short">
        Could not read your history — this is not the same as having none.{" "}
        {loadError}
      </div>
    );
  }

  if (loading && rows.length === 0) {
    return (
      <div className="px-5 py-4 text-xs text-ink-dim">
        Reading your history from transaction logs…
      </div>
    );
  }

  if (rows.length === 0) {
    return (
      <div className="px-5 py-4 text-xs text-ink-dim">
        No trades yet on this account.
      </div>
    );
  }

  return (
    <>
      <div className="overflow-x-auto">
        <table className="w-full min-w-[720px] text-xs">
          <thead className="text-[10px] uppercase tracking-[0.1em] text-ink-dim">
            <tr className="border-b border-line-soft">
              {["Time", "Market", "Event", "Size", "Price", "Realised", "Fee", ""].map(
                (h) => (
                  <th key={h} className="px-3 py-2 text-left font-medium">
                    {h}
                  </th>
                )
              )}
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => {
              const m = markets.find((x) => x.index === r.marketIndex);
              const symbol = m?.symbol ?? `#${r.marketIndex}`;
              const up = r.realised !== undefined && r.realised >= 0n;
              return (
                <tr
                  key={`${r.signature}-${i}`}
                  className="border-b border-line-soft/60"
                >
                  <td className="tnum px-3 py-2 text-ink-dim">{when(r.ts)}</td>
                  <td className="px-3 py-2 font-semibold">{symbol}</td>
                  <td className={`px-3 py-2 font-medium ${KIND_STYLE[r.kind]}`}>
                    {r.kind}
                  </td>
                  <td className="tnum px-3 py-2">{fmtBase(r.size)}</td>
                  <td className="tnum px-3 py-2">
                    {fmtPrice(r.price, priceplaces(symbol))}
                  </td>
                  <td
                    className={`tnum px-3 py-2 ${
                      r.realised === undefined
                        ? "text-ink-dim"
                        : up
                          ? "text-long"
                          : "text-short"
                    }`}
                  >
                    {r.realised === undefined
                      ? "—"
                      : `${up ? "+" : "−"}$${fmtUsd(r.realised < 0n ? -r.realised : r.realised)}`}
                  </td>
                  <td className="tnum px-3 py-2 text-ink-dim">
                    ${fmtUsd(r.fee)}
                  </td>
                  <td className="px-3 py-2 text-right">
                    <a
                      href={`https://explorer.solana.com/tx/${r.signature}?cluster=devnet`}
                      target="_blank"
                      rel="noreferrer"
                      className="text-[11px] text-ink-dim underline decoration-dotted hover:text-brand-soft"
                    >
                      tx
                    </a>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {truncated ? (
        <div className="px-5 py-2 text-[10px] text-ink-dim">
          Showing events from the most recent 100 transactions on this account.
        </div>
      ) : null}
    </>
  );
}
