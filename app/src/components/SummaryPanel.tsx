import type { HistoryTotals } from "@/lib/history";
import type { OpenPosition } from "@/lib/positions";
import { fmtUsd } from "@/lib/format";

type Props = {
  totals: HistoryTotals;
  positions: OpenPosition[];
  truncated: boolean;
  loading: boolean;
  loadError?: string | undefined;
};

/**
 * One number a trader is actually asking for, and the parts it is made of.
 *
 * Deliberately not a chart. The question "what am I up or down" is a single magnitude, and
 * the form that answers a single magnitude is a number — a sparkline or a P&L curve would be
 * a different question (how did it get there) drawn over the top of this one.
 *
 * Sign is carried by the long/short status colours the rest of the terminal already uses,
 * and always alongside an explicit `+`/`−`, so it never rests on colour alone.
 */

function Tile({
  label,
  value,
  hint,
  tone,
}: {
  label: string;
  value: string;
  hint?: string;
  tone?: "long" | "short" | undefined;
}) {
  const colour =
    tone === "long"
      ? "text-long"
      : tone === "short"
        ? "text-short"
        : "text-ink";
  return (
    <div className="rounded border border-line-soft bg-surface px-3 py-2.5">
      <div className="text-[10px] uppercase tracking-[0.1em] text-ink-dim">
        {label}
      </div>
      <div className={`tnum mt-1 text-sm font-medium ${colour}`}>{value}</div>
      {hint ? (
        <div className="mt-0.5 text-[10px] text-ink-dim">{hint}</div>
      ) : null}
    </div>
  );
}

const signed = (v: bigint) =>
  `${v >= 0n ? "+" : "−"}$${fmtUsd(v < 0n ? -v : v)}`;
const tone = (v: bigint) => (v >= 0n ? ("long" as const) : ("short" as const));

export function SummaryPanel({
  totals,
  positions,
  truncated,
  loading,
  loadError,
}: Props) {
  const unrealised = positions.reduce((sum, p) => sum + p.unrealised, 0n);
  // What the account is worth in P&L terms right now: what has already been banked, after
  // the fees that were paid to bank it, plus what is currently on the table.
  const total = totals.net + unrealised;

  const winRate =
    totals.closes === 0
      ? "—"
      : `${Math.round((totals.wins / totals.closes) * 100)}%`;

  if (loadError) {
    return (
      <div className="px-5 py-4">
        <div className="rounded border border-short/40 bg-short/10 p-2 text-[11px] text-short">
          Could not read your history, so these totals would be wrong and are
          not shown. {loadError}
        </div>
      </div>
    );
  }

  return (
    <div className="px-5 py-4">
      <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
        <div>
          <div className="text-[10px] uppercase tracking-[0.1em] text-ink-dim">
            Total P&amp;L
          </div>
          <div
            className={`tnum text-2xl font-semibold ${total >= 0n ? "text-long" : "text-short"}`}
          >
            {loading ? "…" : signed(total)}
          </div>
        </div>
        <div className="text-[11px] text-ink-dim">
          realised after fees, plus unrealised on {positions.length} open{" "}
          {positions.length === 1 ? "position" : "positions"}
        </div>
      </div>

      <div className="mt-3 grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
        <Tile
          label="Realised"
          value={loading ? "…" : signed(totals.realised)}
          tone={tone(totals.realised)}
          hint="closed trades"
        />
        <Tile
          label="Unrealised"
          value={signed(unrealised)}
          tone={tone(unrealised)}
          hint="marked at the oracle mid"
        />
        <Tile
          label="Fees paid"
          value={loading ? "…" : `−$${fmtUsd(totals.fees)}`}
          hint="open + close"
        />
        <Tile
          label="Closed"
          value={loading ? "…" : String(totals.closes)}
          hint={`${totals.wins}W / ${totals.losses}L`}
        />
        <Tile label="Win rate" value={loading ? "…" : winRate} />
        <Tile
          label="Volume"
          value={loading ? "…" : `$${fmtUsd(totals.volume)}`}
          hint="notional traded"
        />
      </div>

      {truncated ? (
        // A P&L total that quietly drops older trades is worse than one that admits it.
        <div className="mt-3 text-[10px] text-ink-dim">
          Totals cover the most recent 100 transactions on this account. Older
          trades are not included.
        </div>
      ) : null}
    </div>
  );
}
