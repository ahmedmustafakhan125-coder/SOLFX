import { useMemo, useState } from "react";
import type { Address, Instruction, TransactionSigner } from "@solana/kit";
import { nox } from "@solfx/client";

import { NoxShell } from "@/components/NoxShell";
import { useMarketplace } from "@/hooks/useMarketplace";
import { useSend } from "@/hooks/useSend";
import { useSigner } from "@/hooks/useSigner";
import { fmtUsd } from "@/lib/format";
import type { LoadedMarket } from "@/lib/markets";
import {
  acceptOfferIx,
  closeRequestIx,
  createProfileIx,
  declineOfferIx,
  fmtFactorBps,
  fmtPctBps,
  fmtSlots,
  MARKET_CU,
  claimSettlementIxs,
  MANDATE_STATE,
  marketBitmap,
  marketIndices,
  nextSeq,
  noteText,
  OFFER_STATE_NAME,
  parseUsdc,
  postInvestorListingIx,
  postOfferIx,
  postRequestIx,
  postTraderListingIx,
  previewSplit,
  requestSettlementIx,
  revokeOfferIx,
  TIER_NAME,
  traderStats,
  type Marketplace,
  type OfferRules,
  type TraderStats,
} from "@/lib/nox";

const explorer = (a: string) =>
  `https://explorer.solana.com/address/${a}?cluster=devnet`;
const short = (a: string) => `${a.slice(0, 4)}…${a.slice(-4)}`;

/**
 * What each tier allows, from `TraderTier::max_mandate` in `programs/noxfunds/src/state.rs`.
 * Shown so an investor sees the ceiling before the program refuses it, not after.
 */
const TIER_MAX_MANDATE = [
  10_000_000_000n,
  25_000_000_000n,
  75_000_000_000n,
  200_000_000_000n,
] as const;

/**
 * An Anchor program answers an instruction it does not have with InstructionFallbackNotFound
 * (101, 0x65). Here that means one thing: the marketplace upgrade is not on this cluster yet.
 */
function explain(error: string | undefined): string | undefined {
  if (!error) return undefined;
  if (/InstructionFallbackNotFound|custom program error: 0x65\b/.test(error)) {
    return "The marketplace instructions are not deployed on this cluster yet — the NOXFUNDS upgrade is pending.";
  }
  return error;
}

// --- atoms --------------------------------------------------------------------------------

function Panel({
  title,
  hint,
  count,
  children,
}: {
  title: string;
  hint?: string;
  count?: number;
  children: React.ReactNode;
}) {
  return (
    <section className="panel">
      <header className="flex items-baseline gap-3 border-b border-line-soft px-5 py-3">
        <h2 className="text-sm font-bold uppercase tracking-wider">{title}</h2>
        {count !== undefined ? (
          <span className="tnum text-xs text-brand">{count}</span>
        ) : null}
        {hint ? (
          <span className="ml-auto text-[11px] text-ink-dim">{hint}</span>
        ) : null}
      </header>
      <div className="p-5">{children}</div>
    </section>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="text-sm text-ink-dim">{children}</p>;
}

function Who({ address }: { address: string }) {
  return (
    <a
      href={explorer(address)}
      target="_blank"
      rel="noreferrer"
      className="tnum text-brand-soft hover:underline"
      title={address}
    >
      {short(address)}
    </a>
  );
}

function Btn({
  children,
  onClick,
  disabled,
  kind = "solid",
}: {
  children: React.ReactNode;
  onClick: () => void;
  disabled?: boolean;
  kind?: "solid" | "line" | "danger";
}) {
  const style =
    kind === "solid"
      ? "cta-solid"
      : kind === "danger"
        ? "border border-short/60 text-short hover:bg-short/10"
        : "border border-line text-ink-muted hover:border-brand hover:text-brand";
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={`${style} px-3 py-1.5 text-[11px] font-bold uppercase tracking-[0.12em] transition-colors disabled:cursor-not-allowed disabled:opacity-40`}
    >
      {children}
    </button>
  );
}

function Input({
  label,
  value,
  onChange,
  suffix,
  placeholder,
  wide,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  suffix?: string;
  placeholder?: string;
  wide?: boolean;
}) {
  return (
    <label className={`block ${wide ? "sm:col-span-2" : ""}`}>
      <span className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
        {label}
      </span>
      <span className="mt-1 flex items-center border border-line bg-bg focus-within:border-brand">
        <input
          value={value}
          placeholder={placeholder}
          onChange={(e) => onChange(e.target.value)}
          className="tnum w-full bg-transparent px-3 py-2 text-sm text-ink outline-none"
        />
        {suffix ? (
          <span className="px-3 text-xs text-ink-dim">{suffix}</span>
        ) : null}
      </span>
    </label>
  );
}

function MarketPicker({
  markets,
  value,
  onChange,
}: {
  markets: readonly LoadedMarket[];
  value: readonly number[];
  onChange: (v: number[]) => void;
}) {
  if (markets.length === 0) {
    return <p className="text-xs text-ink-dim">Loading markets…</p>;
  }
  return (
    <div className="sm:col-span-2">
      <span className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
        Permitted markets
      </span>
      <div className="mt-1 flex flex-wrap gap-1.5">
        {markets.map((m) => {
          const on = value.includes(m.index);
          return (
            <button
              key={m.index}
              type="button"
              onClick={() =>
                onChange(
                  on ? value.filter((i) => i !== m.index) : [...value, m.index]
                )
              }
              className={`border px-2 py-1 text-[11px] transition-colors ${
                on
                  ? "border-brand bg-brand/10 text-brand"
                  : "border-line text-ink-dim hover:text-ink"
              }`}
            >
              {m.symbol}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function marketNames(bitmap: bigint, markets: readonly LoadedMarket[]): string {
  const names = marketIndices(bitmap).map(
    (i) => markets.find((m) => m.index === i)?.symbol ?? `#${i}`
  );
  return names.length ? names.join(", ") : "—";
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="bg-bg p-3">
      <div className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
        {label}
      </div>
      <div className="tnum mt-1 text-sm font-bold text-ink">{value}</div>
    </div>
  );
}

function RecordStrip({ stats }: { stats: TraderStats }) {
  return (
    <div className="grid grid-cols-2 gap-px border border-line bg-line sm:grid-cols-4 lg:grid-cols-7">
      <Stat label="Tier" value={TIER_NAME[stats.tier]} />
      <Stat label="Trades" value={String(stats.trades)} />
      <Stat label="Win rate" value={fmtPctBps(stats.winRateBps)} />
      <Stat label="Profit factor" value={fmtFactorBps(stats.profitFactorBps)} />
      <Stat label="Worst drawdown" value={fmtPctBps(stats.maxDrawdownBps)} />
      <Stat label="Avg hold" value={fmtSlots(stats.avgHoldSlots)} />
      <Stat label="Settled in profit" value={String(stats.settledInProfit)} />
    </div>
  );
}

// --- the page -------------------------------------------------------------------------------

/**
 * The NOXFUNDS marketplace: one page, two sides.
 *
 * An investor sees traders — their records, which the traders cannot edit — and can make an
 * escrowed offer. A trader sees investors who have listed, can ask them for capital, and sees
 * the offers addressed to them, which they accept or decline with a reason.
 *
 * Every number on the page is read from an account the program maintains. There is no
 * server of ours between the chain and this table.
 */
/** The words at the top of each of the three pages. */
const HEADINGS: Record<
  Mode,
  { eyebrow: string; title: React.ReactNode; blurb: string }
> = {
  market: {
    eyebrow: "Marketplace",
    title: (
      <>
        Find each other. Agree terms.{" "}
        <span className="text-brand">Escrow does the rest.</span>
      </>
    ),
    blurb:
      "There is no chat, on purpose. Every message here is a typed object on chain tied to a real step: an offer escrows the capital behind its terms, a refusal carries its reason, and a request can only reach an investor who has said they are looking. The terms are the message.",
  },
  investor: {
    eyebrow: "Investor",
    title: (
      <>
        Your capital, your rules,{" "}
        <span className="text-brand">and your money back.</span>
      </>
    ),
    blurb:
      "Everything you have offered and every mandate you have funded. Once a trader accepts, the capital sits in that mandate's own vault — not with the trader, not with us — and only settlement moves it.",
  },
  trader: {
    eyebrow: "Trader",
    title: (
      <>
        Your record, and the{" "}
        <span className="text-brand">capital it earns.</span>
      </>
    ),
    blurb:
      "Your listing, the offers addressed to you, and the mandates you trade. The record is the program's, not yours: every trade from every mandate lands on it, and there is no second account to start fresh on.",
  },
};

function NoxPage({ mode }: { mode: Mode }) {
  const s = useMarketplace();
  const signer = useSigner();
  const tx = useSend();
  const heading = HEADINGS[mode];

  const [buildError, setBuildError] = useState<string | undefined>(undefined);

  /**
   * Build, send, refresh — one path for every action on the page.
   *
   * The signer handed to each builder is the one this page holds, which is the same cached
   * instance `useSend` signs with. Two signer instances for one address and kit refuses the
   * transaction, so builders must never obtain their own.
   */
  async function act(
    build: (signer: TransactionSigner) => Promise<Instruction[]> | Instruction[]
  ) {
    setBuildError(undefined);
    if (!signer) {
      setBuildError("Connect a wallet first.");
      return;
    }
    try {
      const ixs = await build(signer);
      if (await tx.send(ixs, MARKET_CU)) s.refresh();
    } catch (e) {
      // A builder can throw before anything is sent — a bad seq, a missing account.
      setBuildError(e instanceof Error ? e.message : String(e));
    }
  }

  const m = s.market;

  return (
    <NoxShell>
      <section className="px-4 pb-8 pt-16 md:px-8">
        <div className="mx-auto max-w-[1200px]">
          <div className="text-[10px] uppercase tracking-[0.22em] text-brand">
            {heading.eyebrow}
          </div>
          <h1 className="mt-3 max-w-3xl text-3xl font-extrabold uppercase tracking-tight md:text-4xl">
            {heading.title}
          </h1>
          <p className="mt-5 max-w-2xl text-sm leading-relaxed text-ink-muted">
            {heading.blurb}
          </p>

          <div className="mt-8 flex flex-wrap items-center gap-4">
            {m ? (
              <span className="tnum text-xs text-ink-dim">
                {m.profiles.size} traders · {m.investorListings.length}{" "}
                investors listed ·{" "}
                {
                  m.offers.filter((o) => o.data.state === nox.OfferState.Open)
                    .length
                }{" "}
                open offers
              </span>
            ) : null}
            <button
              onClick={s.refresh}
              className="ml-auto text-[11px] uppercase tracking-[0.14em] text-ink-dim hover:text-brand"
            >
              {s.loading ? "Reading chain…" : "Refresh"}
            </button>
          </div>

          {!s.me ? (
            <p className="mt-6 border border-brand/30 bg-brand/5 p-4 text-sm text-brand-soft">
              Read-only until you connect a wallet. Everything below is public;
              acting on it needs a signature.
            </p>
          ) : null}
          {s.error ? (
            <p className="mt-6 border border-short/40 bg-short/5 p-4 text-sm text-short">
              Could not read the marketplace: {s.error}
            </p>
          ) : null}
        </div>
      </section>

      <section className="px-4 pb-24 md:px-8">
        <div className="mx-auto grid max-w-[1200px] gap-6">
          {m ? (
            <>
              {/* The marketplace shows both tables; a dashboard shows one side's own business. */}
              {mode !== "trader" ? (
                <InvestorView
                  mode={mode}
                  m={m}
                  s={s}
                  signer={!!signer}
                  act={act}
                  busy={tx.busy}
                />
              ) : null}
              {mode !== "investor" ? (
                <TraderView
                  mode={mode}
                  m={m}
                  s={s}
                  signer={!!signer}
                  act={act}
                  busy={tx.busy}
                />
              ) : null}
            </>
          ) : s.loading ? (
            <Empty>Reading the marketplace from chain…</Empty>
          ) : null}
        </div>
      </section>

      {buildError ? (
        <div className="fixed bottom-4 left-4 z-50 max-w-md border border-short/50 bg-surface p-4 text-xs text-short shadow-2xl">
          {buildError}
          <button
            onClick={() => setBuildError(undefined)}
            className="mt-2 block text-[10px] uppercase tracking-wider text-ink-dim hover:text-ink"
          >
            Dismiss
          </button>
        </div>
      ) : null}
      {tx.busy || tx.error || tx.signature ? (
        <div className="fixed bottom-4 right-4 z-50 max-w-md border border-line bg-surface p-4 text-xs shadow-2xl">
          {tx.busy ? (
            <span className="text-ink-muted">
              Waiting for signature and confirmation…
            </span>
          ) : tx.error ? (
            <div>
              <div className="font-bold text-short">Refused</div>
              <div className="mt-1 text-ink-muted">{explain(tx.error)}</div>
              {tx.logs?.length ? (
                <details className="mt-2">
                  <summary className="cursor-pointer text-ink-dim">
                    Program logs
                  </summary>
                  <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap text-[10px] text-ink-dim">
                    {tx.logs.join("\n")}
                  </pre>
                </details>
              ) : null}
            </div>
          ) : (
            <div>
              <span className="font-bold text-long">Confirmed </span>
              <a
                href={`https://explorer.solana.com/tx/${tx.signature}?cluster=devnet`}
                target="_blank"
                rel="noreferrer"
                className="tnum text-brand-soft hover:underline"
              >
                {tx.signature?.slice(0, 16)}…
              </a>
            </div>
          )}
          {!tx.busy ? (
            <button
              onClick={tx.reset}
              className="mt-2 block text-[10px] uppercase tracking-wider text-ink-dim hover:text-ink"
            >
              Dismiss
            </button>
          ) : null}
        </div>
      ) : null}
    </NoxShell>
  );
}

type Act = (
  build: (signer: TransactionSigner) => Promise<Instruction[]> | Instruction[]
) => Promise<void>;

type Mode = "market" | "investor" | "trader";

/** Discovery: who is looking, on both sides. */
export function NoxMarket() {
  return <NoxPage mode="market" />;
}

/** One investor's own offers, mandates and listing. */
export function NoxInvestor() {
  return <NoxPage mode="investor" />;
}

/** One trader's record, offers received, and the mandates they trade. */
export function NoxTrader() {
  return <NoxPage mode="trader" />;
}

type ViewProps = {
  mode: Mode;
  m: Marketplace;
  s: ReturnType<typeof useMarketplace>;
  signer: boolean;
  act: Act;
  busy: boolean;
};

// --- investor --------------------------------------------------------------------------------

type SortKey = "profit" | "win" | "dd" | "trades" | "hold";

function InvestorView({ m, s, signer, act, busy, mode }: ViewProps) {
  // The traders table is discovery and belongs on the marketplace; everything else is this
  // investor's own business and belongs on their dashboard.
  const discovery = mode === "market";
  const own = mode === "investor";
  const me = s.me;
  const [sort, setSort] = useState<SortKey>("profit");
  const [target, setTarget] = useState<Address | undefined>(undefined);

  const traders = useMemo(() => {
    const listings = new Map(m.traderListings.map((l) => [l.data.trader, l]));
    const rows = [...m.profiles.values()].map((p) => ({
      trader: p.data.authority,
      stats: traderStats(p.data),
      listing: listings.get(p.data.authority),
    }));
    // Nulls last in every ordering: "not computable" is not the same as "worst".
    const key = (r: (typeof rows)[number]): bigint | null => {
      switch (sort) {
        case "profit":
          return r.stats.profitFactorBps;
        case "win":
          return r.stats.winRateBps;
        case "dd":
          return -BigInt(r.stats.maxDrawdownBps);
        case "trades":
          return BigInt(r.stats.trades);
        case "hold":
          return r.stats.avgHoldSlots;
      }
    };
    return rows.sort((a, b) => {
      const ka = key(a);
      const kb = key(b);
      if (ka === null) return kb === null ? 0 : 1;
      if (kb === null) return -1;
      return kb > ka ? 1 : kb < ka ? -1 : 0;
    });
  }, [m, sort]);

  const mine = m.offers.filter((o) => o.data.investor === me);
  const inbox = m.requests.filter((r) => r.data.investor === me);
  const myListing = m.investorListings.find((l) => l.data.investor === me);

  const SortBtn = ({ k, label }: { k: SortKey; label: string }) => (
    <button
      onClick={() => setSort(k)}
      className={`uppercase tracking-[0.12em] ${sort === k ? "text-brand" : "hover:text-ink"}`}
    >
      {label}
      {sort === k ? " ↓" : ""}
    </button>
  );

  return (
    <>
      {discovery && (
        <Panel
          title="Traders"
          count={traders.length}
          hint="Records are the program's, not the trader's — nobody can edit them"
        >
          {traders.length === 0 ? (
            <Empty>No trader has a profile on this cluster yet.</Empty>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[900px] text-sm">
                <thead>
                  <tr className="text-left text-[10px] text-ink-dim">
                    <th className="pb-3 font-medium uppercase tracking-[0.12em]">
                      Trader
                    </th>
                    <th className="pb-3 font-medium uppercase tracking-[0.12em]">
                      Tier
                    </th>
                    <th className="pb-3 text-right font-medium">
                      <SortBtn k="trades" label="Trades" />
                    </th>
                    <th className="pb-3 text-right font-medium">
                      <SortBtn k="win" label="Win rate" />
                    </th>
                    <th className="pb-3 text-right font-medium">
                      <SortBtn k="profit" label="Profit factor" />
                    </th>
                    <th className="pb-3 text-right font-medium">
                      <SortBtn k="dd" label="Worst DD" />
                    </th>
                    <th className="pb-3 text-right font-medium">
                      <SortBtn k="hold" label="Avg hold" />
                    </th>
                    <th className="pb-3 pl-4 font-medium uppercase tracking-[0.12em]">
                      Asking
                    </th>
                    <th />
                  </tr>
                </thead>
                <tbody className="tnum">
                  {traders.map((r) => (
                    <tr
                      key={r.trader}
                      className="border-t border-line-soft align-top"
                    >
                      <td className="py-3">
                        <Who address={r.trader} />
                      </td>
                      <td className="py-3 text-ink-muted">
                        {TIER_NAME[r.stats.tier]}
                      </td>
                      <td className="py-3 text-right">{r.stats.trades}</td>
                      <td className="py-3 text-right">
                        {fmtPctBps(r.stats.winRateBps)}
                      </td>
                      <td className="py-3 text-right">
                        {fmtFactorBps(r.stats.profitFactorBps)}
                      </td>
                      <td className="py-3 text-right">
                        {fmtPctBps(r.stats.maxDrawdownBps)}
                      </td>
                      <td className="py-3 text-right">
                        {fmtSlots(r.stats.avgHoldSlots)}
                      </td>
                      <td className="py-3 pl-4 font-sans text-xs text-ink-muted">
                        {r.listing && r.listing.data.open ? (
                          <>
                            ${fmtUsd(r.listing.data.minPrincipal, 0)}–$
                            {fmtUsd(r.listing.data.maxPrincipal, 0)} ·{" "}
                            {fmtPctBps(r.listing.data.wantedSplitBps)} split
                            <div className="mt-1 max-w-[16rem] text-ink-dim">
                              {noteText(
                                r.listing.data.note,
                                r.listing.data.noteLen
                              )}
                            </div>
                          </>
                        ) : (
                          <span className="text-ink-dim">Not listed</span>
                        )}
                      </td>
                      <td className="py-3 text-right">
                        <Btn
                          kind={target === r.trader ? "line" : "solid"}
                          disabled={!signer || r.trader === me}
                          onClick={() =>
                            setTarget(
                              target === r.trader ? undefined : r.trader
                            )
                          }
                        >
                          {target === r.trader ? "Close" : "Make offer"}
                        </Btn>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          {target ? (
            <OfferForm
              key={target}
              m={m}
              s={s}
              trader={target}
              // What this trader asked you for, if they sent a request. Offering anything else by
              // default is how a $1,000 offer went out from a wallet holding $500 and was refused.
              requested={
                m.requests.find(
                  (r) => r.data.trader === target && r.data.investor === me
                )?.data.wantedPrincipal
              }
              act={act}
              busy={busy}
              onDone={() => setTarget(undefined)}
            />
          ) : null}
        </Panel>
      )}

      {own && (
        <div className="grid gap-6 lg:grid-cols-2">
          <Panel title="Your offers" count={mine.length}>
            {!me ? (
              <Empty>Connect a wallet to see offers you have made.</Empty>
            ) : mine.length === 0 ? (
              <Empty>You have not made an offer.</Empty>
            ) : (
              <ul className="space-y-3">
                {mine.map((o) => {
                  const st = o.data.state;
                  const reply = noteText(o.data.reply, o.data.replyLen);
                  return (
                    <li
                      key={o.address}
                      className="border border-line-soft p-3 text-sm"
                    >
                      <div className="flex flex-wrap items-center gap-3">
                        <span className="tnum font-bold">
                          ${fmtUsd(o.data.principal, 0)}
                        </span>
                        <span className="text-ink-dim">to</span>
                        <Who address={o.data.trader} />
                        <span
                          className={`ml-auto text-[11px] uppercase tracking-[0.12em] ${
                            st === nox.OfferState.Accepted
                              ? "text-long"
                              : st === nox.OfferState.Declined
                                ? "text-short"
                                : "text-ink-dim"
                          }`}
                        >
                          {OFFER_STATE_NAME[st]}
                        </span>
                      </div>
                      {reply ? (
                        <p className="mt-2 border-l-2 border-short/60 pl-3 text-xs text-ink-muted">
                          Their reason: {reply}
                        </p>
                      ) : null}
                      {st === nox.OfferState.Open ||
                      st === nox.OfferState.Declined ? (
                        <div className="mt-3">
                          <Btn
                            kind="line"
                            disabled={busy || !m.config}
                            onClick={() =>
                              void act(async (signer) => [
                                await revokeOfferIx(
                                  // signer is guaranteed by `me`
                                  signer,
                                  o,
                                  m.config!.usdcMint
                                ),
                              ])
                            }
                          >
                            Revoke · take the capital back
                          </Btn>
                        </div>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            )}
          </Panel>

          <Panel
            title="Requests to you"
            count={inbox.length}
            hint="Only reach you while your listing is open"
          >
            {!me ? (
              <Empty>Connect a wallet to see requests.</Empty>
            ) : inbox.length === 0 ? (
              <Empty>
                No trader has asked you for capital.{" "}
                {myListing?.data.open ? "" : "List yourself below so they can."}
              </Empty>
            ) : (
              <ul className="space-y-3">
                {inbox.map((r) => {
                  const p = m.profiles.get(r.data.trader);
                  return (
                    <li
                      key={r.address}
                      className="border border-line-soft p-3 text-sm"
                    >
                      <div className="flex flex-wrap items-center gap-3">
                        <Who address={r.data.trader} />
                        <span className="tnum">
                          wants ${fmtUsd(r.data.wantedPrincipal, 0)}
                        </span>
                        <span className="text-ink-dim">
                          at {fmtPctBps(r.data.wantedSplitBps)}
                        </span>
                      </div>
                      <p className="mt-2 text-xs text-ink-muted">
                        {noteText(r.data.note, r.data.noteLen) || "No note."}
                      </p>
                      {p ? (
                        <div className="mt-3">
                          <RecordStrip stats={traderStats(p.data)} />
                        </div>
                      ) : null}
                      <div className="mt-3 flex gap-2">
                        <Btn
                          disabled={busy}
                          onClick={() => setTarget(r.data.trader)}
                        >
                          Make an offer
                        </Btn>
                        <Btn
                          kind="line"
                          disabled={busy}
                          onClick={() =>
                            void act(async (signer) => [
                              closeRequestIx(signer, r),
                            ])
                          }
                        >
                          Dismiss
                        </Btn>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </Panel>
        </div>
      )}

      {own && (
        <MandatesPanel m={m} me={me} role="investor" act={act} busy={busy} />
      )}

      {own && <InvestorListingEditor m={m} s={s} act={act} busy={busy} />}
    </>
  );
}

/**
 * The mandates one side has, and where their money actually is.
 *
 * This is the panel whose absence made the marketplace read as a dead end: two wallets agreed
 * terms, $500 moved, and nothing on screen said so. `principal` is what was committed; `vault`
 * is what the mandate's own account holds right now, which is a different number as soon as the
 * capital is moved into SolFX to trade with.
 */
function MandatesPanel({
  m,
  me,
  role,
  act,
  busy,
}: {
  m: Marketplace;
  me: Address | undefined;
  role: "investor" | "trader";
  act: Act;
  busy: boolean;
}) {
  const mine = m.mandates.filter((x) =>
    role === "investor" ? x.data.investor === me : x.data.trader === me
  );
  return (
    <Panel
      title={role === "investor" ? "Your mandates" : "Mandates you trade"}
      count={mine.length}
      hint="The vault is the mandate's own account — not the trader's, and not ours"
    >
      {!me ? (
        <Empty>Connect a wallet to see your mandates.</Empty>
      ) : mine.length === 0 ? (
        <Empty>
          {role === "investor"
            ? "No mandates yet. One begins when a trader accepts an offer."
            : "No mandates yet. One begins when you accept an offer."}
        </Empty>
      ) : (
        <ul className="divide-y divide-line-soft">
          {mine.map(({ address, data }) => {
            const state = MANDATE_STATE[data.state] ?? MANDATE_STATE[0];
            const vault = m.vaults.get(address) ?? 0n;
            const equity = data.lastEquity > 0n ? data.lastEquity : null;
            return (
              <li key={address} className="py-4">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <a
                    href={explorer(address)}
                    target="_blank"
                    rel="noreferrer"
                    className="tnum text-xs text-brand-soft hover:underline"
                  >
                    {address.slice(0, 8)}…{address.slice(-4)}
                  </a>
                  <span
                    className={`text-[10px] uppercase tracking-[0.14em] ${
                      data.state === 0 ? "text-long" : "text-ink-dim"
                    }`}
                  >
                    {state.name}
                  </span>
                  <span className="text-xs text-ink-dim">
                    with{" "}
                    <Who
                      address={
                        role === "investor" ? data.trader : data.investor
                      }
                    />
                  </span>
                </div>
                <div className="mt-3 grid gap-px border border-line bg-line sm:grid-cols-4">
                  <Stat
                    label="Principal"
                    value={`$${fmtUsd(data.principal, 2)}`}
                  />
                  <Stat label="Vault holds" value={`$${fmtUsd(vault, 2)}`} />
                  <Stat
                    label="Equity, last marked"
                    value={
                      equity === null
                        ? "not marked yet"
                        : `$${fmtUsd(equity, 2)}`
                    }
                  />
                  <Stat
                    label="Trader keeps"
                    value={`${data.traderSplitBps / 100}% of net`}
                  />
                </div>
                <p className="mt-2 text-xs text-ink-dim">
                  {state.means}
                  {data.openPositions > 0
                    ? ` ${data.openPositions} position${data.openPositions === 1 ? "" : "s"} open.`
                    : ""}
                </p>
                <Settlement
                  m={m}
                  me={me}
                  role={role}
                  mandate={{ address, data }}
                  vault={vault}
                  act={act}
                  busy={busy}
                />
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}

/**
 * Ending a mandate, and what it would pay.
 *
 * Two steps, because the program makes them two: the investor asks, which stops new trades and
 * moves nothing, and then anyone at all runs the payout. That second part is permissionless by
 * design — an investor who has asked for their money must never need the trader's cooperation to
 * get it — so the button is offered to whoever is looking.
 *
 * The figures are a preview computed by the same rule the program applies, from the equity at the
 * last mark. The real split runs against the equity at the moment of the claim, and the panel
 * says so rather than implying a number it cannot promise.
 */
function Settlement({
  m,
  me,
  role,
  mandate,
  vault,
  act,
  busy,
}: {
  m: Marketplace;
  me: Address | undefined;
  role: "investor" | "trader";
  mandate: { address: Address; data: nox.Mandate };
  vault: bigint;
  act: Act;
  busy: boolean;
}) {
  const { address, data } = mandate;
  const settled = data.state === 3;
  if (settled) return null;

  const flat = data.openPositions === 0;
  const canAsk =
    role === "investor" && data.investor === me && data.state === 0;
  const canClaim = (data.state === 1 || data.state === 2) && flat;
  const marked = data.lastEquity > 0n ? data.lastEquity : vault;
  const split = previewSplit(
    marked,
    data.principal,
    m.config?.protocolFeeBps ?? 500,
    data.traderSplitBps
  );

  return (
    <div className="mt-3 border border-line-soft p-3">
      <div className="flex flex-wrap items-center gap-3">
        {canAsk ? (
          <Btn
            disabled={busy}
            onClick={() =>
              void act((signer) => [requestSettlementIx(signer, address)])
            }
          >
            Request settlement
          </Btn>
        ) : null}
        {canClaim ? (
          <Btn
            kind="solid"
            disabled={busy}
            onClick={() =>
              void act((signer) => claimSettlementIxs(signer, m, mandate))
            }
          >
            Pay everyone out
          </Btn>
        ) : null}
        <span className="text-[11px] text-ink-dim">
          {data.state === 0
            ? role === "investor"
              ? "Asking stops new trades. Your capital is not released until the payout."
              : "Only the investor can end this. You cannot withdraw."
            : !flat
              ? `Closing out: ${data.openPositions} position${data.openPositions === 1 ? "" : "s"} still open. Anyone may close them.`
              : "Flat and ready. Anyone may run the payout — the investor waits on nobody."}
        </span>
      </div>

      <div className="mt-3 grid gap-px border border-line bg-line sm:grid-cols-4">
        <Stat label="Investor takes" value={`$${fmtUsd(split.investor, 2)}`} />
        <Stat label="Trader takes" value={`$${fmtUsd(split.trader, 2)}`} />
        <Stat label="Protocol fee" value={`$${fmtUsd(split.protocol, 2)}`} />
        <Stat
          label="Profit to split"
          value={split.gross === 0n ? "none" : `$${fmtUsd(split.gross, 2)}`}
        />
      </div>
      <p className="mt-2 text-[11px] text-ink-dim">
        {split.gross === 0n
          ? "No profit, so no fee and no trader share: the investor takes whatever is left. That is what bearing the loss means."
          : `5% of gross to the protocol first, then ${data.traderSplitBps / 100}% of what remains to the trader.`}{" "}
        A preview from the last mark — the payout runs against the equity at the
        moment of the claim.
      </p>
    </div>
  );
}

function OfferForm({
  m,
  s,
  trader,
  requested,
  act,
  busy,
  onDone,
}: {
  m: Marketplace;
  s: ReturnType<typeof useMarketplace>;
  trader: Address;
  /** The principal this trader requested, when the form was opened from their request. */
  requested?: bigint;
  act: Act;
  busy: boolean;
  onDone: () => void;
}) {
  const listing = m.traderListings.find((l) => l.data.trader === trader);
  const profile = m.profiles.get(trader);
  const ceiling = profile
    ? TIER_MAX_MANDATE[profile.data.tier]
    : TIER_MAX_MANDATE[0];

  // Answer the question that was asked: the requested amount if there is a request, otherwise
  // the least this trader's listing accepts, and only then a round number. A default that
  // ignores both is a refusal waiting to happen — the program checks the principal against the
  // investor's balance, and the first offer made here asked for $1,000 from a $500 wallet.
  const [principal, setPrincipal] = useState(
    String(
      Number(requested ?? listing?.data.minPrincipal ?? 1_000_000_000n) / 1e6
    )
  );
  const [split, setSplit] = useState(
    String((listing?.data.wantedSplitBps ?? 7000) / 100)
  );
  const [days, setDays] = useState("7");
  const [note, setNote] = useState("");
  const [dd, setDd] = useState("6");
  const [daily, setDaily] = useState("3");
  const [risk, setRisk] = useState("1");
  const [stop, setStop] = useState("5");
  const [conc, setConc] = useState("2");
  const [markets, setMarkets] = useState<number[]>(
    listing ? marketIndices(listing.data.wantedMarkets) : []
  );

  const p = parseUsdc(principal);
  const pctToBps = (v: string) => {
    const n = Number(v);
    return Number.isFinite(n) && n > 0 ? Math.round(n * 100) : 0;
  };
  const rules: OfferRules | null =
    p === null
      ? null
      : {
          // Half the principal per trade, all of it in total: a sensible default the
          // investor can see and change, not a hidden constant.
          maxTradeNotional: p / 2n,
          maxTotalNotional: p,
          maxDrawdownBps: pctToBps(dd),
          maxDailyLossBps: pctToBps(daily),
          maxRiskPerTradeBps: pctToBps(risk),
          maxStopDistanceBps: pctToBps(stop),
          maxConcurrentPositions: Math.max(0, Math.min(8, Number(conc) | 0)),
          allowedMarkets: marketBitmap(markets),
          minHoldSlots: 0n,
        };

  // The same checks `MandateRules::validate` makes, so the form refuses what the program would.
  const problem =
    p === null
      ? "Enter a principal in USDC."
      : p > ceiling
        ? `${profile ? TIER_NAME[profile.data.tier] : "Bronze"} traders take at most $${fmtUsd(ceiling, 0)}.`
        : markets.length === 0
          ? "Pick at least one market."
          : !rules || rules.maxDrawdownBps <= 0 || rules.maxDrawdownBps > 10_000
            ? "Drawdown must be between 0 and 100%."
            : rules.maxDailyLossBps <= 0 ||
                rules.maxDailyLossBps > rules.maxDrawdownBps
              ? "Daily loss must be positive and no more than the total drawdown."
              : rules.maxRiskPerTradeBps <= 0 ||
                  rules.maxRiskPerTradeBps > rules.maxDrawdownBps
                ? "Risk per trade must be positive and no more than the total drawdown."
                : rules.maxConcurrentPositions < 1
                  ? "Allow at least one position."
                  : pctToBps(split) <= 0 || pctToBps(split) >= 10_000
                    ? "The trader's split must be between 0 and 100%."
                    : new TextEncoder().encode(note).length > 180
                      ? "The note is over 180 bytes."
                      : !m.config
                        ? "NOXFUNDS is not initialised on this cluster."
                        : null;

  return (
    <div className="mt-6 border border-brand/40 bg-brand/5 p-5">
      <div className="text-[10px] uppercase tracking-[0.18em] text-brand">
        Offer to <Who address={trader} />
      </div>
      <p className="mt-2 text-xs text-ink-muted">
        The principal moves into escrow when you post. It stays yours until the
        trader signs, and you can take it back at any moment before that —
        expired, declined or not.
      </p>
      <div className="mt-4 grid gap-3 sm:grid-cols-4">
        <Input
          label="Principal"
          value={principal}
          onChange={setPrincipal}
          suffix="USDC"
        />
        <Input
          label="Trader's split"
          value={split}
          onChange={setSplit}
          suffix="% of net"
        />
        <Input
          label="Expires in"
          value={days}
          onChange={setDays}
          suffix="days"
        />
        <Input label="Max positions" value={conc} onChange={setConc} />
        <Input label="Max drawdown" value={dd} onChange={setDd} suffix="%" />
        <Input
          label="Max daily loss"
          value={daily}
          onChange={setDaily}
          suffix="%"
        />
        <Input
          label="Risk per trade"
          value={risk}
          onChange={setRisk}
          suffix="% at stop"
        />
        <Input
          label="Max stop distance"
          value={stop}
          onChange={setStop}
          suffix="%"
        />
        <MarketPicker
          markets={s.markets}
          value={markets}
          onChange={setMarkets}
        />
        <Input
          label="Note to the trader"
          value={note}
          onChange={setNote}
          placeholder="≤ 180 bytes"
          wide
        />
      </div>
      <div className="mt-4 flex flex-wrap items-center gap-3">
        <Btn
          disabled={busy || problem !== null}
          onClick={() =>
            void act(async (signer) => {
              const seq = nextSeq(m, signer.address, trader);
              const d = BigInt(Math.max(1, Number(days) | 0));
              return [
                await postOfferIx({
                  signer,
                  trader,
                  seq,
                  principal: p!,
                  rules: rules!,
                  traderSplitBps: pctToBps(split),
                  expiresAt: m.now + d * 86_400n,
                  note,
                  usdcMint: m.config!.usdcMint,
                }),
              ];
            }).then(onDone)
          }
        >
          Escrow ${p ? fmtUsd(p, 0) : "—"} and post
        </Btn>
        {problem ? <span className="text-xs text-short">{problem}</span> : null}
      </div>
    </div>
  );
}

function InvestorListingEditor({
  m,
  s,
  act,
  busy,
}: {
  m: Marketplace;
  s: ReturnType<typeof useMarketplace>;
  act: Act;
  busy: boolean;
}) {
  const mine = m.investorListings.find((l) => l.data.investor === s.me);
  const d = mine?.data;
  const [min, setMin] = useState(
    d ? fmtUsd(d.minPrincipal, 0).replace(/,/g, "") : "1000"
  );
  const [max, setMax] = useState(
    d ? fmtUsd(d.maxPrincipal, 0).replace(/,/g, "") : "10000"
  );
  const [dd, setDd] = useState(d ? String(d.maxDrawdownBps / 100) : "6");
  const [risk, setRisk] = useState(
    d ? String(d.maxRiskPerTradeBps / 100) : "1"
  );
  const [split, setSplit] = useState(
    d ? String(d.offeredSplitBps / 100) : "70"
  );
  const [note, setNote] = useState(d ? noteText(d.note, d.noteLen) : "");
  const [markets, setMarkets] = useState<number[]>(
    d ? marketIndices(d.allowedMarkets) : []
  );

  const bps = (v: string) => Math.round(Number(v) * 100) || 0;
  const lo = parseUsdc(min);
  const hi = parseUsdc(max);
  const ok =
    lo !== null &&
    hi !== null &&
    hi >= lo &&
    markets.length > 0 &&
    bps(dd) > 0 &&
    bps(risk) > 0 &&
    bps(risk) <= bps(dd) &&
    bps(split) > 0 &&
    bps(split) < 10_000;

  const submit = (open: boolean) =>
    void act(async (signer) => [
      await postInvestorListingIx(
        signer,
        {
          minPrincipal: lo!,
          maxPrincipal: hi!,
          maxDrawdownBps: bps(dd),
          maxRiskPerTradeBps: bps(risk),
          allowedMarkets: marketBitmap(markets),
          offeredSplitBps: bps(split),
          note,
        },
        !!mine,
        open
      ),
    ]);

  return (
    <Panel
      title="Your investor listing"
      hint={
        mine
          ? mine.data.open
            ? "Open — traders can request"
            : "Closed — no new requests"
          : "Not listed"
      }
    >
      {!s.me ? (
        <Empty>Connect a wallet to list yourself.</Empty>
      ) : (
        <>
          <p className="mb-4 text-xs text-ink-muted">
            Advertising only: nothing is escrowed and nothing binds you. It is
            what lets traders find you and send you a request. Closing it shuts
            the door to new ones.
          </p>
          <div className="grid gap-3 sm:grid-cols-4">
            <Input label="From" value={min} onChange={setMin} suffix="USDC" />
            <Input label="Up to" value={max} onChange={setMax} suffix="USDC" />
            <Input
              label="Max drawdown"
              value={dd}
              onChange={setDd}
              suffix="%"
            />
            <Input
              label="Risk per trade"
              value={risk}
              onChange={setRisk}
              suffix="%"
            />
            <Input
              label="Trader's split"
              value={split}
              onChange={setSplit}
              suffix="%"
            />
            <MarketPicker
              markets={s.markets}
              value={markets}
              onChange={setMarkets}
            />
            <Input
              label="Note"
              value={note}
              onChange={setNote}
              placeholder="≤ 180 bytes"
              wide
            />
          </div>
          <div className="mt-4 flex gap-2">
            <Btn disabled={busy || !ok} onClick={() => submit(true)}>
              {mine ? "Save and keep open" : "List me"}
            </Btn>
            {mine?.data.open ? (
              <Btn
                kind="line"
                disabled={busy || !ok}
                onClick={() => submit(false)}
              >
                Close listing
              </Btn>
            ) : null}
          </div>
        </>
      )}
    </Panel>
  );
}

// --- trader --------------------------------------------------------------------------------

function TraderView({ m, s, act, busy, mode }: ViewProps) {
  const discovery = mode === "market";
  const own = mode === "trader";
  const me = s.me;
  const profile = me ? m.profiles.get(me) : undefined;
  const inbox = m.offers.filter(
    (o) => o.data.trader === me && o.data.state === nox.OfferState.Open
  );
  const mine = m.requests.filter((r) => r.data.trader === me);
  const investors = m.investorListings.filter((l) => l.data.open);
  const [asking, setAsking] = useState<Address | undefined>(undefined);
  const [reasons, setReasons] = useState<Record<string, string>>({});

  return (
    <>
      {own && (
        <Panel
          title="Your record"
          hint="Every trade from every mandate. You cannot start fresh."
        >
          {!me ? (
            <Empty>Connect a wallet to see your record.</Empty>
          ) : profile ? (
            <RecordStrip stats={traderStats(profile.data)} />
          ) : (
            <div className="flex flex-wrap items-center gap-4">
              <Empty>
                You have no trader profile yet. It is the record investors read,
                and nothing on this side works without it.
              </Empty>
              <Btn
                disabled={busy}
                onClick={() =>
                  void act(async (signer) => [await createProfileIx(signer)])
                }
              >
                Create my profile
              </Btn>
            </div>
          )}
        </Panel>
      )}

      {own && (
        <Panel
          title="Offers to you"
          count={inbox.length}
          hint="The capital is already in escrow"
        >
          {!me ? (
            <Empty>Connect a wallet to see offers.</Empty>
          ) : inbox.length === 0 ? (
            <Empty>No open offers addressed to you.</Empty>
          ) : (
            <ul className="space-y-4">
              {inbox.map((o) => {
                const d = o.data;
                const expired = m.now > d.expiresAt;
                return (
                  <li key={o.address} className="border border-line-soft p-4">
                    <div className="flex flex-wrap items-baseline gap-3">
                      <span className="tnum text-lg font-bold">
                        ${fmtUsd(d.principal, 0)}
                      </span>
                      <span className="text-xs text-ink-dim">from</span>
                      <Who address={d.investor} />
                      <span className="tnum text-xs text-ink-dim">
                        you keep {fmtPctBps(d.traderSplitBps)} of net profit
                      </span>
                      <span
                        className={`ml-auto text-[11px] uppercase tracking-[0.12em] ${expired ? "text-short" : "text-ink-dim"}`}
                      >
                        {expired
                          ? "Expired"
                          : `Expires ${new Date(Number(d.expiresAt) * 1000).toUTCString().slice(5, 22)} UTC`}
                      </span>
                    </div>
                    {noteText(d.note, d.noteLen) ? (
                      <p className="mt-2 border-l-2 border-brand/60 pl-3 text-xs text-ink-muted">
                        {noteText(d.note, d.noteLen)}
                      </p>
                    ) : null}
                    <div className="mt-3 grid grid-cols-2 gap-px border border-line bg-line sm:grid-cols-4">
                      <Stat
                        label="Max drawdown"
                        value={fmtPctBps(d.maxDrawdownBps)}
                      />
                      <Stat
                        label="Max daily loss"
                        value={fmtPctBps(d.maxDailyLossBps)}
                      />
                      <Stat
                        label="Risk per trade"
                        value={fmtPctBps(d.maxRiskPerTradeBps)}
                      />
                      <Stat
                        label="Max stop distance"
                        value={fmtPctBps(d.maxStopDistanceBps)}
                      />
                      <Stat
                        label="Per trade"
                        value={`$${fmtUsd(d.maxTradeNotional, 0)}`}
                      />
                      <Stat
                        label="In total"
                        value={`$${fmtUsd(d.maxTotalNotional, 0)}`}
                      />
                      <Stat
                        label="Positions"
                        value={String(d.maxConcurrentPositions)}
                      />
                      <Stat
                        label="Markets"
                        value={marketNames(d.allowedMarkets, s.markets)}
                      />
                    </div>
                    <p className="mt-3 text-[11px] text-ink-dim">
                      These rules are fixed the moment you accept and nobody —
                      including you and the investor — can change them
                      afterwards.
                    </p>
                    <div className="mt-3 flex flex-wrap items-center gap-2">
                      <Btn
                        disabled={busy || expired || !profile || !m.config}
                        onClick={() =>
                          void act(async (signer) => [
                            await acceptOfferIx(signer, o, m.config!.usdcMint),
                          ])
                        }
                      >
                        Accept · fund the mandate
                      </Btn>
                      <input
                        value={reasons[o.address] ?? ""}
                        onChange={(e) =>
                          setReasons({
                            ...reasons,
                            [o.address]: e.target.value,
                          })
                        }
                        placeholder="Reason, if declining (they will read it)"
                        className="min-w-[16rem] flex-1 border border-line bg-bg px-3 py-1.5 text-xs text-ink outline-none focus:border-brand"
                      />
                      <Btn
                        kind="danger"
                        disabled={busy}
                        onClick={() =>
                          void act(async (signer) => [
                            declineOfferIx(
                              signer,
                              o.address,
                              reasons[o.address] ?? ""
                            ),
                          ])
                        }
                      >
                        Decline
                      </Btn>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </Panel>
      )}

      {own && (
        <MandatesPanel m={m} me={me} role="trader" act={act} busy={busy} />
      )}

      {discovery && (
        <Panel
          title="Investors"
          count={investors.length}
          hint="Only investors who have listed can be asked"
        >
          {investors.length === 0 ? (
            <Empty>No investor has listed on this cluster yet.</Empty>
          ) : (
            <ul className="space-y-3">
              {investors.map((l) => {
                const d = l.data;
                const already = mine.some(
                  (r) => r.data.investor === d.investor
                );
                return (
                  <li
                    key={l.address}
                    className="border border-line-soft p-4 text-sm"
                  >
                    <div className="flex flex-wrap items-center gap-3">
                      <Who address={d.investor} />
                      <span className="tnum">
                        ${fmtUsd(d.minPrincipal, 0)}–$
                        {fmtUsd(d.maxPrincipal, 0)}
                      </span>
                      <span className="tnum text-xs text-ink-dim">
                        offers {fmtPctBps(d.offeredSplitBps)} · DD{" "}
                        {fmtPctBps(d.maxDrawdownBps)} · risk{" "}
                        {fmtPctBps(d.maxRiskPerTradeBps)}
                      </span>
                      <span className="ml-auto">
                        <Btn
                          kind={asking === d.investor ? "line" : "solid"}
                          disabled={
                            !me || !profile || already || d.investor === me
                          }
                          onClick={() =>
                            setAsking(
                              asking === d.investor ? undefined : d.investor
                            )
                          }
                        >
                          {already
                            ? "Requested"
                            : asking === d.investor
                              ? "Close"
                              : "Request funding"}
                        </Btn>
                      </span>
                    </div>
                    <div className="mt-1 text-xs text-ink-dim">
                      {marketNames(d.allowedMarkets, s.markets)}
                      {noteText(d.note, d.noteLen)
                        ? ` — ${noteText(d.note, d.noteLen)}`
                        : ""}
                    </div>
                    {asking === d.investor ? (
                      <RequestForm
                        investor={d.investor}
                        suggested={d.minPrincipal}
                        split={d.offeredSplitBps}
                        act={act}
                        busy={busy}
                        onDone={() => setAsking(undefined)}
                      />
                    ) : null}
                  </li>
                );
              })}
            </ul>
          )}
        </Panel>
      )}

      {own && (
        <div className="grid gap-6 lg:grid-cols-2">
          <Panel title="Your requests" count={mine.length}>
            {!me ? (
              <Empty>Connect a wallet to see your requests.</Empty>
            ) : mine.length === 0 ? (
              <Empty>You have not asked anyone yet.</Empty>
            ) : (
              <ul className="space-y-2">
                {mine.map((r) => (
                  <li
                    key={r.address}
                    className="flex flex-wrap items-center gap-3 border border-line-soft p-3 text-sm"
                  >
                    <Who address={r.data.investor} />
                    <span className="tnum">
                      ${fmtUsd(r.data.wantedPrincipal, 0)}
                    </span>
                    <span className="ml-auto">
                      <Btn
                        kind="line"
                        disabled={busy}
                        onClick={() =>
                          void act(async (signer) => [
                            closeRequestIx(signer, r),
                          ])
                        }
                      >
                        Withdraw · rent back
                      </Btn>
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Panel>

          <TraderListingEditor
            m={m}
            s={s}
            act={act}
            busy={busy}
            hasProfile={!!profile}
          />
        </div>
      )}
    </>
  );
}

function RequestForm({
  investor,
  suggested,
  split: offered,
  act,
  busy,
  onDone,
}: {
  investor: Address;
  suggested: bigint;
  split: number;
  act: Act;
  busy: boolean;
  onDone: () => void;
}) {
  const [amount, setAmount] = useState(fmtUsd(suggested, 0).replace(/,/g, ""));
  const [split, setSplit] = useState(String(offered / 100));
  const [note, setNote] = useState("");
  const p = parseUsdc(amount);
  const bps = Math.round(Number(split) * 100) || 0;
  const ok =
    p !== null &&
    bps > 0 &&
    bps < 10_000 &&
    new TextEncoder().encode(note).length <= 180;

  return (
    <div className="mt-4 border border-brand/40 bg-brand/5 p-4">
      <div className="grid gap-3 sm:grid-cols-4">
        <Input
          label="Asking for"
          value={amount}
          onChange={setAmount}
          suffix="USDC"
        />
        <Input
          label="Your split"
          value={split}
          onChange={setSplit}
          suffix="%"
        />
        <Input
          label="Why you"
          value={note}
          onChange={setNote}
          placeholder="≤ 180 bytes — they read it beside your record"
          wide
        />
      </div>
      <div className="mt-3 flex items-center gap-3">
        <Btn
          disabled={busy || !ok}
          onClick={() =>
            void act(async (signer) => [
              await postRequestIx({
                signer: signer,
                investor,
                wantedPrincipal: p!,
                wantedSplitBps: bps,
                note,
              }),
            ]).then(onDone)
          }
        >
          Send request
        </Btn>
        <span className="text-[11px] text-ink-dim">
          You pay a small rent deposit and get it back whenever it is closed, by
          either of you.
        </span>
      </div>
    </div>
  );
}

function TraderListingEditor({
  m,
  s,
  act,
  busy,
  hasProfile,
}: {
  m: Marketplace;
  s: ReturnType<typeof useMarketplace>;
  act: Act;
  busy: boolean;
  hasProfile: boolean;
}) {
  const mine = m.traderListings.find((l) => l.data.trader === s.me);
  const d = mine?.data;
  const [min, setMin] = useState(
    d ? fmtUsd(d.minPrincipal, 0).replace(/,/g, "") : "1000"
  );
  const [max, setMax] = useState(
    d ? fmtUsd(d.maxPrincipal, 0).replace(/,/g, "") : "10000"
  );
  const [split, setSplit] = useState(d ? String(d.wantedSplitBps / 100) : "70");
  const [note, setNote] = useState(d ? noteText(d.note, d.noteLen) : "");
  const [markets, setMarkets] = useState<number[]>(
    d ? marketIndices(d.wantedMarkets) : []
  );
  const lo = parseUsdc(min);
  const hi = parseUsdc(max);
  const bps = Math.round(Number(split) * 100) || 0;
  const ok =
    lo !== null &&
    hi !== null &&
    hi >= lo &&
    markets.length > 0 &&
    bps > 0 &&
    bps < 10_000;

  const submit = (open: boolean) =>
    void act(async (signer) => [
      await postTraderListingIx(
        signer,
        {
          minPrincipal: lo!,
          maxPrincipal: hi!,
          wantedMarkets: marketBitmap(markets),
          wantedSplitBps: bps,
          note,
        },
        !!mine,
        open
      ),
    ]);

  return (
    <Panel
      title="Your trader listing"
      hint={mine ? (mine.data.open ? "Open" : "Closed") : "Not listed"}
    >
      {!s.me ? (
        <Empty>Connect a wallet to list yourself.</Empty>
      ) : !hasProfile ? (
        <Empty>Create your profile first — a listing points at it.</Empty>
      ) : (
        <>
          <div className="grid gap-3 sm:grid-cols-2">
            <Input label="From" value={min} onChange={setMin} suffix="USDC" />
            <Input label="Up to" value={max} onChange={setMax} suffix="USDC" />
            <Input
              label="Your split"
              value={split}
              onChange={setSplit}
              suffix="% of net"
            />
            <MarketPicker
              markets={s.markets}
              value={markets}
              onChange={setMarkets}
            />
            <Input
              label="Note"
              value={note}
              onChange={setNote}
              placeholder="≤ 180 bytes"
              wide
            />
          </div>
          <div className="mt-4 flex gap-2">
            <Btn disabled={busy || !ok} onClick={() => submit(true)}>
              {mine ? "Save and keep open" : "List me"}
            </Btn>
            {mine?.data.open ? (
              <Btn
                kind="line"
                disabled={busy || !ok}
                onClick={() => submit(false)}
              >
                Close listing
              </Btn>
            ) : null}
          </div>
        </>
      )}
    </Panel>
  );
}
