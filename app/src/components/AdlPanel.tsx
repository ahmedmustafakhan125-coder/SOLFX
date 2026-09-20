import { Direction, adlRankBps, isAdlEligible } from "@solfx/client";

import type { LoadedMarket } from "@/lib/markets";
import type { OpenPosition } from "@/lib/positions";
import { marketsWithPendingDebt, totalPendingDebt } from "@/lib/adl";
import { useInsurance } from "@/hooks/useInsurance";
import { fmtBps, fmtUsd } from "@/lib/format";

type Props = {
  positions: OpenPosition[];
  markets: LoadedMarket[];
  /** Latched by the parent so switching tabs does not re-read the fund every time. */
  active: boolean;
};

/**
 * Auto-deleveraging: what it is, whether it is happening, and where you stand in the queue.
 *
 * # Why this panel exists rather than a setting
 *
 * There is nothing to control. A trader cannot opt out of ADL, and no fee buys immunity —
 * `ARCHITECTURE.md` § 6.9 requires only that the venue *tells them it exists, in plain
 * language*, and every venue that hid it and then used it was destroyed for it. So this shows
 * the waterfall honestly: the insurance fund that absorbs a shortfall first, whether any
 * shortfall is outstanding right now, and each position's own rank.
 *
 * The one lever a trader has is the ranking itself. ADL takes the most profitable positions
 * first, measured as unrealised P&L against collateral, so realising profit lowers exposure
 * to it. That is stated rather than implied.
 */
export function AdlPanel({ positions, markets, active }: Props) {
  const { fund, error } = useInsurance(active);

  const pending = totalPendingDebt(markets);
  const affected = marketsWithPendingDebt(markets);
  const live = pending > 0n;

  // Ranked the way § 6.9 ranks candidates: uPnL as a proportion of collateral, descending.
  const ranked = [...positions]
    .map((p) => ({
      position: p,
      rank: adlRankBps(p.unrealised, p.data.collateral),
      eligible: isAdlEligible(p.unrealised),
    }))
    .sort((a, b) => (b.rank > a.rank ? 1 : b.rank < a.rank ? -1 : 0));

  return (
    <div className="space-y-4 px-5 py-4">
      <div
        className={
          "rounded border p-3 text-[11px] leading-relaxed " +
          (live
            ? "border-short/40 bg-short/10"
            : "border-line-soft bg-surface-high/40")
        }
      >
        <div
          className={"mb-1 font-semibold " + (live ? "text-short" : "text-ink")}
        >
          {live
            ? "A shortfall is outstanding. Deleveraging is live."
            : "No shortfall outstanding. Nothing is being deleveraged."}
        </div>
        <p className="text-ink-dim">
          When a position gaps through its liquidation price, the loss can
          exceed the collateral behind it. The insurance fund covers that first.
          If it cannot, the most profitable positions on the other side are
          force-closed and part of their{" "}
          <span className="text-ink">profit</span> is withheld to cover the
          rest, never their collateral. Whatever remains is absorbed by the
          liquidity pool.
        </p>
        <p className="mt-2 text-ink-muted">
          You cannot opt out, and there is no fee that avoids it. The worst case
          for a deleveraged position is being returned to flat: principal comes
          back in full.
        </p>
      </div>

      <div className="grid gap-x-8 gap-y-1 text-xs sm:grid-cols-2">
        <div className="flex items-baseline justify-between border-b border-line-soft py-1.5">
          <span className="text-ink-dim">Insurance fund</span>
          <span className="tnum">
            {error ? "—" : fund ? `$${fmtUsd(fund.balance)}` : "reading…"}
          </span>
        </div>
        <div className="flex items-baseline justify-between border-b border-line-soft py-1.5">
          <span className="text-ink-dim">Fund target</span>
          <span className="tnum">
            {fund ? `$${fmtUsd(fund.targetBalance)}` : "—"}
          </span>
        </div>
        <div className="flex items-baseline justify-between border-b border-line-soft py-1.5">
          <span className="text-ink-dim">Bad debt it has absorbed</span>
          <span className="tnum">
            {fund ? `$${fmtUsd(fund.totalBadDebtCovered)}` : "—"}
          </span>
        </div>
        <div className="flex items-baseline justify-between border-b border-line-soft py-1.5">
          <span className="text-ink-dim">Awaiting deleveraging</span>
          <span className={"tnum " + (live ? "text-short" : "")}>
            ${fmtUsd(pending)}
          </span>
        </div>
      </div>

      {error ? (
        <div className="rounded border border-short/40 bg-short/10 p-2 text-[11px] text-short">
          Could not read the insurance fund: {error}
        </div>
      ) : null}

      {affected.length > 0 ? (
        <div className="text-xs">
          <div className="mb-1 text-[10px] uppercase tracking-[0.14em] text-ink-dim">
            Markets with a shortfall
          </div>
          {affected.map((m) => (
            <div
              key={m.index}
              className="flex items-baseline justify-between border-b border-line-soft py-1.5"
            >
              <span>{m.symbol}</span>
              <span className="tnum text-short">
                ${fmtUsd(m.data.pendingAdlDebt)}
              </span>
            </div>
          ))}
        </div>
      ) : null}

      <div>
        <div className="mb-1 text-[10px] uppercase tracking-[0.14em] text-ink-dim">
          Your positions, in queue order
        </div>
        {ranked.length === 0 ? (
          <p className="text-xs text-ink-dim">
            No open positions, so nothing of yours can be deleveraged.
          </p>
        ) : (
          <table className="w-full text-xs">
            <thead>
              <tr className="text-left text-[10px] uppercase tracking-[0.14em] text-ink-muted">
                <th className="pb-1 font-normal">Market</th>
                <th className="pb-1 font-normal">Side</th>
                <th className="pb-1 text-right font-normal">Unrealised</th>
                <th className="pb-1 text-right font-normal">
                  Profit vs collateral
                </th>
                <th className="pb-1 text-right font-normal">In the queue</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line-soft">
              {ranked.map(({ position: p, rank, eligible }) => {
                const m = markets.find((x) => x.index === p.marketIndex);
                return (
                  <tr key={p.address}>
                    <td className="py-1.5">
                      {m ? m.symbol : `#${p.marketIndex}`}
                    </td>
                    <td className="py-1.5">
                      {p.data.direction === Direction.Long ? "Long" : "Short"}
                    </td>
                    <td
                      className={
                        "tnum py-1.5 text-right " +
                        (p.unrealised >= 0n ? "text-long" : "text-short")
                      }
                    >
                      {p.unrealised >= 0n ? "+" : "−"}$
                      {fmtUsd(p.unrealised < 0n ? -p.unrealised : p.unrealised)}
                    </td>
                    <td className="tnum py-1.5 text-right">
                      {fmtBps(Number(rank))}
                    </td>
                    <td className="py-1.5 text-right">
                      {eligible ? (
                        <span className="text-warn">eligible</span>
                      ) : (
                        <span className="text-ink-muted">not in profit</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
        <p className="mt-2 text-[11px] leading-relaxed text-ink-muted">
          Only a position in profit can be deleveraged at all. The program
          refuses anything else with{" "}
          <span className="text-ink-dim">NotAdlEligible</span>. Among those, the
          highest profit-to-collateral goes first, which is why a small position
          with a large proportional gain ranks ahead of a big one with a small
          percentage.
        </p>
      </div>
    </div>
  );
}
