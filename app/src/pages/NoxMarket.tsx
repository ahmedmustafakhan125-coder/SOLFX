import { useEffect, useMemo, useState } from "react";
import type { Address, Instruction, TransactionSigner } from "@solana/kit";
import { Direction, nox, QuoteConversionKind } from "@solfx/client";

import { NoxShell } from "@/components/NoxShell";
import { useMarketplace } from "@/hooks/useMarketplace";
import { useRpc, useSolfx } from "@/hooks/useSolfx";
import { useSend } from "@/hooks/useSend";
import { useSigner } from "@/hooks/useSigner";
import { fmtPrice, fmtUsd, priceplaces } from "@/lib/format";
import type { LoadedMarket } from "@/lib/markets";
import {
  loadPositions,
  repricePositions,
  type OpenPosition,
} from "@/lib/positions";
import type { LivePrice } from "@/lib/prices";
import {
  acceptOfferIx,
  closeRequestIx,
  createProfileIx,
  declineOfferIx,
  fmtFactorBps,
  fmtPctBps,
  fmtSlots,
  MARKET_CU,
  abandonEvaluationIx,
  claimSettlementIxs,
  claimStagePassIx,
  EVAL,
  evalCloseIx,
  evalObserveIx,
  evalOpenIx,
  EVAL_STATE_NAME,
  evalProgress,
  MANDATE_STATE,
  marketBitmap,
  marketIndices,
  mandateAuthority,
  fundedCancelOrderIx,
  fundedCloseIx,
  FUNDED_CLOSE_CU,
  fundedOpenIx,
  FUNDED_OPEN_CU,
  fundedPriceLimit,
  fundedTakeProfitIx,
  FUNDED_TRIGGER_CU,
  type Row,
  nextSeq,
  NOTIONAL_DIVISOR,
  ruleRefusal,
  nicknameOf,
  noteText,
  OFFER_STATE_NAME,
  parseUsdc,
  postInvestorListingIx,
  postOfferIx,
  postRequestIx,
  postTraderListingIx,
  previewSplit,
  startEvaluationIx,
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

/**
 * A wallet, as a name and an address — never a name alone.
 *
 * Nicknames are self-chosen and unenforced: two wallets may pick the same one, so a name that
 * replaced the address would be an impersonation waiting to happen. The address stays, and stays
 * the link.
 */
function Who({ address, m }: { address: string; m?: Marketplace }) {
  const name = m ? nicknameOf(m, address as Address) : "";
  return (
    <a
      href={explorer(address)}
      target="_blank"
      rel="noreferrer"
      className="hover:underline"
      title={address}
    >
      {name ? <span className="font-bold text-ink">{name} </span> : null}
      <span className="tnum text-brand-soft">{short(address)}</span>
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
    build: (
      signer: TransactionSigner
    ) => Promise<Instruction[]> | Instruction[],
    cu = MARKET_CU
  ) {
    setBuildError(undefined);
    if (!signer) {
      setBuildError("Connect a wallet first.");
      return;
    }
    try {
      const ixs = await build(signer);
      if (await tx.send(ixs, cu)) s.refresh();
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
  build: (signer: TransactionSigner) => Promise<Instruction[]> | Instruction[],
  /** Compute units to request. The marketplace's default is far too little for a funded open,
   *  which runs two CPIs into `solfx-core`. */
  cu?: number
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
                        <Who address={r.trader} m={m} />
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
                        <Who address={o.data.trader} m={m} />
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
                        <Who address={r.data.trader} m={m} />
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

/**
 * The evaluation: a $50 stake, a simulated account, and two phases to pass.
 *
 * A trader could not start one without this — the instructions existed and only the CLI called
 * them. Everything here is simulated except the stake, which is real USDC in the evaluation's own
 * vault, refunded when Phase 2 is passed and forfeited if the trader walks away or breaks a limit.
 *
 * The progress strip shows one blocker rather than six numbers, because "profit factor 1.31, need
 * 1.50" tells a trader what to do and a table of statistics does not.
 */
function EvaluationPanel({
  m,
  me,
  hasProfile,
  markets,
  prices,
  priceAccounts,
  act,
  busy,
}: {
  m: Marketplace;
  me: Address | undefined;
  /** `start_evaluation` takes the trader profile account; there is no evaluation without one. */
  hasProfile: boolean;
  markets: readonly LoadedMarket[];
  /** Keyed by feed id, as `useSolfx` keys it — not by symbol. */
  prices: Record<string, LivePrice | undefined>;
  priceAccounts: Record<string, Address | undefined>;
  act: Act;
  busy: boolean;
}) {
  const [size, setSize] = useState("10000");
  const [marketIndex, setMarketIndex] = useState<number | null>(null);
  const [notional, setNotional] = useState("1000");
  const [stopBps, setStopBps] = useState("100");

  const mine = m.evaluations.filter((e) => e.data.trader === me);
  const live = mine.find((e) => e.data.state === 0) ?? mine[0];
  const usdcMint = m.config?.usdcMint;

  // `eval_open_position` takes one price account and calls `load_validated_price(.., None, None)`,
  // so a market needing a second leg — a synthetic like XAU/EUR, or a non-USD quote like EUR/JPY —
  // is refused with `MissingSecondaryPriceUpdate` before anything else is checked. Offering one in
  // the list would be offering a trade the program cannot take.
  const tradeable = markets.filter(
    (x) =>
      x.tradeable &&
      x.data.priceSource.__kind === "Direct" &&
      x.data.quoteConversionKind === QuoteConversionKind.None
  );
  const chosen = tradeable.find((x) => x.index === marketIndex) ?? tradeable[0];
  const priceAccount = chosen ? priceAccounts[chosen.feedIdHex] : undefined;
  const price = chosen ? prices[chosen.feedIdHex] : undefined;

  const open = live
    ? m.virtualPositions.filter((v) => v.data.evaluation === live.address)
    : [];

  // The position PDA is seeded `[evaluation, market_index, nonce]`, so nonces are per market and
  // `open.length` is not one: hold BTC at 0 and 1, close 0, and the next open would collide at 1.
  // Closing frees the account (`close = trader`), so the lowest unused nonce on this market is
  // always right.
  const usedNonces = chosen
    ? open
        .filter((v) => v.data.marketIndex === chosen.index)
        .map((v) => v.data.nonce)
    : [];
  let nonce = 0;
  while (usedNonces.includes(nonce)) nonce += 1;

  if (!me) {
    return (
      <Panel
        title="Evaluation"
        hint="Prove yourself before anyone risks money on you"
      >
        <Empty>Connect a wallet to start an evaluation.</Empty>
      </Panel>
    );
  }

  // --- no evaluation yet: the terms, then the stake -----------------------------------------
  if (!live || live.data.state !== 0) {
    const accountSize = parseUsdc(size);
    const ok =
      accountSize !== null &&
      accountSize >= EVAL.minAccount &&
      accountSize <= EVAL.maxAccount;
    const seq = mine.length;
    return (
      <Panel
        title="Evaluation"
        hint="Everything is simulated except the $50 stake, which comes back when you pass"
      >
        {live ? (
          <p className="mb-4 text-xs text-ink-dim">
            Your last evaluation ended: {EVAL_STATE_NAME[live.data.state]}.
            Starting another opens a fresh record at sequence {seq}.
          </p>
        ) : null}
        <div className="grid gap-px border border-line bg-line sm:grid-cols-4">
          <Stat label="Stake" value="$50, refundable" />
          <Stat label="Phase 1 target" value={`${EVAL.targetBps[0] / 100}%`} />
          <Stat label="Phase 2 target" value={`${EVAL.targetBps[1] / 100}%`} />
          <Stat label="Max drawdown" value={`${EVAL.maxDrawdownBps / 100}%`} />
          <Stat
            label="Daily loss limit"
            value={`${EVAL.maxDailyLossBps / 100}%`}
          />
          <Stat
            label="Risk per trade"
            value={`${EVAL.maxRiskBps / 100}% at the stop`}
          />
          <Stat
            label="Trades"
            value={`${EVAL.minTrades} over ${EVAL.minDays} days`}
          />
          <Stat label="Minimum hold" value={`${EVAL.minHoldSecs / 60} min`} />
        </div>
        <div className="mt-4 flex flex-wrap items-end gap-3">
          <Input
            label="Simulated account"
            value={size}
            onChange={setSize}
            suffix="USDC"
          />
          <Btn
            disabled={busy || !ok || !usdcMint || !hasProfile}
            onClick={() =>
              void act((signer) =>
                startEvaluationIx(signer, usdcMint!, seq, accountSize!)
              )
            }
          >
            Stake $50 and begin
          </Btn>
          <span className="text-[11px] text-ink-dim">
            {!hasProfile
              ? "Create your trader profile first — the pass is recorded against it."
              : ok
                ? "A stop-loss is mandatory on every trade, and the 10-minute hold applies to closing by choice."
                : `Between $${fmtUsd(EVAL.minAccount, 0)} and $${fmtUsd(EVAL.maxAccount, 0)}.`}
          </span>
        </div>
      </Panel>
    );
  }

  // --- an evaluation in progress --------------------------------------------------------------
  const p = evalProgress(live.data);

  // Sizing is integer the whole way, in the program's own units: notional at QUOTE_PRECISION
  // 1e6, price at PRICE_PRECISION 1e9, size at BASE_PRECISION 1e9, related by
  // `notional = size x price / NOTIONAL_DIVISOR`. A float here would round somewhere the
  // program does not, and the stop would not be the number shown.
  const px = price?.price ?? 0n;
  const notionalQuote = parseUsdc(notional) ?? 0n;
  const bps = Number(stopBps);
  const stopOk = Number.isInteger(bps) && bps > 0 && bps < 10_000;
  const sizeBase = px > 0n ? (notionalQuote * NOTIONAL_DIVISOR) / px : 0n;
  // Below entry for a long, so the stop is the losing side. Floor: a lower stop risks
  // marginally more, which is the side the risk check reads, so it cannot be gamed by rounding.
  const stop = stopOk ? (px * BigInt(10_000 - bps)) / 10_000n : 0n;
  // Every open position, with its market and oracle account. The program marks all of them or
  // none: a missing leg is `IncompleteObservation`, so a short list is not sent at all.
  const legs = open.flatMap((v) => {
    const mk = markets.find((x) => x.index === v.data.marketIndex);
    const pa = mk ? priceAccounts[mk.feedIdHex] : undefined;
    return mk && pa
      ? [{ position: v.address, market: mk.address, priceUpdate: pa }]
      : [];
  });

  const canOpen =
    !busy &&
    sizeBase > 0n &&
    stop > 0n &&
    !!chosen &&
    !!priceAccount &&
    price?.stale === false &&
    open.length < EVAL.maxOpen;

  return (
    <Panel
      title={`Evaluation — Phase ${p.stage}`}
      hint={`Simulated $${fmtUsd(live.data.accountSize, 0)} · stake $50 held`}
    >
      <div className="grid gap-px border border-line bg-line sm:grid-cols-4">
        <Stat label="Balance" value={`$${fmtUsd(p.equity, 2)}`} />
        <Stat label="Target" value={`$${fmtUsd(p.targetEquity, 2)}`} />
        <Stat
          label="Profit"
          value={`${Number(p.profitBps) / 100}% of ${p.targetBps / 100}%`}
        />
        <Stat
          label="Worst drawdown"
          value={`${Number(p.drawdownBps) / 100}%`}
        />
        <Stat
          label="Today's loss"
          value={`${Number(p.dailyLossBps) / 100}% of ${EVAL.maxDailyLossBps / 100}%`}
        />
        <Stat label="Trades" value={`${p.trades} of ${EVAL.minTrades}`} />
        <Stat label="Trading days" value={`${p.days} of ${EVAL.minDays}`} />
        <Stat label="Open" value={`${open.length} of ${EVAL.maxOpen}`} />
      </div>

      <p className="mt-3 text-xs text-ink-dim">
        {p.blocker
          ? `Still needed: ${p.blocker}.`
          : "Every requirement met — claim the stage."}
      </p>

      {/* --- the ticket ------------------------------------------------------------------- */}
      <div className="mt-4 border border-line-soft p-3">
        <div className="flex flex-wrap items-end gap-3">
          <label className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
            Market
            <select
              value={chosen?.index ?? ""}
              onChange={(e) => setMarketIndex(Number(e.target.value))}
              className="mt-1 block border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
              {tradeable.map((x) => (
                <option key={x.index} value={x.index}>
                  {x.symbol}
                </option>
              ))}
            </select>
          </label>
          <Input
            label="Notional"
            value={notional}
            onChange={setNotional}
            suffix="USDC"
          />
          <Input
            label="Stop, below entry"
            value={stopBps}
            onChange={setStopBps}
            suffix="bps"
          />
          <Btn
            disabled={!canOpen}
            onClick={() =>
              void act(async (signer) => [
                await evalOpenIx({
                  signer,
                  evaluation: live.address,
                  marketIndex: chosen!.index,
                  market: chosen!.address,
                  priceUpdate: priceAccount!,
                  nonce,
                  direction: nox.Direction.Long,
                  sizeBase,
                  stopLossPrice: stop,
                }),
              ])
            }
          >
            Open a simulated long
          </Btn>
        </div>
        <p className="mt-2 text-[11px] text-ink-dim">
          {px > 0n && chosen
            ? `${chosen.symbol} at ${fmtPrice(px, priceplaces(chosen.symbol))}${price?.stale ? ` — stale by ${price.ageSeconds}s, so the program would refuse this` : ""}. The fill is priced by SolFX's own function, so it crosses the spread exactly as a real one would. Nothing is filled on the venue.`
            : "Waiting for a price."}{" "}
          A stop is mandatory, and risk at it may not exceed{" "}
          {EVAL.maxRiskBps / 100}% of the balance.
        </p>
      </div>

      {/* --- open simulated positions ------------------------------------------------------ */}
      {open.length > 0 ? (
        <ul className="mt-4 divide-y divide-line-soft">
          {open.map(({ address, data }) => {
            const mk = markets.find((x) => x.index === data.marketIndex);
            const pa = mk ? priceAccounts[mk.feedIdHex] : undefined;
            const held = Number(m.now - data.openedAt);
            const holdLeft = EVAL.minHoldSecs - held;
            return (
              <li
                key={address}
                className="flex flex-wrap items-center gap-3 py-3 text-sm"
              >
                <span className="font-bold">
                  {mk?.symbol ?? `#${data.marketIndex}`}
                </span>
                <span className="tnum text-xs text-ink-muted">
                  entry{" "}
                  {fmtPrice(data.entryPrice, priceplaces(mk?.symbol ?? ""))} · $
                  {fmtUsd(data.entryNotional, 2)} · stop{" "}
                  {fmtPrice(data.stopPrice, priceplaces(mk?.symbol ?? ""))}
                </span>
                <Btn
                  disabled={busy || holdLeft > 0 || !pa || !mk}
                  onClick={() =>
                    void act((signer) => [
                      evalCloseIx({
                        signer,
                        evaluation: live.address,
                        virtualPosition: address,
                        market: mk!.address,
                        priceUpdate: pa!,
                      }),
                    ])
                  }
                >
                  {holdLeft > 0
                    ? `Hold ${Math.ceil(holdLeft / 60)} min`
                    : "Close"}
                </Btn>
              </li>
            );
          })}
        </ul>
      ) : null}

      <div className="mt-4 flex flex-wrap gap-2">
        <Btn
          disabled={busy || legs.length === 0 || legs.length !== open.length}
          onClick={() =>
            void act((signer) => [evalObserveIx(signer, live.address, legs)])
          }
        >
          {legs.length === open.length
            ? "Mark to market"
            : "Mark to market — a price account is missing"}
        </Btn>
        <Btn
          disabled={busy || !!p.blocker || !usdcMint}
          onClick={() =>
            void act(async (signer) => [
              await claimStagePassIx(signer, usdcMint!, live.address),
            ])
          }
        >
          {p.stage === 1 ? "Claim Phase 1" : "Claim Phase 2 and the stake"}
        </Btn>
        <Btn
          kind="danger"
          disabled={busy}
          onClick={() =>
            void act((signer) => [abandonEvaluationIx(signer, live.address)])
          }
        >
          Walk away — the stake is forfeit
        </Btn>
      </div>
    </Panel>
  );
}

/**
 * Trading an investor's money, from the browser.
 *
 * Why this is not the terminal's ticket: a funded position's SolFX authority is the mandate
 * signer, a PDA with no private key. Only `noxfunds` can sign for it, so every order here goes
 * through the wrapper and is checked against the mandate **before** the fill rather than
 * audited afterwards. The preflight line under the ticket is that check, mirrored — so a trader
 * reads the refusal before signing rather than after paying a fee for it.
 */
function FundedTradingPanel({
  m,
  me,
  markets,
  prices,
  priceAccounts,
  act,
  busy,
}: {
  m: Marketplace;
  me: Address | undefined;
  markets: readonly LoadedMarket[];
  prices: Record<string, LivePrice | undefined>;
  priceAccounts: Record<string, Address | undefined>;
  act: Act;
  busy: boolean;
}) {
  const rpc = useRpc();
  const mine = m.mandates.filter((x) => x.data.trader === me);
  const [chosenMandate, setChosenMandate] = useState<Address | undefined>();
  const mandate =
    mine.find((x) => x.address === chosenMandate) ??
    mine.find((x) => x.data.state === 0) ??
    mine[0];

  const [marketIndex, setMarketIndex] = useState<number | null>(null);
  const [direction, setDirection] = useState<nox.DirectionArgs>(
    nox.Direction.Long
  );
  const [notional, setNotional] = useState("1000");
  const [collateral, setCollateral] = useState("200");
  const [slippage, setSlippage] = useState("50");
  const [stopBps, setStopBps] = useState("100");
  const [tpBps, setTpBps] = useState("200");

  // The same restriction the evaluation ticket carries, for the same reason: a market priced
  // from two or three feeds needs those accounts passed, and this page resolves one per market.
  // `nox market --execute` handles the rest until the extra legs are wired here.
  const tradeable = markets.filter(
    (x) =>
      x.tradeable &&
      x.data.priceSource.__kind === "Direct" &&
      x.data.quoteConversionKind === QuoteConversionKind.None
  );
  const permitted = mandate
    ? tradeable.filter(
        (x) => (mandate.data.allowedMarkets & (1n << BigInt(x.index))) !== 0n
      )
    : [];
  const chosen = permitted.find((x) => x.index === marketIndex) ?? permitted[0];
  const price = chosen ? prices[chosen.feedIdHex] : undefined;
  const priceAccount = chosen ? priceAccounts[chosen.feedIdHex] : undefined;

  // The mandate's open positions, read from SolFX against the mandate signer rather than from
  // the mandate's own slot array: the slots say a position exists, the position says at what
  // entry and how it is doing.
  const [open, setOpen] = useState<readonly OpenPosition[]>([]);
  const priceByIndex = useMemo(() => {
    const out: Record<number, bigint | undefined> = {};
    for (const x of markets) out[x.index] = prices[x.feedIdHex]?.price;
    return out;
  }, [markets, prices]);
  useEffect(() => {
    let cancelled = false;
    if (!mandate || markets.length === 0) {
      setOpen([]);
      return;
    }
    void (async () => {
      try {
        const signer = await mandateAuthority(mandate.address);
        const found = await loadPositions(
          rpc,
          signer,
          markets.map((x) => x.index),
          priceByIndex
        );
        if (!cancelled) setOpen(found);
      } catch {
        if (!cancelled) setOpen([]);
      }
    })();
    return () => {
      cancelled = true;
    };
    // `priceByIndex` changes on every price tick; re-reading positions that often is wasteful,
    // so the marks are refreshed separately below and this runs on the mandate and market set.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rpc, mandate?.address, markets]);

  const marked = useMemo(
    () => repricePositions(open, priceByIndex),
    [open, priceByIndex]
  );

  if (!me) {
    return (
      <Panel title="Trade" hint="An investor's capital, under their rules">
        <Empty>Connect a wallet to trade a mandate.</Empty>
      </Panel>
    );
  }
  if (!mandate) {
    return (
      <Panel title="Trade" hint="An investor's capital, under their rules">
        <Empty>
          No mandate yet. Accept an offer in the marketplace and the ticket
          appears here.
        </Empty>
      </Panel>
    );
  }

  const d = mandate.data;
  const active = d.state === 0;
  const px = price?.price ?? 0n;
  const notionalQuote = parseUsdc(notional) ?? 0n;
  const collateralQuote = parseUsdc(collateral) ?? 0n;
  const sizeBase = px > 0n ? (notionalQuote * NOTIONAL_DIVISOR) / px : 0n;

  const long = direction === nox.Direction.Long;
  const stopN = Number(stopBps);
  const stopOk = Number.isInteger(stopN) && stopN > 0 && stopN < 10_000;
  // Adverse to the trader on both sides: a long's stop floors (a little lower, a little more
  // risk measured), a short's ceils. The risk check reads the distance, so rounding the other
  // way would let a ticket slip under the mandate's limit by a unit.
  const stopPrice = !stopOk
    ? 0n
    : long
      ? (px * BigInt(10_000 - stopN)) / 10_000n
      : ceilBps(px, 10_000 + stopN);

  const slipN = Number(slippage);
  const slipOk = Number.isInteger(slipN) && slipN >= 0 && slipN <= 1_000;

  const refusal =
    px === 0n
      ? "waiting for a price"
      : !slipOk
        ? "slippage must be 0–1000 bps"
        : sizeBase === 0n
          ? "enter a notional"
          : collateralQuote === 0n
            ? "enter the margin to post"
            : ruleRefusal({
                mandate: d,
                marketIndex: chosen?.index ?? -1,
                direction,
                price: px,
                sizeBase,
                stopPrice,
                openPositions: d.openPositions,
              });

  const stale = price?.stale === true;
  const canOpen =
    !busy && active && !refusal && !stale && !!chosen && !!priceAccount;

  // Nonces are per market: reusing an occupied one fails as "account already in use", which
  // reads like a protocol error and is not one.
  const used = chosen
    ? marked.filter((p) => p.marketIndex === chosen.index).map((p) => p.nonce)
    : [];
  let nonce = 0;
  while (used.includes(nonce)) nonce += 1;

  return (
    <Panel
      title="Trade"
      hint="Checked against the mandate before the fill, not audited after it"
    >
      {mine.length > 1 ? (
        <label className="mb-4 block text-[10px] uppercase tracking-[0.16em] text-ink-dim">
          Mandate
          <select
            value={mandate.address}
            onChange={(e) => setChosenMandate(e.target.value as Address)}
            className="mt-1 block border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
            {mine.map((x) => (
              <option key={x.address} value={x.address}>
                {x.address.slice(0, 8)}… · ${fmtUsd(x.data.principal, 0)} ·{" "}
                {(MANDATE_STATE[x.data.state] ?? MANDATE_STATE[0]).name}
              </option>
            ))}
          </select>
        </label>
      ) : null}

      <div className="grid gap-px border border-line bg-line sm:grid-cols-4">
        <Stat label="Principal" value={`$${fmtUsd(d.principal, 2)}`} />
        <Stat
          label="Per trade"
          value={`$${fmtUsd(d.maxTradeNotional, 0)} max`}
        />
        <Stat
          label="Book"
          value={`$${fmtUsd(d.openNotional, 0)} of $${fmtUsd(d.maxTotalNotional, 0)}`}
        />
        <Stat
          label="Positions"
          value={`${d.openPositions} of ${d.maxConcurrentPositions}`}
        />
        <Stat
          label="Risk per trade"
          value={`${d.maxRiskPerTradeBps / 100}% at the stop`}
        />
        <Stat
          label="Stop no wider than"
          value={`${d.maxStopDistanceBps / 100}%`}
        />
        <Stat label="Minimum hold" value={`${d.minHoldSlots} slots`} />
        <Stat label="Your split" value={`${d.traderSplitBps / 100}%`} />
      </div>

      {!active ? (
        <p className="mt-3 text-xs text-short">
          This mandate is{" "}
          {(MANDATE_STATE[d.state] ?? MANDATE_STATE[0]).name.toLowerCase()} — no
          new positions. Anything still open can be closed below.
        </p>
      ) : null}

      {/* --- the ticket -------------------------------------------------------------------- */}
      {active ? (
        <div className="mt-4 border border-line-soft p-3">
          {permitted.length === 0 ? (
            <Empty>
              None of the markets this mandate permits can be priced from a
              single oracle account, which is all this page passes today. Use{" "}
              <code>nox market</code> for those.
            </Empty>
          ) : (
            <>
              <div className="flex flex-wrap items-end gap-3">
                <label className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                  Market
                  <select
                    value={chosen?.index ?? ""}
                    onChange={(e) => setMarketIndex(Number(e.target.value))}
                    className="mt-1 block border border-line bg-surface px-3 py-2 text-sm text-ink"
                  >
                    {permitted.map((x) => (
                      <option key={x.index} value={x.index}>
                        {x.symbol}
                      </option>
                    ))}
                  </select>
                </label>
                <div className="flex gap-1.5">
                  <Btn
                    kind={long ? "solid" : "line"}
                    onClick={() => setDirection(nox.Direction.Long)}
                  >
                    Long
                  </Btn>
                  <Btn
                    kind={long ? "line" : "solid"}
                    onClick={() => setDirection(nox.Direction.Short)}
                  >
                    Short
                  </Btn>
                </div>
                <Input
                  label="Notional"
                  value={notional}
                  onChange={setNotional}
                  suffix="USDC"
                />
                <Input
                  label="Margin"
                  value={collateral}
                  onChange={setCollateral}
                  suffix="USDC"
                />
                <Input
                  label={`Stop, ${long ? "below" : "above"} entry`}
                  value={stopBps}
                  onChange={setStopBps}
                  suffix="bps"
                />
                <Input
                  label="Slippage"
                  value={slippage}
                  onChange={setSlippage}
                  suffix="bps"
                />
                <Btn
                  disabled={!canOpen}
                  onClick={() =>
                    void act(
                      async (signer) => [
                        await fundedOpenIx({
                          signer,
                          mandate: mandate.address,
                          marketIndex: chosen!.index,
                          nonce,
                          direction,
                          sizeBase,
                          collateral: collateralQuote,
                          priceLimit: fundedPriceLimit(direction, px, slipN),
                          stopLossPrice: stopPrice,
                          legs: { priceUpdate: priceAccount! },
                        }),
                      ],
                      FUNDED_OPEN_CU
                    )
                  }
                >
                  Open with its stop
                </Btn>
              </div>

              <p className="mt-2 text-[11px] text-ink-dim">
                {px > 0n && chosen ? (
                  <>
                    {chosen.symbol} at{" "}
                    {fmtPrice(px, priceplaces(chosen.symbol))}
                    {stale ? (
                      <span className="text-short">
                        {" "}
                        — stale by {price?.ageSeconds}s, and the program refuses
                        anything over 60
                      </span>
                    ) : null}
                    . Stop at {fmtPrice(stopPrice, priceplaces(chosen.symbol))}{" "}
                    lands in the same transaction as the position — a funded
                    trade is never briefly unprotected.
                  </>
                ) : (
                  "Waiting for a price."
                )}
              </p>
              <p
                className={`mt-1 text-[11px] ${refusal ? "text-short" : "text-long"}`}
              >
                {refusal
                  ? `The mandate would refuse this: ${refusal}.`
                  : "Within every rule on this mandate."}
              </p>
              <p className="mt-2 text-[11px] text-ink-dim">
                A resting entry order is not offered because SolFX has none:
                triggers attach to an open position, so a limit entry would be
                this browser watching a price and sending when it hits — which
                stops the moment the tab closes. Stop and take-profit are real
                on-chain orders a keeper fires.
              </p>
            </>
          )}
        </div>
      ) : null}

      {/* --- open positions ---------------------------------------------------------------- */}
      <div className="mt-5">
        <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
          Open — {marked.length}
        </div>
        {marked.length === 0 ? (
          <Empty>Nothing open on this mandate.</Empty>
        ) : (
          <ul className="mt-1 divide-y divide-line-soft">
            {marked.map((p) => (
              <FundedRow
                key={p.address}
                p={p}
                mandate={mandate}
                markets={markets}
                prices={prices}
                priceAccounts={priceAccounts}
                slot={m.slot}
                slippageBps={slipOk ? slipN : 50}
                tpBps={tpBps}
                setTpBps={setTpBps}
                act={act}
                busy={busy}
              />
            ))}
          </ul>
        )}
      </div>
    </Panel>
  );
}

/** Ceiling of `price x bps / 10_000`, for a short's stop — the side that measures more risk. */
function ceilBps(price: bigint, bps: number): bigint {
  const n = price * BigInt(bps);
  return (n + 9_999n) / 10_000n;
}

function FundedRow({
  p,
  mandate,
  markets,
  prices,
  priceAccounts,
  slot,
  slippageBps,
  tpBps,
  setTpBps,
  act,
  busy,
}: {
  p: OpenPosition;
  mandate: Row<nox.Mandate>;
  markets: readonly LoadedMarket[];
  prices: Record<string, LivePrice | undefined>;
  priceAccounts: Record<string, Address | undefined>;
  slot: bigint;
  slippageBps: number;
  tpBps: string;
  setTpBps: (v: string) => void;
  act: Act;
  busy: boolean;
}) {
  const mk = markets.find((x) => x.index === p.marketIndex);
  const live = mk ? prices[mk.feedIdHex] : undefined;
  const account = mk ? priceAccounts[mk.feedIdHex] : undefined;
  const px = live?.price ?? 0n;
  const long = p.data.direction === Direction.Long;
  const places = priceplaces(mk?.symbol ?? "");

  // The no-scalping rule is counted in slots, so the countdown is too.
  const held = slot > p.data.openedAtSlot ? slot - p.data.openedAtSlot : 0n;
  const leftSlots = mandate.data.minHoldSlots - held;
  const holdLeft = leftSlots > 0n ? leftSlots : 0n;

  const bps = Number(tpBps);
  const tpOk = Number.isInteger(bps) && bps > 0 && bps < 10_000;
  // A target is profit, so it sits above a long and below a short. Rounding is adverse to the
  // trader on both: a long's target ceils (a little further away), a short's floors.
  const target = !tpOk
    ? 0n
    : long
      ? ceilBps(px, 10_000 + bps)
      : (px * BigInt(10_000 - bps)) / 10_000n;

  // The stop took `order_id = nonce` at open, so the target takes the next free one.
  const tpOrderId = p.nonce + 1;

  return (
    <li className="flex flex-wrap items-center gap-x-4 gap-y-2 py-3 text-sm">
      <span className="font-bold">{mk?.symbol ?? `#${p.marketIndex}`}</span>
      <span
        className={`text-[10px] uppercase tracking-[0.14em] ${long ? "text-long" : "text-short"}`}
      >
        {long ? "Long" : "Short"}
      </span>
      <span className="tnum text-xs text-ink-muted">
        entry {fmtPrice(p.data.entryPrice, places)} · $
        {fmtUsd(p.data.entryNotional, 2)} · margin $
        {fmtUsd(p.data.collateral, 2)}
      </span>
      <span
        className={`tnum text-xs font-bold ${p.unrealised >= 0n ? "text-long" : "text-short"}`}
      >
        {p.unrealised >= 0n ? "+" : "−"}$
        {fmtUsd(p.unrealised < 0n ? -p.unrealised : p.unrealised, 2)}
      </span>

      <span className="ml-auto flex flex-wrap items-center gap-2">
        <Input label="Target" value={tpBps} onChange={setTpBps} suffix="bps" />
        <Btn
          disabled={busy || !tpOk || px === 0n || !account || !mk}
          onClick={() =>
            void act(
              async (signer) => [
                await fundedTakeProfitIx({
                  signer,
                  mandate: mandate.address,
                  marketIndex: p.marketIndex,
                  nonce: p.nonce,
                  orderId: tpOrderId,
                  triggerPrice: target,
                  sizeBase: p.data.sizeBase,
                  legs: { priceUpdate: account! },
                }),
              ],
              FUNDED_TRIGGER_CU
            )
          }
        >
          {tpOk && px > 0n
            ? `Target ${fmtPrice(target, places)}`
            : "Set target"}
        </Btn>
        <Btn
          kind="line"
          disabled={busy}
          onClick={() =>
            void act(
              async (signer) => [
                await fundedCancelOrderIx({
                  signer,
                  mandate: mandate.address,
                  marketIndex: p.marketIndex,
                  nonce: p.nonce,
                  orderId: tpOrderId,
                }),
              ],
              FUNDED_TRIGGER_CU
            )
          }
        >
          Cancel target
        </Btn>
        <Btn
          kind="danger"
          disabled={busy || holdLeft > 0n || px === 0n || !account}
          onClick={() =>
            void act(
              async (signer) => [
                // The stop's rent goes to the keeper when it fires and to the authority when it
                // is cancelled — there is no third path, so a voluntary close that leaves it
                // behind strands 0.002 SOL of the mandate's money for good.
                await fundedCancelOrderIx({
                  signer,
                  mandate: mandate.address,
                  marketIndex: p.marketIndex,
                  nonce: p.nonce,
                  orderId: p.nonce,
                }),
                await fundedCloseIx({
                  signer,
                  mandate: mandate.address,
                  marketIndex: p.marketIndex,
                  nonce: p.nonce,
                  priceLimit: fundedPriceLimit(
                    long ? nox.Direction.Short : nox.Direction.Long,
                    px,
                    slippageBps
                  ),
                  legs: { priceUpdate: account! },
                }),
              ],
              FUNDED_CLOSE_CU + FUNDED_TRIGGER_CU
            )
          }
        >
          {holdLeft > 0n ? `Hold ${holdLeft} slots` : "Close"}
        </Btn>
      </span>
    </li>
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
        Offer to <Who address={trader} m={m} />
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
  const [nickname, setNickname] = useState(
    d ? noteText(d.nickname, d.nicknameLen) : ""
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
          nickname,
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
              label="Display name"
              value={nickname}
              onChange={setNickname}
              placeholder="≤ 24 characters, optional"
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
  // Live prices and their oracle accounts, for the evaluation ticket. The marketplace read is
  // account state only and carries neither.
  const solfx = useSolfx();
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
        <EvaluationPanel
          m={m}
          me={me}
          hasProfile={!!profile}
          markets={s.markets}
          prices={solfx.prices}
          priceAccounts={solfx.priceAccounts}
          act={act}
          busy={busy}
        />
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
                      <Who address={d.investor} m={m} />
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

      {own && (
        <FundedTradingPanel
          m={m}
          me={me}
          markets={s.markets}
          prices={solfx.prices}
          priceAccounts={solfx.priceAccounts}
          act={act}
          busy={busy}
        />
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
                      <Who address={d.investor} m={m} />
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
                    <Who address={r.data.investor} m={m} />
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
  const [nickname, setNickname] = useState(
    d ? noteText(d.nickname, d.nicknameLen) : ""
  );
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
          nickname,
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
              label="Display name"
              value={nickname}
              onChange={setNickname}
              placeholder="≤ 24 characters, optional"
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
