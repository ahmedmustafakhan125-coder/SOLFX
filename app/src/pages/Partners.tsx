import { referral } from "@solfx/client";

import { Header } from "@/components/Header";
import { usePartners } from "@/hooks/usePartners";
import { RPC_URL, rpcLabel } from "@/config";
import { fmtBps, fmtUsd } from "@/lib/format";

const {
  IbTier,
  TIER_NAMES,
  entitlement,
  isCappedByPool,
  nextTierAt,
  tierForVolume,
  tierShareBps,
} = referral;

function short(a: string) {
  return `${a.slice(0, 4)}…${a.slice(-4)}`;
}

function Row({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-1.5">
      <span className="text-ink-dim">
        {label}
        {hint ? (
          <span className="ml-1 text-[10px] text-ink-muted">{hint}</span>
        ) : null}
      </span>
      <span className="tnum text-ink">{value}</span>
    </div>
  );
}

/** The tier ladder, with the viewer's position on it marked. */
function TierLadder({ volume }: { volume: bigint }) {
  const current = tierForVolume(volume);
  const tiers = [IbTier.Bronze, IbTier.Silver, IbTier.Gold, IbTier.Diamond];
  return (
    <table className="w-full text-xs">
      <thead>
        <tr className="text-left text-[10px] uppercase tracking-[0.14em] text-ink-muted">
          <th className="pb-1 font-normal">Tier</th>
          <th className="pb-1 font-normal">30-day volume</th>
          <th className="pb-1 text-right font-normal">Share of fees</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-line-soft">
        {tiers.map((t) => {
          const floor = t === IbTier.Bronze ? 0n : nextTierAt(t - 1);
          const ceiling = nextTierAt(t);
          return (
            <tr key={t} className={t === current ? "text-ink" : "text-ink-dim"}>
              <td className="py-1.5">
                {t === current ? (
                  <span className="mr-1.5 inline-block h-1.5 w-1.5 rounded-full bg-brand align-middle" />
                ) : (
                  <span className="mr-1.5 inline-block h-1.5 w-1.5 align-middle" />
                )}
                {TIER_NAMES[t]}
              </td>
              <td className="tnum py-1.5">
                {ceiling === null
                  ? `$${fmtUsd(floor ?? 0n, 0)}+`
                  : `$${fmtUsd(floor ?? 0n, 0)} – $${fmtUsd(ceiling, 0)}`}
              </td>
              <td className="tnum py-1.5 text-right">
                {fmtBps(tierShareBps(t))}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

/**
 * The introducing-broker programme.
 *
 * # Why this page is read-only right now
 *
 * `solfx-referral` is deployed on devnet but has never been initialised — it holds zero
 * accounts, so there is no config to register against and no rebate to claim. Rather than
 * render buttons that would fail, the page reports that state and shows what *is* true: the
 * referral pool inside `solfx-core` is accruing on every trade, and a trader can see exactly
 * what their own trading has generated for whoever introduced them.
 *
 * The point of the whole programme is that an IB does not have to trust the broker's ledger.
 * So every figure here is read from chain and the tier maths is recomputed locally from
 * `solfx-math`'s own rules, rather than echoed from whatever the last write said.
 */
export function Partners() {
  const { status, loading, error } = usePartners();

  const ib = status?.me?.ib ?? null;
  const tier = ib ? tierForVolume(ib.referredVolume) : IbTier.Bronze;
  const earned =
    ib && status?.programme
      ? entitlement(ib.referredPoolShare, status.programme.poolSplitBps, tier)
      : 0n;
  const capped =
    ib && status?.programme
      ? isCappedByPool(
          ib.referredPoolShare,
          status.programme.poolSplitBps,
          tier
        )
      : false;

  return (
    <div className="min-h-screen bg-surface text-ink">
      <Header rpcLabel={rpcLabel(RPC_URL)} />

      <main className="mx-auto max-w-5xl px-5 py-8">
        <div className="mb-6">
          <h1 className="text-2xl font-bold tracking-tight">Partners</h1>
          <p className="mt-1 max-w-2xl text-sm leading-relaxed text-ink-dim">
            Every FX affiliate has the same complaint: the broker controls the
            ledger. Rebates get miscounted, clients get reassigned, payouts get
            delayed, and there is no way to check any of it. Here the ledger is
            the chain, so an introducing broker can recompute every figure on
            this page from public state without trusting us.
          </p>
        </div>

        {error ? (
          <div className="mb-6 rounded border border-short/40 bg-short/10 p-3 text-sm text-short">
            Could not read the referral state: {error}
          </div>
        ) : null}

        {loading && !status ? (
          <div className="text-sm text-ink-dim">reading…</div>
        ) : null}

        {status ? (
          <div className="space-y-6">
            {/* --- the honest state of the programme on this cluster --- */}
            {!status.programme || !status.pool.authority ? (
              <div className="rounded-lg border border-warn/40 bg-warn/5 p-4 text-xs leading-relaxed">
                <div className="mb-1 font-semibold text-warn">
                  The referral programme is not live on this cluster yet.
                </div>
                <p className="text-ink-dim">
                  The <span className="text-ink">solfx-referral</span> program
                  is deployed but has not been initialised, and{" "}
                  <span className="text-ink">Protocol.referral_authority</span>{" "}
                  is still unset. That is the state the protocol launches in,
                  and the state in which it pays no rebates. Two admin
                  transactions turn it on:{" "}
                  <span className="text-ink">initialize_referral</span> on the
                  referral program, then{" "}
                  <span className="text-ink">set_referral_authority</span> on
                  the core. Registration and claiming appear here once they have
                  run.
                </p>
                <p className="mt-2 text-ink-muted">
                  The pool below is accruing regardless. Nothing is being lost:
                  the money is in the fee vault and the counters are on chain.
                </p>
              </div>
            ) : null}

            <div className="grid gap-6 md:grid-cols-2">
              {/* --- the pool, from solfx-core --- */}
              <section className="rounded-lg border border-line-soft bg-surface-high p-4">
                <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
                  The referral pool
                </h2>
                <div className="divide-y divide-line-soft text-xs">
                  <Row
                    label="Share of every fee"
                    value={fmtBps(status.pool.splitBps)}
                    hint="earmarked for referrals"
                  />
                  <Row
                    label="Accrued lifetime"
                    value={`$${fmtUsd(status.pool.totalAccrued)}`}
                  />
                  <Row
                    label="Claimed lifetime"
                    value={`$${fmtUsd(status.pool.totalClaimed)}`}
                  />
                  <Row
                    label="Unclaimed"
                    value={`$${fmtUsd(status.pool.totalAccrued - status.pool.totalClaimed)}`}
                  />
                  <Row
                    label="Referral authority"
                    value={
                      status.pool.authority
                        ? short(status.pool.authority)
                        : "not set, payouts disabled"
                    }
                  />
                  {status.programme ? (
                    <>
                      <Row
                        label="Brokers registered"
                        value={String(status.programme.ibCount)}
                      />
                      <Row
                        label="Sub-broker override"
                        value={fmtBps(status.programme.overrideBps)}
                        hint="of the parent's earnings"
                      />
                    </>
                  ) : null}
                </div>
                <p className="mt-3 text-[11px] leading-relaxed text-ink-muted">
                  A tier is a share of <em>fees</em>; the pool is a share of the
                  same fees. Where a tier would pay more than the pool holds,
                  the entitlement is capped at the pool, so an IB is never paid
                  out of treasury money.
                </p>
              </section>

              {/* --- the tier ladder --- */}
              <section className="rounded-lg border border-line-soft bg-surface-high p-4">
                <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
                  Tiers
                </h2>
                <TierLadder volume={ib?.referredVolume ?? 0n} />
                <p className="mt-3 text-[11px] leading-relaxed text-ink-muted">
                  Boundaries are inclusive at the bottom of the higher tier:
                  exactly $5M is Silver. The tier applies to everything synced
                  after you cross it, not retroactively. Recomputing history at
                  each boundary is both unaffordable on chain and the kind of
                  surprise that starts disputes.
                </p>
              </section>
            </div>

            {/* --- the connected wallet --- */}
            <section className="rounded-lg border border-line-soft bg-surface-high p-4">
              <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
                Your standing
              </h2>

              {!status.me ? (
                <p className="text-xs text-ink-dim">
                  Connect a wallet to see who introduced you, what your trading
                  has generated, and, once the programme is live, your own
                  broker account.
                </p>
              ) : (
                <div className="grid gap-6 md:grid-cols-2">
                  <div>
                    <div className="mb-2 text-[10px] uppercase tracking-[0.14em] text-ink-muted">
                      As a trader
                    </div>
                    <div className="divide-y divide-line-soft text-xs">
                      <Row
                        label="Introduced by"
                        value={
                          status.me.referrer
                            ? short(status.me.referrer)
                            : "nobody"
                        }
                        hint="bound once, never reassigned"
                      />
                      <Row
                        label="Fees you have generated"
                        value={`$${fmtUsd(status.me.feesGenerated)}`}
                        hint="for the referral pool"
                      />
                      <Row
                        label="Lifetime volume"
                        value={`$${fmtUsd(status.me.lifetimeVolume)}`}
                      />
                      <Row
                        label="30-day volume"
                        value={`$${fmtUsd(status.me.thirtyDayVolume)}`}
                      />
                    </div>
                  </div>

                  <div>
                    <div className="mb-2 text-[10px] uppercase tracking-[0.14em] text-ink-muted">
                      As a broker
                    </div>
                    {!ib ? (
                      <p className="text-xs leading-relaxed text-ink-dim">
                        This wallet has no broker account.
                        {status.programme
                          ? " Registering is permissionless: anyone may become an IB, though earning anything still requires a trader to have named you when they created their account."
                          : " Registration opens when the programme is initialised on this cluster."}
                      </p>
                    ) : (
                      <div className="divide-y divide-line-soft text-xs">
                        <Row label="Tier" value={TIER_NAMES[tier]} />
                        <Row
                          label="Referred volume"
                          value={`$${fmtUsd(ib.referredVolume)}`}
                        />
                        <Row
                          label="Traders introduced"
                          value={String(ib.referredTraderCount)}
                        />
                        <Row
                          label="Unclaimed"
                          value={`$${fmtUsd(ib.unclaimed)}`}
                          hint="on chain"
                        />
                        <Row
                          label="Recomputed entitlement"
                          value={`$${fmtUsd(earned)}${capped ? " (capped)" : ""}`}
                          hint="from public state"
                        />
                        <Row
                          label="Lifetime earned"
                          value={`$${fmtUsd(ib.lifetimeEarned)}`}
                        />
                        <Row
                          label="Lifetime claimed"
                          value={`$${fmtUsd(ib.lifetimeClaimed)}`}
                        />
                        <Row
                          label="Parent"
                          value={
                            ib.parent === "11111111111111111111111111111111"
                              ? "none"
                              : short(ib.parent)
                          }
                        />
                      </div>
                    )}
                  </div>
                </div>
              )}
            </section>
          </div>
        ) : null}
      </main>
    </div>
  );
}
