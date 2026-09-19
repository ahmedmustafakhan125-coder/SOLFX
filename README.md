# SolFX

A non-custodial forex brokerage on Solana, built for forex traders — lot-based sizing,
pip-denominated P&L, transparent swap rates, and a verifiable partner-rebate ledger — offering
the emerging-market pairs no venue on any chain lists, with collateral that never leaves your
own account.

**Status: all nine phases complete, live on devnet, 988 tests green.** Three on-chain
programs, an off-chain keeper, a browser terminal, and **NOXFUNDS** — a decentralized prop firm
built on top of it, also live. Devnet only; no real funds involved, and no external audit.

---

## Documentation

Read in this order:

| Document | What it is |
|---|---|
| [`docs/CONTEXT.md`](docs/CONTEXT.md) | **Complete status of SolFX** — what's built, what's tested, what's open, deployment checklist |
| [`docs/ESSENTIALS.md`](docs/ESSENTIALS.md) | **What you need to be able to explain** — every key concept with a worked example |
| [`docs/FOREX-EXPLAINED.md`](docs/FOREX-EXPLAINED.md) | Plain English, assumes no FX or DeFi background. **Start here.** |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | The full technical specification |
| [`docs/oracle-feasibility.md`](docs/oracle-feasibility.md) | Phase 0b measurements — gates every risk parameter |
| [`docs/WORKFLOW.md`](docs/WORKFLOW.md) | How to find things and run them |
| [`docs/guides/`](docs/guides/README.md) | Per-phase guides: every function, formula and financial rule, with a PNG flowchart each |
| [`docs/test-cases/`](docs/test-cases/README.md) | The full test catalogue — automated tests documented per phase |
| [`docs/test-cases/localnet-manual.md`](docs/test-cases/localnet-manual.md) | **100 manual cases** against a live validator — what LiteSVM cannot cover |
| [`architecture/`](architecture/) | Flowcharts, one per phase (PNG + editable Graphviz sources) |
| [`docs/NOXFUNDS.md`](docs/NOXFUNDS.md) | **NOXFUNDS explained in plain English** — how it works, the rulebook, the money, and what is not built. Start here |
| [`docs/NOXFUNDS-PLAN.md`](docs/NOXFUNDS-PLAN.md) | NOXFUNDS — the full internal design and economics, 752 lines |
| [`docs/noxfunds-budgets.md`](docs/noxfunds-budgets.md) | NOXFUNDS compute, packet size and account locks, measured and asserted in CI |

## Progress

| Phase | Deliverable | State |
|---|---|---|
| 0a | Plain-English explainer | ✅ |
| 0b | Pyth feed feasibility measurement | ✅ — 29 listable markets; weekend-metals thesis refuted |
| 1 | Toolchain, workspace, `solfx-math` with property tests | ✅ |
| 2 | Vault, markets, Pyth pull oracle | ✅ — six oracle gates; markets listable without redeploy |
| 3 | Position engine | ✅ — full lifecycle on every market shape |
| 4 | Risk engine and market regimes | ✅ — liquidation waterfall; **the CHF-depeg replay found four defects** |
| 5 | LP vault | ✅ — the JIT liquidity attack fails |
| 6 | IB referral programme | ✅ — a second program; the ledger IBs can audit |
| **7** | **Keepers** | **✅ code complete** — 2 of 3 exit criteria; the rest needs devnet |
| 8 | Frontend and SDK | ✅ — browser terminal on [solfx.cloud](https://solfx.cloud); trades open and close with Phantom |
| 9 | Market expansion and hardening | ✅ — 97.14 % coverage, a 24-hour clean fuzz run, nine crisis replays |

### Live on devnet

| | |
|---|---|
| `solfx_core` | `2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi` — 9 markets listed, 6 active, 38 instructions |
| `solfx_referral` | `J7dwkNcyPjHtRyqkpnqq3wgE6XmYCaENhYZozX2MHsyt` |
| `noxfunds` | `9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx` — 16 instructions deployed; 34 built (marketplace + evaluation pending an upgrade) |

The off-chain half runs 24/7: price poster, keeper, RPC gateway and web tier.

## NOXFUNDS — the prop firm on top

Every other prop firm watches trades after they execute and punishes violations, because their
contracts cannot see an order before the venue fills it. NOXFUNDS owns the venue, so a trade
that breaks the mandate **is not detected and punished — it fails as a transaction.** It never
existed, and the investor never took the loss.

**[Read the full explanation in plain English →](docs/NOXFUNDS.md)** — no trading or Solana
knowledge assumed, with the three settled devnet mandates you can go and read yourself.

Live on devnet and initialised. What is built and tested:

| | |
|---|---|
| Mandate | An investor's capital and a rule set fixed at funding and never mutable. The principal moves into the mandate's own vault in the same instruction that creates it |
| Rules enforced before the fill | Market, concurrency, notional, stop distance, **risk at the stop** — the rule no centralized firm can check before a trade exists |
| Mandatory stop | Position and stop-loss are placed in one transaction, so "every funded trade carries a stop" is true rather than monitored |
| Track record and tiers | Realised P&L taken from the venue's own arithmetic, not recomputed. Drawdown cannot be hidden by refusing to close a loser |
| Settlement | Principal first, 5 % of gross, then 70/30 — and **permissionless**, so an investor never waits on the trader or the operator |
| Wind-down | A breached mandate reaches settlement with neither the trader nor the operator cooperating |
| Evaluation | A $50 refundable stake and two simulated phases. Simulated fills are priced by SolFX's own functions and match a real fill **to the unit** — the test opens both and compares |
| Marketplace | Listings on both sides, escrowed offers, decline with a reason, funding requests. **No chat** — every message is a typed on-chain object tied to a real step. In the browser at `/nox/market` |

Run a whole mandate against devnet — profile, funded mandate, a trade with its stop, the
equity crank, close and settlement — and read the accounts it leaves behind:

```bash
cargo run -p solfx-keeper --bin nox -- lifecycle             # plan and simulate, send nothing
cargo run -p solfx-keeper --bin nox -- lifecycle --execute
cargo run -p solfx-keeper --bin nox -- status
```

**Not deployed yet:** the evaluation and the marketplace are built and tested but await a program
upgrade on devnet. **Not built:** a screen for the evaluation, and a page that re-derives every
statistic from events. See [`docs/NOXFUNDS.md`](docs/NOXFUNDS.md) Part 10 for the full list.

## What is deliberately *not* trusted to the operator

Liquidations, stop execution, and NOXFUNDS' equity crank, wind-down and settlement are
permissionless by design. A venue that needs its operator present to liquidate, or an investor
who needs the operator to release their capital, fails exactly when the operator is absent.

Commercial track (audits, legal, mainnet) is gated on traction or grant funding and is not
started speculatively. **There has been no external audit**, and green tests mean the
behaviours someone thought to test behave as expected — nothing more.

## Verification — every figure measured, most of them asserted in CI

| | |
|---|---|
| **988 tests, 0 failures** | 721 Rust · 224 SDK · 43 app, none ignored (measured 2026-09-19) |
| **97.14 %** | SBF line coverage of `programs/solfx-core/src/instructions` |
| **56 property tests** | laws over the money paths, not examples — a round trip at an unchanged price always loses; fee splits conserve every unit |
| **24-hour fuzz run, 0 crashes** | 14,255,080 executions, 9/9 actions reached, asserting invariants I1, I2, I4–I8 rather than "no panic" |
| **9 crisis replays** | the 2015 CHF depeg, the 2016 sterling flash crash, COVID spreads, an EM devaluation, a weekend gap |
| **758 / 1,232 bytes** | `liquidate_position` against Solana's packet limit |
| **60,353 / 200,000 CU** | `open_position` on the widest market shape |
| **~497 opens/second** | structural ceiling, bounded by the 12 M compute cap on the one LP pool every trade write-locks |
| **99.88 % feed uptime** | 1,624 clean passes of 1,626 over a 16-hour soak |

Compute, packet size and account locks are pinned per instruction by
[`compute_budget.rs`](programs/solfx-core/tests/compute_budget.rs) and
[`budgets.rs`](programs/noxfunds/tests/budgets.rs), so a regression fails the build rather than
a transaction on a cluster.

## Layout

```
programs/solfx-core/      The protocol. Vault, markets, positions, risk engine, LP pool, triggers.
programs/solfx-referral/  The IB ledger. A separate program so growth can iterate while core freezes.
crates/solfx-math/        Fixed-point financial primitives. Zero dependencies, no floats, no panics.
crates/solfx-keeper/      Off-chain liquidator, trigger executor, cranks and feed watchdog.
docs/                     Specification, per-phase guides, test catalogue, flowcharts
scripts/                  Localnet deploy and keeper launch
scratch/feed-probe/       Phase 0b Pyth measurement tooling
```

## Building

```bash
anchor build                            # both programs
cargo test --workspace                  # 651 tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all

scripts/deploy-localnet.sh              # deploy to a local validator
scripts/run-keeper.sh --dry-run         # start the keepers, sending nothing
```

Build output is redirected to native Linux disk by [`.cargo/config.toml`](.cargo/config.toml).
Source lives on `/mnt/e`, which is a Windows drive mounted through WSL and roughly 200x slower
for the small-file I/O cargo does constantly. See [`docs/WORKFLOW.md`](docs/WORKFLOW.md).

## The three claims worth checking

Two properties are asserted across the full parameter space by
[`crates/solfx-math/tests/properties.rs`](crates/solfx-math/tests/properties.rs), and everything
else in the protocol depends on them holding:

1. **A round trip at an unchanged price always loses.** Open and immediately close, and the
   trader must end with less than they started with. If this ever passes, there is free money in
   the protocol and bots will extract it until the vault is empty.

2. **Rounding always favours the protocol.** Fees and margin requirements round up, payouts round
   down, signed PnL rounds toward −∞. The single most common way a financial program bleeds is a
   division that rounds the wrong way at one size.

3. **The engine agrees with a real broker to the cent.**
   [`crates/solfx-math/tests/broker_parity.rs`](crates/solfx-math/tests/broker_parity.rs)
   reproduces a live MT5 position — GBP/CHF, 60 lots, 1.09659 → 1.09850 — and asserts the
   engine returns **$14,141.69**, the figure the terminal showed. Integer arithmetic
   throughout; no floats anywhere in the check.

## Honest framing

The LP vault is a **B-book** — it is the counterparty to every trade and profits when traders
lose, structurally the same as XM or Exness. The defensible claim is not "no conflict of
interest"; it is that the vault is public and permissionless, the price comes from an oracle no
operator controls, the rules are open-source code that cannot change mid-trade, and collateral
stays in the user's own PDA.

SolFX is also **not** the first FX venue on Solana. GMTrade launched forex perpetuals in January
2026 and leads the chain by volume. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) § 1.

## Licence

MIT.
