import { Link } from "react-router-dom";

const PROGRAM_ID = "2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi";

function Stat({ k, v, note }: { k: string; v: string; note?: string }) {
  return (
    <div className="rounded-lg border border-line-soft bg-surface p-4">
      <div className="tnum text-2xl font-semibold text-ink">{v}</div>
      <div className="mt-1 text-xs font-medium text-ink">{k}</div>
      {note ? <div className="mt-1 text-[11px] leading-snug text-ink-dim">{note}</div> : null}
    </div>
  );
}

function Section({
  eyebrow,
  title,
  children,
}: {
  eyebrow: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="border-t border-line-soft px-6 py-16 md:px-10">
      <div className="mx-auto max-w-5xl">
        <div className="text-[10px] uppercase tracking-[0.2em] text-brand">{eyebrow}</div>
        <h2 className="mt-2 max-w-2xl text-2xl font-bold tracking-tight md:text-3xl">{title}</h2>
        <div className="mt-6">{children}</div>
      </div>
    </section>
  );
}

export function Landing() {
  return (
    <div className="min-h-screen bg-bg text-ink">
      {/* Hero */}
      <section className="relative overflow-hidden px-6 py-24 md:px-10 md:py-32">
        <div
          className="pointer-events-none absolute inset-0 opacity-[0.35]"
          style={{
            backgroundImage:
              "linear-gradient(rgba(153,69,255,0.06) 1px, transparent 1px), linear-gradient(90deg, rgba(153,69,255,0.06) 1px, transparent 1px)",
            backgroundSize: "56px 56px",
          }}
        />
        <div className="relative mx-auto max-w-5xl">
          <div className="inline-flex items-center gap-2 rounded-full border border-line px-3 py-1 text-[11px] text-ink-muted">
            <span className="h-1.5 w-1.5 rounded-full bg-long" />
            Live on Solana devnet
          </div>

          <h1 className="mt-6 max-w-3xl text-4xl font-extrabold leading-[1.05] tracking-tight md:text-6xl">
            A decentralized forex
            <br />
            brokerage.
          </h1>

          <p className="mt-6 max-w-2xl text-base leading-relaxed text-ink-muted">
            Margin FX, metals and crypto settled in USDC on Solana. Prices come from Pyth,
            positions live in your own wallet, and the rules that govern a trade are the
            program's — readable by anyone, applied before the fill rather than after.
          </p>

          <div className="mt-8 flex flex-wrap gap-3">
            <Link
              to="/trade"
              className="rounded-md bg-brand px-6 py-3 text-sm font-semibold text-white hover:bg-brand-dim"
            >
              Open the terminal
            </Link>
            <Link
              to="/about"
              className="rounded-md border border-line px-6 py-3 text-sm font-medium text-ink hover:border-brand"
            >
              About
            </Link>
          </div>

          <div className="mt-14 grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            <Stat k="Live markets" v="6" note="FX majors, a managed currency, gold, silver, BTC" />
            <Stat k="Settlement" v="USDC" note="Non-custodial; collateral stays in program vaults" />
            <Stat k="Oracle" v="Pyth" note="Full guardian verification, 60-second staleness gate" />
            <Stat k="Tests" v="549" note="0 failed, 0 ignored, clippy clean" />
          </div>
        </div>
      </section>

      {/* Non-custodial / immutability */}
      <Section
        eyebrow="Custody and control"
        title="The code can be frozen. The market list cannot be, and that is the point."
      >
        <div className="grid gap-6 md:grid-cols-2">
          <div className="space-y-4 text-sm leading-relaxed text-ink-muted">
            <p>
              A Solana program is upgradeable while an upgrade authority exists. Setting that
              authority to <span className="tnum text-ink">None</span> makes the bytecode
              permanently immutable — nobody, including us, can change it again. That is the
              intended end state once the protocol stops needing changes.
            </p>
            <p>
              Freezing the code does not freeze the venue. Listing a new pair is a{" "}
              <span className="text-ink">state</span> operation, not a code change: the engine
              is the formula and markets are rows. Twelve markets have been listed against one
              unchanged binary in the test suite, which is what makes this claim checkable
              rather than aspirational.
            </p>
            <p>
              So an admin key remains, and it is deliberately narrow — list a market, halt one,
              repoint a retired oracle, retune risk parameters. It cannot touch your collateral,
              cannot close your position, and cannot alter a rule the program enforces.
            </p>
          </div>
          <div className="rounded-lg border border-line-soft bg-surface p-5 text-xs">
            <div className="mb-3 text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              What each key can do
            </div>
            <table className="w-full">
              <tbody className="align-top">
                {[
                  ["Upgrade authority", "Replace the program's code. To be burned.", "text-warn"],
                  ["Protocol admin", "List / halt markets, repoint oracles, retune risk.", "text-brand-soft"],
                  ["Your wallet", "Deposit, open, close, withdraw. Only you.", "text-long"],
                ].map(([who, what, tone]) => (
                  <tr key={who} className="border-b border-line-soft/60 last:border-0">
                    <td className={`py-2.5 pr-4 font-medium ${tone}`}>{who}</td>
                    <td className="py-2.5 text-ink-muted">{what}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="mt-4 border-t border-line-soft pt-3 text-[11px] leading-relaxed text-ink-dim">
              Program <span className="tnum break-all text-ink-muted">{PROGRAM_ID}</span> — the
              IDL is published on chain, so any explorer can decode every instruction.
            </div>
          </div>
        </div>
      </Section>

      {/* What it does */}
      <Section eyebrow="The venue" title="Built for FX traders, not for perp traders.">
        <div className="grid gap-4 md:grid-cols-3">
          {[
            [
              "Sized the way you already trade",
              "Lots, quantity, or plain dollars of exposure. A lot means 100,000 units in FX, 100 ounces in gold and one coin in crypto — the conventions retail platforms use, not base units.",
            ],
            [
              "The swap rate is published",
              "Carry is a field on the market account, readable before you open. Most brokers reveal it after rollover. Here it is on chain, in advance, in basis points.",
            ],
            [
              "Rules enforced before the fill",
              "Leverage caps, minimum notional, slippage bounds and the oracle staleness gate are program constraints. A trade that breaks one is not flagged afterwards — it never executes.",
            ],
            [
              "Per-market risk, not one global number",
              "Leverage is 50x on EUR/USD and 10x on BTC because they are different instruments. Margin, spread and confidence limits are all per market.",
            ],
            [
              "Session-aware",
              "FX and metals trade Sunday 21:00 to Friday 21:00 UTC and are refused outside it. The oracle stops publishing at the close and the program will not price a stale market.",
            ],
            [
              "Your keys, your collateral",
              "Deposits sit in program vaults under PDA authority. There is no operator withdrawal path, and the accounting invariants that prove it are asserted in the test suite.",
            ],
          ].map(([h, b]) => (
            <div key={h} className="rounded-lg border border-line-soft bg-surface p-5">
              <h3 className="text-sm font-semibold">{h}</h3>
              <p className="mt-2 text-xs leading-relaxed text-ink-muted">{b}</p>
            </div>
          ))}
        </div>
      </Section>

      {/* Honest status */}
      <Section eyebrow="Status" title="What is true today, stated plainly.">
        <div className="grid gap-4 md:grid-cols-2">
          <div className="rounded-lg border border-long/25 bg-long/5 p-5">
            <div className="text-xs font-semibold text-long">Working</div>
            <ul className="mt-3 space-y-2 text-xs leading-relaxed text-ink-muted">
              <li>· Six markets live on devnet, priced and tradeable</li>
              <li>· Full round trip — open, hold, close — on every one of them</li>
              <li>· 549 tests passing, none ignored; property tests over the money paths</li>
              <li>· Liquidation, funding, carry, LP vault and IB rebates all implemented</li>
            </ul>
          </div>
          <div className="rounded-lg border border-warn/25 bg-warn/5 p-5">
            <div className="text-xs font-semibold text-warn">Not yet</div>
            <ul className="mt-3 space-y-2 text-xs leading-relaxed text-ink-muted">
              <li>· <strong className="text-ink">No external audit.</strong> None. Fuzzing is planned, not done</li>
              <li>· Devnet only — this is test money and nothing here is production</li>
              <li>· The upgrade authority still exists, so the code is not yet immutable</li>
              <li>· A verifiable build is roadmap; today you verify by reading the source</li>
            </ul>
          </div>
        </div>
      </Section>

      {/* A-book — last, as the closing argument */}
      <Section
        eyebrow="Where this is going"
        title="Today the pool is the counterparty. A-book is the destination."
      >
        <div className="grid gap-6 md:grid-cols-2">
          <div className="space-y-4 text-sm leading-relaxed text-ink-muted">
            <p>
              SolFX is B-book: a capitalised LP pool takes the other side of every trade. That
              is how GMX, gTrade and Hyperliquid's HLP work too — it is the on-chain norm, not
              a shortcut, and it is what lets the venue exist without a banking relationship.
            </p>
            <p>
              What we set out to build was A-book — orders passed through to real interbank
              liquidity, the broker earning commission instead of taking the other side. The
              obstacle was never the technology. It is licensing: routing retail FX to a prime
              broker requires being a regulated broker or an FCM.
            </p>
            <p className="text-ink">That gate has started to move.</p>
            <p>
              Banks now settle with Visa in USDC — over Solana. The CFTC opened stablecoin
              margin at futures commission merchants in December 2025 and widened it in
              February 2026, and by July 2026 an FCM was accepting USDC as initial margin
              against derivatives.
            </p>
            <p>
              Retail FX is <span className="text-ink">not</span> in scope yet, and those
              permissions rest on no-action letters rather than settled rules. So we are honest
              about the present: the pool is the counterparty today. The hedging bridge is built
              at a defined seam — per-market open-interest limits — so that when the gate opens,
              flow above a threshold routes out instead of being warehoused.
            </p>
          </div>

          <div className="space-y-3">
            {[
              ["Today", "B-book. LP pool is the counterparty, capitalised and transparent.", true],
              ["Seam already built", "Per-market OI caps define where hedging plugs in.", true],
              ["Gate", "Retail FX access to stablecoin-margined prime liquidity.", false],
              ["Then", "Hybrid: internalise small flow, route large or toxic flow out.", false],
            ].map(([label, body, done]) => (
              <div
                key={label as string}
                className={`rounded-lg border p-4 ${
                  done ? "border-brand/30 bg-brand/5" : "border-line-soft bg-surface"
                }`}
              >
                <div className="flex items-center gap-2">
                  <span
                    className={`h-1.5 w-1.5 rounded-full ${done ? "bg-brand" : "bg-ink-dim"}`}
                  />
                  <span className="text-xs font-semibold">{label as string}</span>
                </div>
                <p className="mt-1.5 pl-3.5 text-xs leading-relaxed text-ink-muted">
                  {body as string}
                </p>
              </div>
            ))}
            <p className="pt-2 text-[11px] leading-relaxed text-ink-dim">
              Sources: Visa USDC settlement over Solana; CFTC no-action letters 25-40 and 26-05;
              Marex accepting USDC as initial margin, July 2026.
            </p>
          </div>
        </div>
      </Section>

      <footer className="border-t border-line-soft px-6 py-10 md:px-10">
        <div className="mx-auto flex max-w-5xl flex-wrap items-center justify-between gap-4 text-xs text-ink-dim">
          <div>
            <span className="font-semibold text-ink">SolFX</span> — a decentralized forex
            brokerage
          </div>
          <div className="flex gap-5">
            <Link to="/trade" className="hover:text-ink">Terminal</Link>
            <Link to="/about" className="hover:text-ink">About</Link>
          </div>
          <div>Devnet · not investment advice · unaudited</div>
        </div>
      </footer>
    </div>
  );
}
