import { Link } from "react-router-dom";

import { Wordmark } from "@/components/Logo";
import { ProductSwitcher } from "@/components/ProductSwitcher";
import { TICKER, useTicker } from "@/hooks/useTicker";

const PROGRAM_ID = "2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi";

function Nav() {
  return (
    <header className="sticky top-0 z-50 border-b border-line bg-bg/95 backdrop-blur">
      <div className="mx-auto flex max-w-[1440px] items-center gap-8 px-4 py-4 md:px-8">
        <Link to="/" className="text-brand-soft">
          <Wordmark product="solfx" />
        </Link>
        <nav className="hidden items-center gap-6 text-sm md:flex">
          <Link
            to="/trade"
            className="border-b-2 border-brand-soft pb-1 font-bold uppercase tracking-wider text-brand-soft"
          >
            Trade
          </Link>
          <Link
            to="/trade"
            className="uppercase tracking-wider text-ink-muted hover:text-ink"
          >
            Markets
          </Link>
          <Link
            to="/nox"
            className="uppercase tracking-wider text-ink-muted hover:text-ink"
          >
            Noxfunds
          </Link>
          <Link
            to="/about"
            className="uppercase tracking-wider text-ink-muted hover:text-ink"
          >
            About
          </Link>
          <a
            href="#verification"
            className="uppercase tracking-wider text-ink-muted hover:text-ink"
          >
            Verification
          </a>
          <a
            href="#scope"
            className="uppercase tracking-wider text-ink-muted hover:text-ink"
          >
            Scope
          </a>
          <a
            href="#status"
            className="uppercase tracking-wider text-ink-muted hover:text-ink"
          >
            Status
          </a>
        </nav>
        <div className="ml-auto">
          <Link
            to="/trade"
            className="bg-brand px-5 py-2.5 text-sm font-semibold text-white hover:bg-brand-dim"
          >
            Launch Terminal
          </Link>
        </div>
      </div>
    </header>
  );
}

function Quote({ t }: { t: (typeof TICKER)[number] }) {
  const quotes = useTicker();
  const q = quotes[t.symbol];
  const up = (q?.changePct ?? 0) >= 0;
  return (
    <div className="panel flex items-center gap-4 p-4">
      <div className="flex h-12 w-12 shrink-0 items-center justify-center border border-line bg-surface-highest text-brand-soft">
        <span className="text-lg font-semibold">{t.glyph}</span>
      </div>
      <div className="min-w-0 flex-1">
        <div className="text-base font-bold">{t.symbol}</div>
        <div className="truncate text-[10px] uppercase tracking-wider text-ink-dim">
          {t.name}
        </div>
      </div>
      <div className="text-right">
        <div className="tnum text-base">
          {q ? q.price.toFixed(t.places) : "—"}
        </div>
        <div className={`tnum text-[11px] ${up ? "text-long" : "text-short"}`}>
          {q
            ? `${up ? "▲" : "▼"} ${q.changePct >= 0 ? "+" : ""}${q.changePct.toFixed(2)}%`
            : ""}
        </div>
      </div>
    </div>
  );
}

function Feature({
  icon,
  title,
  body,
}: {
  icon: string;
  title: string;
  body: string;
}) {
  return (
    <div className="panel p-6">
      <div className="flex h-10 w-10 items-center justify-center border border-line bg-surface-highest text-brand-soft">
        <span className="text-lg">{icon}</span>
      </div>
      <h3 className="mt-5 text-base font-bold uppercase tracking-wide">
        {title}
      </h3>
      <p className="mt-2 text-sm leading-relaxed text-ink-muted">{body}</p>
    </div>
  );
}

/**
 * The verification band.
 *
 * Every figure here is measured and reproducible from the repository — the commands are on
 * the page for exactly that reason. Nothing is rounded up, and the things that are *not*
 * true sit in the band immediately below this one rather than being left out.
 *
 * That pairing is deliberate. A venue asking people to trust it with money is more credible
 * for saying "no external audit" in the same breath as "97% coverage" than for saying only
 * the second. The numbers are good enough not to need help.
 */
/** Reproduces every figure in the verification band, in order. */
const CHECK_COMMANDS = `cargo test --workspace          # 721 passing, 0 ignored
npm --prefix clients/js test    # 224
npm --prefix app run test       # 43

anchor coverage                 # 97.14% of programs/solfx-core/src/instructions
cargo test -p solfx-math --test properties   # the 56 laws
cargo test -p solfx-core --test scenarios    # the 9 crisis replays

cargo clippy --workspace --all-targets       # clean; the lints are in Cargo.toml`;

const PROOF_STATS: readonly {
  figure: string;
  label: string;
  detail: string;
}[] = [
  {
    figure: "988",
    label: "tests passing",
    detail: "721 Rust · 224 SDK · 43 app · none ignored",
  },
  {
    figure: "97.14%",
    label: "coverage of the on-chain code",
    detail: "SBF source coverage across 38 instructions",
  },
  {
    figure: "56",
    label: "property tests",
    detail: "laws over the money paths, not examples",
  },
  {
    figure: "99.88%",
    label: "price-feed uptime",
    detail: "1,624 of 1,626 passes over 16h 23m",
  },
];

const PROOF_GROUPS: readonly {
  title: string;
  items: readonly string[];
}[] = [
  {
    title: "The arithmetic cannot drift",
    items: [
      "Every money path goes through one fixed-point module. No floating point anywhere in the protocol: not in a test, not in a log line.",
      "The build denies overflow, truncation, silent division, unwrap, panic and index-slicing. A money path cannot opt out.",
      "56 property tests assert laws rather than examples: a round trip at an unchanged price always loses, P&L is antisymmetric, fee splits conserve every unit, the book is never crossed.",
    ],
  },
  {
    title: "Solvency is checked, not assumed",
    items: [
      "Seven accounting invariants are re-asserted after individual instructions in the test suite: vault balances against the sum of accounts, open interest against live positions, LP supply against assets under management.",
      "Compute and packet ceilings are enforced by tests, so a regression fails CI rather than a transaction: open_position runs at about 55,000 to 59,000 units, a figure that moves in 1,500-unit steps as the address search varies, against asserted ceilings of 120,000 and 140,000, and a liquidation is 758 bytes against Solana's 1,232 limit.",
      "Refusal is tested as heavily as success. A venue that accepts everything is not a venue.",
    ],
  },
  {
    title: "It has been run against history",
    items: [
      "Nine crisis replays drive the engine through real events: the 2015 Swiss franc depeg, the 2016 sterling flash crash, COVID's sustained wide spreads, an emerging-market devaluation, and a weekend gap.",
      "Each one asserts the documented order of absorption, insurance fund first and then socialised loss, and that the venue refuses new risk without ever bricking.",
      "21.7 million fuzzing executions have found zero crashes.",
    ],
  },
];

/// Measured ceilings for both programs. Every figure is asserted by a test, so a regression
/// fails the build rather than turning this page into a lie.
const CEILINGS: readonly {
  metric: string;
  solfx: string;
  nox: string;
  limit: string;
}[] = [
  {
    metric: "Compute, heaviest instruction",
    solfx: "60,353",
    nox: "116,736",
    limit: "200,000 default",
  },
  {
    metric: "Transaction size",
    solfx: "758 bytes",
    nox: "863 bytes",
    limit: "1,232 bytes",
  },
  {
    metric: "Account locks",
    solfx: "18",
    nox: "21",
    limit: "64",
  },
  {
    metric: "Opens per second, structural",
    solfx: "~497",
    nox: "~265",
    limit: "12M CU per account, per block",
  },
];

/// What is deliberately devnet-shaped, and what mainnet would actually require. Written down
/// because the gap is the interesting part: a demo that claims to be production-ready is
/// telling you it has not looked.
const MAINNET_PATH: readonly {
  area: string;
  now: string;
  mainnet: string;
}[] = [
  {
    area: "Audit",
    now: "None. Zero external audits of either program.",
    mainnet:
      "Two independent audits covering both programs and the cross-program call between them, plus a funded bug bounty, before any real capital.",
  },
  {
    area: "Fuzzing",
    now: "21.7 million executions, zero crashes, but the Trident campaign has not run to its exit criterion.",
    mainnet:
      "Trident to completion against the stated criterion, then running continuously in CI rather than as a one-off.",
  },
  {
    area: "Price feeds",
    now: "Only 8 of 33 markets have a sponsored Pyth feed on devnet, so the protocol posts its own guardian-signed updates.",
    mainnet:
      "Pyth carries the mainnet aggregate for all 33. The self-posting path exists because devnet is thin, and it retires on mainnet. The on-chain verification requirement does not change.",
  },
  {
    area: "Redundancy",
    now: "One price-poster, one keeper, one RPC gateway. If the poster stops, every market halts within 60 seconds on the staleness gate.",
    mainnet:
      "Leader-elected posters across regions. Naively running two is measurably worse, not better: each pass is 10 sends against a 1-per-second limit, so a second instance roughly quadruples pass time.",
  },
  {
    area: "Throughput",
    now: "The RPC gateway is rate-limited to 9 requests per second.",
    mainnet:
      "A paid provider with failover. The gateway is the binding limit today and sits roughly 55× below what the programs themselves support.",
  },
  {
    area: "Collateral",
    now: "A test USDC mint.",
    mainnet:
      "Circle's USDC. The 6-decimal check already refuses anything else at the program level, so this is configuration rather than code.",
  },
  {
    area: "Keys",
    now: "A single hot key holds upgrade authority over both programs and receives protocol fees.",
    mainnet:
      "A Squads multisig as upgrade authority with a timelock, a separate treasury address, and hardware signing. One signature should never control both the code and the revenue.",
  },
  {
    area: "Scaling",
    now: "Every trade write-locks one shared LP pool, which caps the venue at the per-account compute limit.",
    mainnet:
      "Per-market pools if volume ever approaches that ceiling. This is a v2 redesign rather than a parameter, which is exactly why it is named now and not discovered later.",
  },
];

function Band({
  eyebrow,
  title,
  accent,
  sub,
  children,
}: {
  eyebrow?: string;
  title: string;
  accent?: string;
  sub?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="border-t border-line px-4 py-20 md:px-8">
      <div className="mx-auto max-w-[1440px]">
        <div className="mx-auto max-w-3xl text-center">
          {eyebrow ? (
            <div className="text-[10px] uppercase tracking-[0.2em] text-brand">
              {eyebrow}
            </div>
          ) : null}
          <h2 className="mt-2 text-3xl font-bold tracking-tight md:text-4xl">
            {title}{" "}
            {accent ? <span className="text-brand">{accent}</span> : null}
          </h2>
          {sub ? <p className="mt-3 text-sm text-ink-muted">{sub}</p> : null}
        </div>
        <div className="mt-12">{children}</div>
      </div>
    </section>
  );
}

export function Landing() {
  return (
    <div className="relative min-h-screen bg-bg text-ink">
      {/* The shared backdrop. Identical to NOXFUNDS'; only the accent hue differs. */}
      <div className="gridwork pointer-events-none fixed inset-0 z-0" />
      <div className="glowfield pointer-events-none fixed inset-0 z-0" />

      <div className="relative z-10">
        <ProductSwitcher active="solfx" />
        <Nav />

        {/* Hero */}
        <section className="relative flex min-h-[700px] items-center overflow-hidden px-4 py-24 md:px-8">
          <div className="relative z-10 mx-auto grid w-full max-w-[1440px] grid-cols-1 items-center gap-8 lg:grid-cols-12">
            <div className="flex flex-col gap-8 lg:col-span-7">
              <div className="flex w-fit items-center gap-2 border border-line bg-surface px-4 py-2">
                <span className="h-2 w-2 bg-long" />
                <span className="tnum text-xs uppercase tracking-wider text-ink-muted">
                  Devnet is live
                </span>
              </div>

              <h1 className="font-display text-4xl font-extrabold leading-[1.05] tracking-tight md:text-6xl">
                A Decentralized
                <br />
                Forex <span className="text-brand">Brokerage.</span>
              </h1>

              <p className="max-w-2xl text-lg leading-relaxed text-ink-muted">
                Margin FX, metals and crypto on Solana, settled in USDC. Prices
                come from Pyth, collateral stays in your own wallet's control,
                and the rules that govern a trade are the program's, applied
                before the fill rather than after.
              </p>

              <div className="flex flex-wrap items-center gap-4 pt-2">
                <Link
                  to="/trade"
                  className="bg-brand px-7 py-3.5 text-sm font-semibold text-white hover:bg-brand-dim"
                >
                  Trade Now →
                </Link>
                <Link
                  to="/about"
                  className="border border-line px-7 py-3.5 text-sm font-medium hover:border-brand"
                >
                  About the project
                </Link>
              </div>
            </div>

            <div className="relative z-10 mt-12 flex flex-col gap-4 lg:col-span-5 lg:mt-0">
              {TICKER.slice(0, 3).map((t) => (
                <Quote key={t.symbol} t={t} />
              ))}
              <p className="text-[11px] leading-relaxed text-ink-dim">
                Live from Pyth, the same feed the program prices against. Change
                is measured against the feed's own EMA.
              </p>
            </div>
          </div>
        </section>

        {/* Performance */}
        <Band
          title="Engineered for"
          accent="Precision"
          sub="Why this is built on Solana, and what that actually buys a forex trader."
        >
          <div className="grid gap-4 md:grid-cols-3">
            <Feature
              icon="⚡"
              title="Sub-second settlement"
              body="Roughly 400ms block times, so a fill confirms while you are still looking at it. Opening a position measures about 60,000 compute units against a 140,000 ceiling."
            />
            <Feature
              icon="◎"
              title="Fractions of a cent"
              body="A round trip on $1,000 of notional costs about $0.08 in protocol fees beyond the spread, measured on devnet across all six markets rather than estimated."
            />
            <Feature
              icon="⊞"
              title="One binary, many markets"
              body="The engine is the formula and markets are rows. Twelve were listed against a single unchanged program in the test suite, so new pairs need no redeploy."
            />
          </div>
        </Band>

        {/* Markets */}
        <Band
          title="Six live"
          accent="Markets"
          sub="Majors, a managed currency, both metals, and the one instrument that trades all weekend."
        >
          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
            {TICKER.map((t) => (
              <Quote key={t.symbol} t={t} />
            ))}
          </div>
          <p className="mx-auto mt-6 max-w-2xl text-center text-xs leading-relaxed text-ink-dim">
            FX and metals trade Sunday 21:00 to Friday 21:00 UTC and are
            correctly refused outside it. Pyth stops publishing at the close and
            the program will not price a stale market. Only BTC/USD is
            continuous.
          </p>
        </Band>

        {/* Custody */}
        <Band
          eyebrow="Custody and control"
          title="The code can be frozen. The market list"
          accent="cannot."
        >
          <div className="grid gap-6 lg:grid-cols-2">
            <div className="space-y-4 text-sm leading-relaxed text-ink-muted">
              <p>
                A Solana program stays upgradeable while an upgrade authority
                exists. Setting that authority to{" "}
                <span className="tnum text-ink">None</span> makes the bytecode
                permanently immutable, so nobody, including us, can change it
                again. That is the intended end state once the protocol stops
                needing changes.
              </p>
              <p>
                Freezing the code does not freeze the venue. Listing a pair is a{" "}
                <span className="text-ink">state</span> operation rather than a
                code change, which is why an admin key survives the burn. It is
                deliberately narrow: list a market, halt one, repoint a retired
                oracle, retune risk parameters.
              </p>
              <p>
                It cannot touch your collateral, cannot close your position, and
                cannot alter a rule the program enforces.
              </p>
            </div>
            <div className="panel p-6 text-sm">
              <div className="mb-4 text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                What each key can do
              </div>
              <table className="w-full">
                <tbody className="align-top">
                  {[
                    [
                      "Upgrade authority",
                      "Replace the program's code. To be burned.",
                      "text-warn",
                    ],
                    [
                      "Protocol admin",
                      "List and halt markets, repoint oracles, retune risk.",
                      "text-brand-soft",
                    ],
                    [
                      "Your wallet",
                      "Deposit, open, close, withdraw. Only you.",
                      "text-long",
                    ],
                  ].map(([who, what, tone]) => (
                    <tr
                      key={who}
                      className="border-b border-line-soft last:border-0"
                    >
                      <td className={`py-3 pr-4 font-medium ${tone}`}>{who}</td>
                      <td className="py-3 text-ink-muted">{what}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <div className="mt-5 border-t border-line-soft pt-4 text-[11px] leading-relaxed text-ink-dim">
                Program{" "}
                <span className="tnum break-all text-ink-muted">
                  {PROGRAM_ID}
                </span>
                . The IDL is published on chain, so any explorer can decode
                every instruction.
              </div>
            </div>
          </div>
        </Band>

        {/* Status */}
        {/* Verification: measured, reproducible, and paired with what is not true */}
        <section
          id="verification"
          className="border-t border-line px-4 py-20 md:px-8"
        >
          <div className="mx-auto max-w-[1440px]">
            <div className="mx-auto max-w-3xl text-center">
              <div className="text-[10px] uppercase tracking-[0.2em] text-brand">
                Verification
              </div>
              <h2 className="mt-2 text-3xl font-bold tracking-tight md:text-4xl">
                Every number here is{" "}
                <span className="text-brand">measured.</span>
              </h2>
              <p className="mt-3 text-sm text-ink-muted">
                Not estimated, not aspirational. Each one is reproducible from
                the repository, and the commands that produce it are at the
                bottom of this section.
              </p>
            </div>

            <div className="mt-12 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
              {PROOF_STATS.map((s) => (
                <div key={s.label} className="panel p-6">
                  <div className="font-mono text-3xl font-bold tracking-tight text-brand md:text-4xl">
                    {s.figure}
                  </div>
                  <div className="mt-2 text-sm font-bold uppercase tracking-wide">
                    {s.label}
                  </div>
                  <p className="mt-2 text-xs leading-relaxed text-ink-muted">
                    {s.detail}
                  </p>
                </div>
              ))}
            </div>

            <div className="mt-4 grid gap-4 lg:grid-cols-3">
              {PROOF_GROUPS.map((g) => (
                <div key={g.title} className="panel p-6">
                  <h3 className="text-base font-bold uppercase tracking-wide">
                    {g.title}
                  </h3>
                  <ul className="mt-4 space-y-3 text-sm leading-relaxed text-ink-muted">
                    {g.items.map((item) => (
                      <li key={item} className="flex gap-2">
                        <span className="mt-[2px] shrink-0 text-brand">·</span>
                        <span>{item}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>

            <div className="mt-4 panel p-6">
              <h3 className="text-base font-bold uppercase tracking-wide">
                Check it yourself
              </h3>
              <p className="mt-2 text-sm leading-relaxed text-ink-muted">
                The figures above come from these, run against the same commit
                that is deployed here.
              </p>
              <pre className="mt-4 overflow-x-auto border border-line bg-surface-highest p-4 font-mono text-xs leading-relaxed text-ink-muted">
                <code>{CHECK_COMMANDS}</code>
              </pre>
              <p className="mt-3 text-xs leading-relaxed text-ink-dim">
                The protocol is 38 instructions across two Anchor programs. It
                has never been upgraded since the markets you can trade here
                were listed. The extensibility claim was tested by listing two
                new markets against a byte-identical binary.
              </p>
            </div>
          </div>
        </section>

        <section id="scope" className="border-t border-line px-4 py-20 md:px-8">
          <div className="mx-auto max-w-[1440px]">
            <div className="mx-auto max-w-3xl text-center">
              <div className="text-[10px] uppercase tracking-[0.2em] text-brand">
                Scope
              </div>
              <h2 className="mt-2 text-3xl font-bold tracking-tight md:text-4xl">
                Built for devnet.{" "}
                <span className="text-brand">Designed for mainnet.</span>
              </h2>
              <p className="mt-3 text-sm text-ink-muted">
                This is a devnet deployment, and some of what you see is shaped
                by that. The gap is written down below rather than glossed over,
                because knowing precisely what separates a working protocol from
                a production one is the difference between a demo and a plan.
              </p>
            </div>

            <div className="mt-12 panel p-6">
              <h3 className="text-base font-bold uppercase tracking-wide">
                The ceilings, measured
              </h3>
              <p className="mt-2 text-sm leading-relaxed text-ink-muted">
                These do not change between devnet and mainnet. They are
                properties of the programs and of Solana, and every one is
                asserted by a test so a regression fails the build.
              </p>
              <div className="mt-6 overflow-x-auto">
                <table className="w-full min-w-[640px] border-collapse text-sm">
                  <thead>
                    <tr className="border-b border-line text-left text-[10px] uppercase tracking-[0.15em] text-ink-dim">
                      <th className="py-3 pr-4 font-bold">Metric</th>
                      <th className="py-3 pr-4 text-right font-bold">SolFX</th>
                      <th className="py-3 pr-4 text-right font-bold">
                        NOXFUNDS
                      </th>
                      <th className="py-3 text-right font-bold">Limit</th>
                    </tr>
                  </thead>
                  <tbody>
                    {CEILINGS.map((c) => (
                      <tr key={c.metric} className="border-b border-line/50">
                        <td className="py-3 pr-4 text-ink-muted">{c.metric}</td>
                        <td className="py-3 pr-4 text-right font-mono text-brand">
                          {c.solfx}
                        </td>
                        <td className="py-3 pr-4 text-right font-mono text-brand">
                          {c.nox}
                        </td>
                        <td className="py-3 text-right font-mono text-xs text-ink-dim">
                          {c.limit}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="mt-4 text-xs leading-relaxed text-ink-dim">
                Throughput is bounded by the 12,000,000 compute units Solana
                allows a single writable account per block, not by the
                100,000,000 block limit. Every trade touches the same LP pool,
                so the hot account tops out at 12% of the block. NOXFUNDS costs
                roughly twice a bare trade because it prices the market, then
                makes two cross-program calls that each price it again: that is
                what guarantees a funded position can never exist without its
                stop.
              </p>
            </div>

            <div className="mt-4 grid gap-4 md:grid-cols-2">
              {MAINNET_PATH.map((m) => (
                <div key={m.area} className="panel p-6">
                  <h3 className="text-base font-bold uppercase tracking-wide">
                    {m.area}
                  </h3>
                  <dl className="mt-4 space-y-3 text-sm leading-relaxed">
                    <div>
                      <dt className="text-[10px] uppercase tracking-[0.15em] text-ink-dim">
                        On devnet today
                      </dt>
                      <dd className="mt-1 text-ink-muted">{m.now}</dd>
                    </div>
                    <div>
                      <dt className="text-[10px] uppercase tracking-[0.15em] text-brand">
                        For mainnet
                      </dt>
                      <dd className="mt-1 text-ink-muted">{m.mainnet}</dd>
                    </div>
                  </dl>
                </div>
              ))}
            </div>

            <div className="mt-4 panel p-6">
              <p className="text-sm leading-relaxed text-ink-muted">
                <span className="font-bold text-ink">
                  What this list is not:
                </span>{" "}
                a roadmap of features. Every item is a thing that must be true
                before somebody else&rsquo;s money is at risk, and none of it is
                blocked on the protocol working. The protocol works. It is
                blocked on the review, the redundancy and the key management
                that a devnet deployment does not need and a mainnet one cannot
                open without.
              </p>
            </div>
          </div>
        </section>

        <section
          id="status"
          className="border-t border-line px-4 py-20 md:px-8"
        >
          <div className="mx-auto max-w-[1440px]">
            <div className="mx-auto max-w-3xl text-center">
              <div className="text-[10px] uppercase tracking-[0.2em] text-brand">
                Status
              </div>
              <h2 className="mt-2 text-3xl font-bold tracking-tight md:text-4xl">
                And what is <span className="text-brand">not.</span>
              </h2>
              <p className="mt-3 text-sm text-ink-muted">
                The numbers above are good. They are also not the same thing as
                safety, and the difference is worth stating plainly.
              </p>
            </div>
            <div className="mt-12 grid gap-4 md:grid-cols-2">
              <div className="border border-long/25 bg-long/5 p-6">
                <div className="text-xs font-bold uppercase tracking-wider text-long">
                  Working
                </div>
                <ul className="mt-4 space-y-2 text-sm leading-relaxed text-ink-muted">
                  <li>· Six markets live on devnet, priced and tradeable</li>
                  <li>
                    · Full round trip, open then hold then close, verified on
                    every one
                  </li>
                  <li>
                    · 788 tests passing, none ignored; 97% coverage of the
                    on-chain code
                  </li>
                  <li>
                    · Liquidation, funding, carry, LP vault and IB rebates
                    implemented
                  </li>
                </ul>
              </div>
              <div className="border border-warn/25 bg-warn/5 p-6">
                <div className="text-xs font-bold uppercase tracking-wider text-warn">
                  Not yet
                </div>
                <ul className="mt-4 space-y-2 text-sm leading-relaxed text-ink-muted">
                  <li>
                    · <strong className="text-ink">No external audit.</strong>{" "}
                    None. Tests prove what someone thought to test
                  </li>
                  <li>
                    · Fuzzing has run 21.7M executions with zero crashes, but
                    not yet the 24 continuous hours the exit criterion asks for
                  </li>
                  <li>
                    · Devnet only, on test money, and nothing here is production
                  </li>
                  <li>
                    · The upgrade authority still exists, so the code is not yet
                    immutable
                  </li>
                  <li>
                    · A verifiable build is roadmap; today you verify by reading
                    the source
                  </li>
                </ul>
              </div>
            </div>
          </div>
        </section>

        {/* A-book, last */}
        <Band
          eyebrow="Where this is going"
          title="Today the pool is the counterparty. A-book is the"
          accent="destination."
        >
          <div className="grid gap-6 lg:grid-cols-2">
            <div className="space-y-4 text-sm leading-relaxed text-ink-muted">
              <p>
                SolFX is B-book: a capitalised LP pool takes the other side of
                every trade. That is how GMX, gTrade and Hyperliquid's HLP work
                too. It is the on-chain norm, not a shortcut, and what lets the
                venue exist without a banking relationship.
              </p>
              <p>
                What this set out to be was A-book, with orders passed through
                to real interbank liquidity, the broker earning commission
                instead of taking the other side. The obstacle was never
                technology. It is licensing: routing retail FX to a prime broker
                requires being a regulated broker or an FCM.
              </p>
              <p className="text-ink">That gate has started to move.</p>
              <p>
                Banks now settle with Visa in USDC, over Solana. The CFTC opened
                stablecoin margin at futures commission merchants in December
                2025 and widened it in February 2026, and by July 2026 an FCM
                was accepting USDC as initial margin against derivatives.
              </p>
              <p>
                Retail FX is <span className="text-ink">not</span> in scope yet,
                and those permissions rest on no-action letters rather than
                settled rules. So the present is stated plainly: the pool is the
                counterparty today. The hedging bridge sits at a defined seam,
                the per-market open-interest limits, so when the gate opens flow
                above a threshold routes out instead of being warehoused.
              </p>
            </div>

            <div className="space-y-3">
              {[
                [
                  "Today",
                  "B-book. The LP pool is the counterparty, capitalised and transparent.",
                  true,
                ],
                [
                  "Seam already built",
                  "Per-market OI caps define exactly where hedging plugs in.",
                  true,
                ],
                [
                  "The gate",
                  "Retail FX access to stablecoin-margined prime liquidity.",
                  false,
                ],
                [
                  "Then",
                  "Hybrid: internalise small flow, route large or toxic flow out.",
                  false,
                ],
              ].map(([label, body, done]) => (
                <div
                  key={label as string}
                  className={`border p-5 ${done ? "border-brand/40 bg-brand/5" : "border-line bg-surface"}`}
                >
                  <div className="flex items-center gap-2">
                    <span
                      className={`h-2 w-2 ${done ? "bg-brand" : "bg-ink-dim"}`}
                    />
                    <span className="text-xs font-bold uppercase tracking-wider">
                      {label as string}
                    </span>
                  </div>
                  <p className="mt-2 pl-4 text-sm leading-relaxed text-ink-muted">
                    {body as string}
                  </p>
                </div>
              ))}
              <p className="pt-2 text-[11px] leading-relaxed text-ink-dim">
                Sources: Visa USDC settlement over Solana; CFTC no-action
                letters 25-40 and 26-05; Marex accepting USDC as initial margin,
                July 2026.
              </p>
            </div>
          </div>
        </Band>

        <footer className="border-t border-line px-4 py-10 md:px-8">
          <div className="mx-auto flex max-w-[1440px] flex-wrap items-center justify-between gap-4">
            <span className="text-xl font-extrabold tracking-tighter text-brand-soft">
              SOL-FX
            </span>
            <div className="flex gap-6 text-xs uppercase tracking-wider text-ink-dim">
              <Link to="/trade" className="hover:text-ink">
                Terminal
              </Link>
              <Link to="/about" className="hover:text-ink">
                About
              </Link>
              <a href="#status" className="hover:text-ink">
                Status
              </a>
            </div>
            <div className="text-xs text-ink-dim">
              © 2026 SolFX · devnet · unaudited · not investment advice
            </div>
          </div>
        </footer>
      </div>
    </div>
  );
}
