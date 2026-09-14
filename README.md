# SolFX

A non-custodial forex brokerage on Solana, built for forex traders — lot-based sizing,
pip-denominated P&L, transparent swap rates, and a verifiable partner-rebate ledger — offering
the emerging-market pairs no venue on any chain lists, with collateral that never leaves your
own account.

**Status:** Phase 7 of 9 — two on-chain programs, an off-chain keeper, **490 tests**.
Deployed and verified on a local validator; devnet next. No real funds involved.

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
| [`docs/diagrams/`](docs/diagrams/) | Flowcharts (PNG + editable Graphviz sources) |
| [`docs/NOXFUNDS-PLAN.md`](docs/NOXFUNDS-PLAN.md) | **NOXFUNDS** — the prop-firm product built on top of SolFX. Design plan, not yet built |

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
| 8 | Frontend and SDK | — |
| 9 | Market expansion and hardening | — |

Commercial track (audits, legal, mainnet) is gated on traction or grant funding and is not
started speculatively.

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
cargo test --workspace                  # 490 tests
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
