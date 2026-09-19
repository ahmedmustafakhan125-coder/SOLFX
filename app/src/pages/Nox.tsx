import { Link } from "react-router-dom";

import { NoxFlow } from "@/components/NoxFlow";
import { ProductSwitcher } from "@/components/ProductSwitcher";

const NOX_PROGRAM = "9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx";
const SOLFX_PROGRAM = "2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi";

const explorer = (a: string) =>
  `https://explorer.solana.com/address/${a}?cluster=devnet`;

/**
 * Real mandates from devnet, read back off chain rather than typed from a spreadsheet.
 *
 * All three lost a little, which is the honest result of opening and closing a position in the
 * same minute: you pay the spread and the fee in both directions. They are shown as they are
 * because a page that only lists wins is the thing this product exists to replace.
 */
const MANDATES = [
  {
    address: "Bf7aVemJQwTq5trPgqo6t7vjmHy4M3FcxjjHSp1BCyrc",
    principal: "200.000000",
    returned: "199.797874",
  },
  {
    address: "5mhW6R4pZ5vjspS7kPGaojAf3NMaqkRrvgofSHuRBhd7",
    principal: "200.000000",
    returned: "199.770001",
  },
  {
    address: "5qyCdLD2NXCK5jJuHryVwkwvjkxtc2CjeP34DgUohwEs",
    principal: "200.000000",
    returned: "199.790000",
  },
];

const RULES = [
  ["Max trade notional", "One oversized bet"],
  ["Max total notional", "Many bets adding up to one oversized bet"],
  ["Max concurrent positions", "Attention spread too thin"],
  ["Stop-loss required", "Trading without a floor"],
  ["Max risk per trade", "A stop so far away it is not a stop"],
  ["Max stop distance", "The same thing, measured in price"],
  ["Stop on the correct side", "A “stop” that is actually a target"],
  ["Allowed markets", "Trading outside the remit"],
  ["Max drawdown", "Trading on after the capital is impaired"],
  ["Minimum hold time", "Scalping the statistics"],
];

const TIERS = [
  ["Bronze", "—", "—", "—", "$10,000", "1"],
  ["Silver", "20", "1.20×", "—", "$25,000", "2"],
  ["Gold", "50", "1.50×", "≤ 4%", "$75,000", "3"],
  ["Platinum", "100", "1.80×", "≤ 4%", "$200,000", "5"],
];

const NAV = [
  ["#how", "How it works"],
  ["#rules", "Rules"],
  ["#money", "Economics"],
  ["#proof", "Proof"],
  ["#limits", "Limits"],
];

function Nav() {
  return (
    <header className="sticky top-0 z-50 border-b border-line-soft bg-[#050607]/95 backdrop-blur">
      <div className="mx-auto flex max-w-[1440px] items-center gap-8 px-4 py-4 md:px-8">
        <Link
          to="/nox"
          className="text-lg font-extrabold uppercase tracking-tight md:text-xl"
        >
          SOL-FX <span className="text-ink-dim">/</span>{" "}
          <span className="text-brand">NOXFUNDS</span>
        </Link>

        <nav className="hidden items-center gap-7 text-[13px] lg:flex">
          {NAV.map(([href, label]) => (
            <a
              key={href}
              href={href}
              className="text-ink-muted transition-colors hover:text-brand"
            >
              {label}
            </a>
          ))}
        </nav>

        <a
          href="https://github.com/ahmedmustafakhan125-coder/SOLFX/blob/main/docs/NOXFUNDS.md"
          target="_blank"
          rel="noreferrer"
          className="nox-cta ml-auto px-5 py-2 text-[11px] font-bold uppercase tracking-[0.14em]"
        >
          Read the spec
        </a>
      </div>
    </header>
  );
}

function Section({
  id,
  eyebrow,
  title,
  children,
}: {
  id?: string;
  eyebrow: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section id={id} className="border-t border-line-soft px-4 py-20 md:px-8">
      <div className="mx-auto max-w-[1200px]">
        <div className="text-[10px] uppercase tracking-[0.22em] text-brand">
          {eyebrow}
        </div>
        <h2 className="mt-3 max-w-3xl text-2xl font-extrabold uppercase tracking-tight md:text-[28px]">
          {title}
        </h2>
        {/* The short rule under a heading, straight from the reference. */}
        <div className="mt-4 h-0.5 w-14 bg-brand" />
        <div className="mt-10">{children}</div>
      </div>
    </section>
  );
}

export function Nox() {
  return (
    <div data-product="nox" className="min-h-screen bg-bg text-ink">
      <ProductSwitcher active="nox" />
      <Nav />

      {/* ------------------------------------------------------------------ hero */}
      <section className="relative overflow-hidden px-4 py-28 md:px-8 md:py-36">
        <div className="nox-grid pointer-events-none absolute inset-0" />
        <div className="nox-glow pointer-events-none absolute inset-0" />

        <div className="relative z-10 mx-auto max-w-[1200px]">
          <div className="flex w-fit items-center gap-2 border border-brand/30 bg-brand/5 px-3 py-1.5">
            <span className="h-1.5 w-1.5 rounded-full bg-brand" />
            <span className="tnum text-[10px] uppercase tracking-[0.18em] text-brand-soft">
              System online / Solana devnet
            </span>
          </div>

          <h1 className="mt-8 max-w-4xl text-4xl font-extrabold uppercase leading-[1.03] tracking-tight md:text-6xl">
            A prop firm where a rule-breaking trade
            <br className="hidden md:block" />
            <span className="text-brand"> cannot happen.</span>
          </h1>

          <p className="mt-8 max-w-2xl text-base leading-relaxed text-ink-muted">
            Every other prop firm watches trades after they execute and punishes
            violations, because their contracts cannot see an order before the
            venue fills it. NOXFUNDS owns the venue. A trade that breaks the
            rules is not detected and punished —{" "}
            <span className="text-ink">
              it fails as a transaction. It never existed, and the investor
              never took the loss.
            </span>
          </p>

          <div className="mt-10 flex flex-wrap gap-3">
            <a
              href="#how"
              className="nox-cta px-7 py-3 text-[11px] font-bold uppercase tracking-[0.14em]"
            >
              See how it works
            </a>
            <a
              href="#proof"
              className="border border-line px-7 py-3 text-[11px] font-bold uppercase tracking-[0.14em] text-ink-muted transition-colors hover:border-brand hover:text-brand"
            >
              Verify on chain
            </a>
          </div>

          <dl className="mt-20 grid max-w-4xl grid-cols-2 gap-px border border-line bg-line md:grid-cols-4">
            {[
              ["Rules checked", "before the fill"],
              ["Trader can withdraw", "never"],
              ["Protocol fee on a loss", "zero"],
              ["Mandates settled", "3 on devnet"],
            ].map(([k, v]) => (
              <div key={k} className="bg-bg p-4">
                <dt className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                  {k}
                </dt>
                <dd className="mt-1.5 text-sm font-bold text-ink">{v}</dd>
              </div>
            ))}
          </dl>
        </div>
      </section>

      {/* ------------------------------------------------------------- the diagram */}
      <Section
        id="how"
        eyebrow="How it works"
        title="One investor, one trader, one pot of money, one rule set that cannot change."
      >
        <NoxFlow />
      </Section>

      {/* ---------------------------------------------------------------- custody */}
      <Section
        eyebrow="Custody"
        title="Why the trader cannot run off with the money"
      >
        <div className="grid gap-10 md:grid-cols-2">
          <div className="space-y-4 text-sm leading-relaxed text-ink-muted">
            <p>
              The mandate's money sits at a{" "}
              <span className="text-ink">program derived address</span> — an
              address built so that no private key for it exists or can exist.
              Nobody holds the key, because there is no key.
            </p>
            <p>
              Money moves only when the Solana runtime re-derives that address
              from the seeds{" "}
              <span className="text-ink">and the calling program's ID</span>,
              and finds a match. The address belongs to the NOXFUNDS program, so
              only the NOXFUNDS program can authorise it.
            </p>
            <p className="text-ink">
              Not the trader. Not the investor. Not the admin. Not the person
              who deployed it. There is no withdraw instruction a trader can
              call — not a guarded one, none at all.
            </p>
          </div>

          <div className="grid gap-px border border-line bg-line sm:grid-cols-2">
            <div className="bg-bg p-5">
              <div className="text-[10px] uppercase tracking-[0.16em] text-long">
                A trader can
              </div>
              <ul className="mt-3 space-y-2 text-sm text-ink-muted">
                <li>Open positions</li>
                <li>Close positions</li>
                <li>Cancel their own stop-loss</li>
              </ul>
            </div>
            <div className="bg-bg p-5">
              <div className="text-[10px] uppercase tracking-[0.16em] text-short">
                A trader cannot
              </div>
              <ul className="mt-3 space-y-2 text-sm text-ink-muted">
                <li>Withdraw anything</li>
                <li>Change the rules</li>
                <li>Change who gets paid</li>
                <li>Trade a market outside the mandate</li>
              </ul>
            </div>
          </div>
        </div>
      </Section>

      {/* ------------------------------------------------------------------ rules */}
      <Section
        id="rules"
        eyebrow="The rulebook"
        title="Ten rules, each with its own error, each checked before the exchange is called."
      >
        <div className="grid gap-px border border-line bg-line sm:grid-cols-2">
          {RULES.map(([rule, stops], i) => (
            <div key={rule} className="relative bg-bg p-5">
              <span className="tnum absolute right-4 top-4 text-[10px] tracking-wider text-ink-dim/60">
                NOX-R{String(i + 1).padStart(2, "0")}
              </span>
              <div className="pr-20 text-sm font-bold text-ink">{rule}</div>
              <div className="mt-1.5 text-xs text-ink-dim">Stops: {stops}</div>
            </div>
          ))}
        </div>

        <div className="mt-8 grid gap-6 md:grid-cols-2">
          <div className="border-l-2 border-brand pl-5">
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              The rule nobody else can enforce
            </div>
            <p className="mt-2 text-sm leading-relaxed text-ink-muted">
              <span className="text-ink">Risk per trade</span> is checked before
              the fill, and it is only checkable because the stop-loss is
              mandatory and lands in the same transaction as the position. A
              centralised firm can only measure your risk once you already have
              the position.
            </p>
          </div>
          <div className="border-l-2 border-line pl-5">
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              And one rule that knows when to stop
            </div>
            <p className="mt-2 text-sm leading-relaxed text-ink-muted">
              The minimum hold time applies to{" "}
              <span className="text-ink">voluntary</span> closes only. A
              stop-out is exempt — forcing a trader to sit in a losing position
              to satisfy an anti-scalping rule would be a rule that causes the
              loss it exists to prevent.
            </p>
          </div>
        </div>
      </Section>

      {/* ------------------------------------------------------------------ money */}
      <Section
        id="money"
        eyebrow="The money"
        title="5% of gross to the protocol, then 70/30 of what is left."
      >
        <div className="grid gap-10 lg:grid-cols-[minmax(0,420px)_1fr]">
          <div className="panel p-6">
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              Worked example
            </div>
            <table className="mt-4 w-full text-sm">
              <tbody className="tnum">
                {[
                  ["Principal", "$3,500", false],
                  ["Final equity", "$4,500", false],
                  ["Gross profit", "$1,000", true],
                  ["− protocol fee, 5%", "$50", false],
                  ["Net", "$950", true],
                  ["→ trader, 70% of net", "$665", false],
                  ["→ investor, principal + 30%", "$3,785", false],
                ].map(([label, value, strong]) => (
                  <tr
                    key={label as string}
                    className="border-b border-line-soft"
                  >
                    <td
                      className={`py-2.5 pr-4 ${strong ? "font-bold text-ink" : "text-ink-muted"}`}
                    >
                      {label}
                    </td>
                    <td
                      className={`py-2.5 text-right ${strong ? "font-bold text-ink" : "text-ink-muted"}`}
                    >
                      {value}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="space-y-5 text-sm leading-relaxed text-ink-muted">
            <p>
              The 5% comes off <span className="text-ink">gross</span> profit
              first, so the 70/30 split stays exactly 70/30 on what remains. The
              alternative — a three-way 70/25/5 — would quietly cut the investor
              to 25%.
            </p>
            <p>
              <span className="text-ink">
                On a loss there is no fee and no trader share.
              </span>{" "}
              The investor receives everything left. The protocol never earns
              from a losing mandate and the trader is never charged for one.
              That is what "the investor bears the trading loss" means in money.
            </p>
            <div>
              <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                Rounding
              </div>
              <ul className="mt-2 space-y-1.5">
                <li>
                  The protocol fee rounds <span className="text-ink">up</span> —
                  a charge rounds toward whoever charges it.
                </li>
                <li>
                  The trader's share rounds{" "}
                  <span className="text-ink">down</span> — a payout rounds down.
                </li>
                <li>
                  The investor receives the remainder, by subtraction rather
                  than a third division — so the parts sum to the whole by
                  construction, and the dust goes to the party whose capital was
                  at risk.
                </li>
              </ul>
            </div>
          </div>
        </div>
      </Section>

      {/* ------------------------------------------------------------------ tiers */}
      <Section
        eyebrow="Track record"
        title="One profile per trader, every trade from every mandate, no way to start fresh."
      >
        <p className="mb-8 max-w-3xl text-sm leading-relaxed text-ink-muted">
          A 90% win rate next to a 0.6 profit factor is a trader taking tiny
          wins and enormous losses — the most common way a track record lies. So
          win rate never decides a tier on its own. Profit factor and worst
          drawdown do the work. The thresholds are{" "}
          <span className="text-ink">constants in the program</span>, not
          admin-settable fields: a threshold an admin can move is one they can
          move after seeing who it would promote.
        </p>

        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] border border-line text-sm">
            <thead>
              <tr className="bg-surface text-left text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                <th className="px-4 py-3 font-medium">Tier</th>
                <th className="px-4 py-3 text-right font-medium">Trades</th>
                <th className="px-4 py-3 text-right font-medium">
                  Profit factor
                </th>
                <th className="px-4 py-3 text-right font-medium">
                  Max drawdown
                </th>
                <th className="px-4 py-3 text-right font-medium">
                  Max mandate
                </th>
                <th className="px-4 py-3 text-right font-medium">Concurrent</th>
              </tr>
            </thead>
            <tbody className="tnum">
              {TIERS.map(([tier, ...cells]) => (
                <tr key={tier} className="border-t border-line-soft">
                  <td className="px-4 py-3 font-bold text-ink">{tier}</td>
                  {cells.map((c, i) => (
                    <td
                      key={`${tier}-${i}`}
                      className="px-4 py-3 text-right text-ink-muted"
                    >
                      {c}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <p className="mt-6 max-w-3xl text-sm leading-relaxed text-ink-muted">
          Note what Platinum asks for that the others do not:{" "}
          <span className="text-ink">two mandates settled in profit</span>. A
          live result with real investor money returned — not a statistic
          computed from trades inside a mandate that is still open. Silver also
          requires a 45% win rate as an additional condition.
        </p>
      </Section>

      {/* ------------------------------------------------------------------ proof */}
      <Section
        id="proof"
        eyebrow="Proof"
        title="Three mandates have run end to end on devnet. Go and read them."
      >
        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] border border-line text-sm">
            <thead>
              <tr className="bg-surface text-left text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                <th className="px-4 py-3 font-medium">Mandate</th>
                <th className="px-4 py-3 text-right font-medium">Principal</th>
                <th className="px-4 py-3 text-right font-medium">
                  Returned to investor
                </th>
                <th className="px-4 py-3 text-right font-medium">State</th>
              </tr>
            </thead>
            <tbody>
              {MANDATES.map((m) => (
                <tr key={m.address} className="border-t border-line-soft">
                  <td className="px-4 py-3">
                    <a
                      href={explorer(m.address)}
                      target="_blank"
                      rel="noreferrer"
                      className="tnum text-xs text-brand-soft hover:underline"
                    >
                      {m.address}
                    </a>
                  </td>
                  <td className="tnum px-4 py-3 text-right text-ink-muted">
                    {m.principal}
                  </td>
                  <td className="tnum px-4 py-3 text-right text-ink-muted">
                    {m.returned}
                  </td>
                  <td className="px-4 py-3 text-right text-xs uppercase tracking-wider text-ink-dim">
                    Settled
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <p className="mt-6 max-w-3xl text-sm leading-relaxed text-ink-muted">
          All three lost a small amount. That is the honest result of opening
          and closing a position in the same minute: you pay the spread and the
          fee in both directions. The protocol earned nothing from any of them,
          exactly as designed. They are shown as they are because a page that
          only lists wins is the thing this product exists to replace.
        </p>

        <div className="mt-8 grid gap-px border border-line bg-line sm:grid-cols-2">
          <div className="bg-bg p-5">
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              NOXFUNDS program
            </div>
            <a
              href={explorer(NOX_PROGRAM)}
              target="_blank"
              rel="noreferrer"
              className="tnum mt-2 block break-all text-xs text-brand-soft hover:underline"
            >
              {NOX_PROGRAM}
            </a>
          </div>
          <div className="bg-bg p-5">
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              The exchange underneath
            </div>
            <a
              href={explorer(SOLFX_PROGRAM)}
              target="_blank"
              rel="noreferrer"
              className="tnum mt-2 block break-all text-xs text-brand-soft hover:underline"
            >
              {SOLFX_PROGRAM}
            </a>
          </div>
        </div>
      </Section>

      {/* ----------------------------------------------------------------- limits */}
      <Section
        id="limits"
        eyebrow="Limits"
        title="What is not built, and where trust is still required."
      >
        <div className="grid gap-8 md:grid-cols-3">
          <div>
            <div className="text-[10px] uppercase tracking-[0.16em] text-warn">
              Not built
            </div>
            <ul className="mt-3 space-y-2 text-sm text-ink-muted">
              <li>The evaluation stage — mandates are funded directly today</li>
              <li>The marketplace for browsing and funding traders</li>
              <li>The public page that re-derives statistics from events</li>
              <li>
                An automated keeper — cranks are public but nothing runs them
              </li>
            </ul>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-[0.16em] text-short">
              Not done
            </div>
            <ul className="mt-3 space-y-2 text-sm text-ink-muted">
              <li>No security audit. Zero, by anyone</li>
              <li>No fuzzing</li>
              <li>Never on mainnet, and no mainnet date</li>
              <li>Largest mandate ever run: $200</li>
            </ul>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              Trust still required
            </div>
            <ul className="mt-3 space-y-2 text-sm text-ink-muted">
              <li>
                The program can be upgraded, and the upgrade authority is a
                single key
              </li>
              <li>
                That same key holds authority over SolFX — one signature
                controls both
              </li>
              <li>The admin can pause. The admin cannot move funds</li>
            </ul>
          </div>
        </div>

        <div className="mt-10 border border-warn/25 bg-warn/5 p-5 text-sm leading-relaxed text-warn">
          Green tests mean the behaviours someone thought to test behave as
          expected. Nothing more. NOXFUNDS is not audited, not safe, and not
          production-ready. This is devnet, with test USDC.
        </div>
      </Section>

      {/* ----------------------------------------------------------------- footer */}
      <footer className="border-t border-line-soft px-4 py-12 md:px-8">
        <div className="mx-auto flex max-w-[1200px] flex-wrap items-center gap-x-10 gap-y-5">
          <Link
            to="/nox"
            className="text-xl font-extrabold uppercase tracking-tight md:text-2xl"
          >
            SOL-FX <span className="text-ink-dim">/</span>{" "}
            <span className="text-brand">NOXFUNDS</span>
          </Link>

          <div className="flex flex-wrap gap-x-7 gap-y-2 text-[11px] uppercase tracking-[0.14em] text-ink-dim">
            <Link to="/" className="hover:text-brand">
              SolFX
            </Link>
            <Link to="/trade" className="hover:text-brand">
              Terminal
            </Link>
            <Link to="/about" className="hover:text-brand">
              About
            </Link>
            <a
              href="https://github.com/ahmedmustafakhan125-coder/SOLFX/blob/main/docs/NOXFUNDS.md"
              target="_blank"
              rel="noreferrer"
              className="hover:text-brand"
            >
              Specification
            </a>
          </div>

          <span className="ml-auto text-[11px] uppercase tracking-[0.14em] text-ink-dim">
            Devnet · test USDC · not audited
          </span>
        </div>
      </footer>
    </div>
  );
}
