import { useState } from "react";
import { useSearchParams } from "react-router-dom";
import type { Address } from "@solana/kit";
import { noxVerify } from "@solfx/client";

import { NoxShell } from "@/components/NoxShell";
import { useMarketplace } from "@/hooks/useMarketplace";
import { useRpc } from "@/hooks/useSolfx";
import { fmtUsd } from "@/lib/format";
import { nicknameOf } from "@/lib/nox";

const explorerTx = (sig: string) =>
  `https://explorer.solana.com/tx/${sig}?cluster=devnet`;
const explorerAddress = (a: string) =>
  `https://explorer.solana.com/address/${a}?cluster=devnet`;
const short = (a: string) => `${a.slice(0, 4)}…${a.slice(-4)}`;

const TIERS = ["Bronze", "Silver", "Gold", "Platinum"];

/** How each compared field reads to a person. The comparison itself is on the raw values. */
const FIELDS: Record<string, { label: string; show: (v: string) => string }> = {
  trades: { label: "Trades", show: (v) => v },
  wins: { label: "Wins", show: (v) => v },
  losses: { label: "Losses", show: (v) => v },
  grossProfit: {
    label: "Gross profit",
    show: (v) => `$${fmtUsd(BigInt(v), 6)}`,
  },
  grossLoss: { label: "Gross loss", show: (v) => `$${fmtUsd(BigInt(v), 6)}` },
  largestWin: {
    label: "Largest win",
    show: (v) => `$${fmtUsd(BigInt(v), 6)}`,
  },
  largestLoss: {
    label: "Largest loss",
    show: (v) => `$${fmtUsd(BigInt(v), 6)}`,
  },
  totalHoldSlots: { label: "Total hold, slots", show: (v) => v },
  untimedTrades: { label: "Untimed trades", show: (v) => v },
  ambiguousTrades: { label: "Closed together", show: (v) => v },
  maxDrawdownBps: {
    label: "Worst drawdown",
    show: (v) => `${(Number(v) / 100).toFixed(2)}%`,
  },
  mandatesFunded: { label: "Mandates funded", show: (v) => v },
  activeMandates: { label: "Mandates active", show: (v) => v },
  mandatesSettledInProfit: { label: "Settled in profit", show: (v) => v },
  tier: { label: "Tier", show: (v) => TIERS[Number(v)] ?? v },
};

type Result = {
  trader: Address;
  profile: Address;
  transactions: number;
  run: noxVerify.Rederivation;
  rows: noxVerify.Comparison[];
};

/**
 * Anyone can check a trader's record here, without trusting this site's arithmetic.
 *
 * Every figure on a `TraderProfile` is written by the program. This page fetches every
 * transaction that touched the profile, decodes the events the program emitted, folds them the
 * way the program folds them, and puts the result beside the account, field by field. The logic
 * is `noxVerify` in the published client, and `npm run verify:traders` runs the same check from a
 * terminal against any RPC endpoint.
 */
export function NoxVerify() {
  const s = useMarketplace();
  const rpc = useRpc();
  const [params, setParams] = useSearchParams();
  const [progress, setProgress] = useState<[number, number] | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState<string | undefined>();
  const m = s.market;
  const chosen = params.get("trader") ?? "";

  async function run(trader: Address) {
    const row = m?.profiles.get(trader);
    if (!row) return;
    setParams({ trader });
    setResult(null);
    setError(undefined);
    setProgress([0, 0]);
    try {
      const txs = await noxVerify.fetchProfileHistory(rpc, row.address, {
        concurrency: 3,
        onProgress: (done, total) => setProgress([done, total]),
      });
      const r = noxVerify.rederiveProfile(row.address, trader, txs);
      setResult({
        trader,
        profile: row.address,
        transactions: txs.length,
        run: r,
        rows: noxVerify.compareProfile(row.data, r.derived),
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setProgress(null);
    }
  }

  const profiles = m ? [...m.profiles.entries()] : [];
  const agreeing = result?.rows.filter((r) => r.agrees).length ?? 0;

  return (
    <NoxShell>
      <section className="px-4 pb-8 pt-16 md:px-8">
        <div className="mx-auto max-w-[1200px]">
          <div className="text-[10px] uppercase tracking-[0.22em] text-brand">
            Verify
          </div>
          <h1 className="mt-3 max-w-3xl text-3xl font-extrabold uppercase tracking-tight md:text-4xl">
            Check a trader's record yourself
          </h1>
          <p className="mt-5 max-w-2xl text-sm leading-relaxed text-ink-muted">
            Every figure on a trader's record is written by the program. This
            page fetches every transaction that touched the record, replays the
            events the program emitted, and folds them the way the program does.
            If the replay and the account disagree on anything, it says which
            field and which transaction.
          </p>
          <p className="mt-3 max-w-2xl text-[11px] leading-relaxed text-ink-dim">
            It reads the chain through this site's RPC proxy. To take the site
            out of the loop entirely, run the same check from a terminal against
            any endpoint:{" "}
            <code className="text-ink-muted">
              cd clients/js && SOLFX_RPC_URL=… npm run verify:traders
            </code>
          </p>
        </div>
      </section>

      <section className="px-4 pb-16 md:px-8">
        <div className="mx-auto grid max-w-[1200px] gap-6">
          <section className="panel">
            <header className="flex items-baseline gap-3 border-b border-line-soft px-5 py-3">
              <h2 className="text-sm font-bold uppercase tracking-wider">
                Traders
              </h2>
              <span className="tnum text-xs text-brand">{profiles.length}</span>
            </header>
            <div className="p-5">
              {!m ? (
                <p className="text-sm text-ink-dim">Reading the chain…</p>
              ) : profiles.length === 0 ? (
                <p className="text-sm text-ink-dim">
                  No trader has a record yet.
                </p>
              ) : (
                <ul className="divide-y divide-line-soft">
                  {profiles.map(([trader, row]) => {
                    const name = nicknameOf(m, trader);
                    return (
                      <li
                        key={trader}
                        className="flex flex-wrap items-center gap-x-6 gap-y-2 py-3 text-sm"
                      >
                        <a
                          href={explorerAddress(trader)}
                          target="_blank"
                          rel="noreferrer"
                          className="hover:underline"
                          title={trader}
                        >
                          {name ? (
                            <span className="font-bold text-ink">{name} </span>
                          ) : null}
                          <span className="tnum text-brand-soft">
                            {short(trader)}
                          </span>
                        </a>
                        <span className="tnum text-xs text-ink-dim">
                          {row.data.trades} trades ·{" "}
                          {TIERS[row.data.tier] ?? row.data.tier}
                        </span>
                        <button
                          type="button"
                          disabled={progress !== null}
                          onClick={() => void run(trader)}
                          className={`ml-auto px-3 py-1.5 text-[11px] font-bold uppercase tracking-[0.12em] transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                            chosen === trader
                              ? "cta-solid"
                              : "border border-line text-ink-muted hover:border-brand hover:text-brand"
                          }`}
                        >
                          Verify
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          </section>

          {progress ? (
            <p className="tnum text-sm text-ink-muted">
              {progress[1] === 0
                ? "Listing the record's transactions…"
                : `Fetched ${progress[0]} of ${progress[1]} transactions…`}
            </p>
          ) : null}
          {error ? <p className="text-sm text-short">{error}</p> : null}

          {result ? (
            <section className="panel">
              <header className="flex flex-wrap items-baseline gap-3 border-b border-line-soft px-5 py-3">
                <h2 className="text-sm font-bold uppercase tracking-wider">
                  Replay against the account
                </h2>
                <span
                  className={`tnum text-xs font-bold ${
                    agreeing === result.rows.length ? "text-long" : "text-short"
                  }`}
                >
                  {agreeing} of {result.rows.length} figures agree
                </span>
                <a
                  href={explorerAddress(result.profile)}
                  target="_blank"
                  rel="noreferrer"
                  className="tnum ml-auto text-[11px] text-ink-dim hover:underline"
                >
                  record {short(result.profile)}
                </a>
              </header>
              <div className="p-5">
                <p className="tnum text-[11px] text-ink-dim">
                  {result.transactions} transactions read ·{" "}
                  {result.run.trail.length} events moved the record ·{" "}
                  {result.run.failedSkipped} failed transactions skipped ·{" "}
                  {result.run.undecodable} events this client could not read
                </p>

                <table className="tnum mt-4 w-full text-sm">
                  <thead>
                    <tr className="text-left text-[10px] uppercase tracking-[0.14em] text-ink-dim">
                      <th className="py-2 font-normal">Figure</th>
                      <th className="py-2 text-right font-normal">On chain</th>
                      <th className="py-2 text-right font-normal">
                        Replayed from events
                      </th>
                      <th className="py-2 text-right font-normal"> </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-line-soft">
                    {result.rows.map((r) => {
                      const f = FIELDS[r.field];
                      return (
                        <tr key={r.field}>
                          <td className="py-2 text-ink-muted">
                            {f?.label ?? r.field}
                          </td>
                          <td className="py-2 text-right">
                            {f ? f.show(r.onChain) : r.onChain}
                          </td>
                          <td className="py-2 text-right">
                            {f ? f.show(r.replayed) : r.replayed}
                          </td>
                          <td
                            className={`py-2 text-right text-xs font-bold ${r.agrees ? "text-long" : "text-short"}`}
                          >
                            {r.agrees ? "agrees" : "differs"}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>

                {result.run.firstDisagreement ? (
                  <p className="mt-4 text-sm text-short">
                    The program's running {result.run.firstDisagreement.field}{" "}
                    first disagrees with the replay in{" "}
                    <a
                      href={explorerTx(result.run.firstDisagreement.signature)}
                      target="_blank"
                      rel="noreferrer"
                      className="underline"
                    >
                      {short(result.run.firstDisagreement.signature)}
                    </a>
                    : the event says {result.run.firstDisagreement.event}, the
                    replay has {result.run.firstDisagreement.replay}.
                  </p>
                ) : (
                  <p className="mt-4 text-[11px] text-ink-dim">
                    Every trade event also carries the program's running totals
                    after that trade. All of them matched the replay at the
                    point they were emitted, not only at the end.
                  </p>
                )}
                {result.run.unprovable.length > 0 ? (
                  <p className="mt-2 text-[11px] text-ink-dim">
                    {result.run.unprovable.length} mandate(s) had positions
                    close together at a profit. The program records those
                    against the trader and keeps the true total only on the
                    mandate, so whether they settled in profit cannot be proven
                    from events alone.
                  </p>
                ) : null}

                <h3 className="mt-8 text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                  The events, oldest first
                </h3>
                <ul className="mt-2 divide-y divide-line-soft text-sm">
                  {result.run.trail.map((e, i) => (
                    <li
                      key={`${e.signature}-${i}`}
                      className="flex flex-wrap items-baseline gap-x-5 gap-y-1 py-2"
                    >
                      <span className="w-40 font-bold text-ink">{e.name}</span>
                      <span className="tnum text-xs text-ink-muted">
                        {describe(e)}
                      </span>
                      <a
                        href={explorerTx(e.signature)}
                        target="_blank"
                        rel="noreferrer"
                        className="tnum ml-auto text-[11px] text-brand-soft hover:underline"
                      >
                        {e.blockTime
                          ? new Date(Number(e.blockTime) * 1000)
                              .toISOString()
                              .slice(0, 16)
                              .replace("T", " ")
                          : short(e.signature)}
                      </a>
                    </li>
                  ))}
                </ul>
              </div>
            </section>
          ) : null}
        </div>
      </section>
    </NoxShell>
  );
}

/** One line per event, in the terms the record uses. */
function describe(e: noxVerify.TrailEntry): string {
  const d = e.data;
  switch (e.name) {
    case "TradeRecorded": {
      const pnl = d.realizedPnl as bigint;
      const sign = pnl < 0n ? "-" : "+";
      return `${sign}$${fmtUsd(pnl < 0n ? -pnl : pnl, 6)} · held ${String(d.holdSlots)} slots · market ${String(d.marketIndex)}`;
    }
    case "EquityObserved":
      return `drawdown ${(Number(d.drawdownBps) / 100).toFixed(2)}% on ${short(String(d.mandate))}`;
    case "MandateFunded":
      return `$${fmtUsd(d.principal as bigint, 2)} into ${short(String(d.mandate))}`;
    case "MandateSettled":
      return `${short(String(d.mandate))} paid out $${fmtUsd(d.finalEquity as bigint, 2)}${d.wasBreached ? " after a breach" : ""}`;
    case "TierChanged":
      return `${TIERS[Number(d.from)] ?? String(d.from)} to ${TIERS[Number(d.to)] ?? String(d.to)}`;
    default:
      return "";
  }
}
