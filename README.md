# SolFX

A non-custodial forex brokerage on Solana, built for forex traders — lot-based sizing,
pip-denominated P&L, transparent swap rates, and a verifiable partner-rebate ledger — offering
the emerging-market pairs no venue on any chain lists, with collateral that never leaves your
own account.

**Status:** Phase 1 of 9. Devnet capstone track. Not deployed anywhere; no real funds involved.

---

## Documentation

Read in this order:

| Document | What it is |
|---|---|
| [`docs/ESSENTIALS.md`](docs/ESSENTIALS.md) | **What you need to be able to explain** — every key concept with a worked example |
| [`docs/FOREX-EXPLAINED.md`](docs/FOREX-EXPLAINED.md) | Plain English, assumes no FX or DeFi background. **Start here.** |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | The full technical specification |
| [`docs/oracle-feasibility.md`](docs/oracle-feasibility.md) | Phase 0b measurements — gates every risk parameter |
| [`docs/WORKFLOW.md`](docs/WORKFLOW.md) | How to find things and run them |
| [`docs/guides/`](docs/guides/README.md) | Per-phase guides: every function, formula and financial rule, with a PNG flowchart each |
| [`docs/test-cases/`](docs/test-cases/README.md) | The full test catalogue — 422 tests documented per phase |
| [`docs/diagrams/`](docs/diagrams/) | Flowcharts (PNG + editable Graphviz sources) |

## Progress

| Phase | Deliverable | State |
|---|---|---|
| 0a | Plain-English explainer | ✅ |
| 0b | Pyth feed feasibility measurement | ✅ — 29 listable markets; weekend-metals thesis refuted |
| **1** | **Toolchain, workspace, `solfx-math` with property tests** | **✅** |
| 2 | Vault, markets, Pyth pull oracle | — |
| 3 | Position engine | — |
| 4 | Risk engine and market regimes | — |
| 5 | LP vault | — |
| 6 | IB referral programme | — |
| 7 | Keepers | — |
| 8 | Frontend and SDK | — |
| 9 | Market expansion and hardening | — |

Commercial track (audits, legal, mainnet) is gated on traction or grant funding and is not
started speculatively.

## Layout

```
crates/solfx-math/     Fixed-point financial primitives. Zero dependencies, no floats, no panics.
docs/                  Specification and findings
scratch/feed-probe/    Phase 0b Pyth measurement tooling
```

`programs/` (Anchor) arrives in Phase 2.

## Building

```bash
cargo test                              # unit + property suite
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo llvm-cov --workspace --summary-only   # coverage
```

Build output is redirected to native Linux disk by [`.cargo/config.toml`](.cargo/config.toml).
Source lives on `/mnt/e`, which is a Windows drive mounted through WSL and roughly 200x slower
for the small-file I/O cargo does constantly. See [`docs/WORKFLOW.md`](docs/WORKFLOW.md).

## The two claims worth checking

Two properties are asserted across the full parameter space by
[`crates/solfx-math/tests/properties.rs`](crates/solfx-math/tests/properties.rs), and everything
else in the protocol depends on them holding:

1. **A round trip at an unchanged price always loses.** Open and immediately close, and the
   trader must end with less than they started with. If this ever passes, there is free money in
   the protocol and bots will extract it until the vault is empty.

2. **Rounding always favours the protocol.** Fees and margin requirements round up, payouts round
   down, signed PnL rounds toward −∞. The single most common way a financial program bleeds is a
   division that rounds the wrong way at one size.

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
