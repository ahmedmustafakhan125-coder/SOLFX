# Phase 9 — the brief for a local Claude Code session

Copy everything below the line into Claude Code **on your own machine**. It is written to be
self-contained: it states what the VPS could not do and why, so the local session does not
re-derive it.

**Why this cannot run on the VPS:** ~2 GB free RAM (a `cargo build` OOM-killed itself there),
no admin wallet by design, and two of the exit criteria need a 24-hour run and a 30-day
observation window.

---

## Context you need before starting

Read `CLAUDE.md`, then `docs/CONTEXT.md`, then `CLAUDE-SESSION.md`. They outrank anything
below. In particular: **do not modify `programs/solfx-core/src/` or `programs/solfx-referral/`**,
never run `anchor deploy` / `anchor upgrade` / `solana program deploy`, never run `cargo` while
a validator is running, and consult the **solana-mcp before** any decision touching Solana,
Anchor, SPL or Pyth — not after.

State as of 2026-09-11 20:00 UTC:

- `main` has everything; Phase 8's feature list is complete. **11 commits are unpushed.**
- Devnet is live: program `2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi`, 9 markets listed,
  **6 Active** (EUR/USD, USD/JPY, USD/CNH, XAU/USD, XAG/USD, BTC/USD).
- Tests: **198 SDK**, **15 app**, and a Rust suite the VPS could not run — its true count is
  the first thing to establish.
- The Pyth key lapsed at 15:25 UTC and was replaced; the venue recovered at 19:47 UTC.

**A decision has been made that changes Phase 9's exit criteria: the venue stays at 6 pairs.**
A free Pyth tier plus the poster's throughput will not carry 20 markets inside the 60-second
staleness gate, so `ARCHITECTURE.md` § 15's "≥20 live markets" is being replaced. See
`docs/phase-9-feeds.md` for the measurement: only 24 of the 33 planned symbols have a Hermes
feed at all, and every sponsored devnet feed measured 306 s stale against a 60 s gate.

---

## Task 1 — establish the baseline (do this first, alone)

Nothing else is meaningful until these numbers are real rather than quoted.

```bash
pgrep -a solana-test-validator   # must be empty before any cargo command
anchor build
cargo test --workspace 2>&1 | tail -30
cargo fmt --all --check
cargo clippy --workspace --all-targets
```

**Definition of done:** the actual pass/fail counts are recorded in `docs/CONTEXT.md`,
replacing the "555 Rust tests" figure if it is wrong. `clippy` is clean, or every remaining
warning is listed with a reason. Commit as `docs: the real test and lint baseline`.

Also run `npm --prefix app run test` and `npm --prefix clients/js test`, and fix the
**pre-existing** `format:check` failure in `app/` — 21 files fail `prettier --check` on a
pristine checkout, so `npm --prefix app run ci` is red for reasons that predate Phase 9. One
`prettier --write` pass fixes it.

## Task 2 — the keeper's stale-book guard (small, and it bit us)

`crates/solfx-keeper` ships defaults of `refresh_secs = 30` against `max_book_age_secs = 20`,
so the book is stale by construction for a third of every cycle. Because the crank tick (60 s)
is an exact multiple of the refresh interval, once a tick lands in that window it lands there
every time. Measured on the VPS: **165 "book is stale; standing down this pass" in ten minutes
and zero cranks in eight**, while systemd reported `active` and the health probe reported
healthy. The deployment works around it with flags; the defaults are still wrong.

Add a startup check that refuses to run — or at minimum logs at `ERROR` and corrects itself —
when `refresh_secs >= max_book_age_secs`. Add a unit test for the boundary.

**Definition of done:** `cargo test -p solfx-keeper` covers it, and starting the keeper with
`--refresh-secs 30 --max-book-age-secs 20` produces a refusal rather than a silent stall.

## Task 3 — fuzzing (the big one)

`ARCHITECTURE.md` § 15 says "Trident 24h clean". **Check whether that is still the right tool
before using it** — the Anchor docs now describe first-party coverage-guided fuzzing via
`anchor fuzz`, backed by Crucible, which scaffolds a standalone workspace at
`fuzz/<program_name>/` (`Cargo.toml`, `rust-toolchain.toml`, `idls/<program_name>.json`,
`src/main.rs`) and supports **stateful invariant testing**, crash minimisation and **LCOV
output**. Ask the solana-mcp to confirm what your installed Anchor supports, and run
`anchor fuzz --help`. If it exists, prefer it over Trident: it lives outside `programs/`, which
this repo forbids touching, and its LCOV feeds Task 4.

Whichever tool: the fuzz target must assert the protocol's own invariants after every action,
not merely that nothing panicked. They are **I1–I8**, defined in
`programs/solfx-core/tests/common/mod.rs` at `assert_invariants()` — collateral vault vs the
sum of accounts, LP vault vs AUM, open interest vs positions, no size without collateral,
insurance vault, total vault balances, LP supply vs AUM. Reuse that function if the harness can
link it; port it if not.

**Definition of done:** a 24-hour run completes with zero crashes, the command and duration are
recorded in `docs/CONTEXT.md`, and crash artifacts (if any) are committed as regression tests.
Start the run early — it is the long pole.

## Task 4 — coverage

Anchor exposes `anchor coverage [--skip-run] [--skip-build] [--output target/coverage/sbf.lcov]`,
which generates LCOV from SBF traces. `cargo-llvm-cov` is the alternative for the pure-Rust
crates (`solfx-math`, `solfx-keeper`), which is where the money arithmetic lives.

The § 15 criterion is **>90%**. Do not chase the number on glue code: report coverage
**per crate**, and treat `crates/solfx-math` and `programs/solfx-core/src/instructions/` as the
two that must clear 90%. Anything below that in those two is a gap worth a test.

**Definition of done:** a coverage report committed under `docs/`, per-crate figures in
`docs/CONTEXT.md`, and new tests for any money path under 90%.

## Task 5 — the two missing scenario replays

Seven already exist in `programs/solfx-core/tests/scenarios.rs`: CHF depeg (solvent, and the
no-insurance-fund variant), GBP flash crash, the weekend stale-price exploit, a weekend gap
through `GapWindow`, an EM market outside local hours, and a local holiday. § 12.4 lists two
more that do not exist yet:

- **March 2020 COVID** — sustained volatility, confidence-interval blowout, sustained wide
  spreads. The point is that the confidence gate and the spread widen *and keep working*, not
  that a single tick is rejected.
- **EM devaluation** — IDR 1998 or INR 2013. The point is whether EM leverage survives a
  managed-float break.

These live under `programs/solfx-core/tests/`. **`CLAUDE.md` says work goes in `crates/`,
`scripts/` or `docs/`** — adding tests under `programs/` is a deliberate exception, so confirm
it before writing, and do not touch anything under `src/`.

**Definition of done:** both replays pass, and each asserts `assert_invariants()` rather than
only a final balance.

## Task 6 — the § 12.5 extensibility test, rescoped to 6 pairs

The criterion's *point* is that new markets need no redeploy and the engine is asset-class
generic. That does not need 20 markets. On the running devnet deployment, **carrying at least
one live position throughout**:

1. `initialize_market` for a market that did not exist at deploy time — **ETH/USD** and
   **SOL/USD** are the right choices: Hermes serves both, they are `FeedKind::Crypto`, and
   being 24/7 they are testable on a weekend when every FX pair is legitimately shut.
2. Confirm the program binary hash is **unchanged** (`solana program dump` and compare, or
   `anchor verify`).
3. `set_market_status` to activate, add the feed to `deployment.json` so the poster publishes
   it, and trade it end to end from the browser.
4. Confirm every pre-existing position is untouched — same collateral, same entry, same
   accrued funding.
5. Variants: the 6 live pairs already cover **direct USD-quoted** (EUR/USD, XAU/USD, BTC/USD)
   and **non-USD-quoted** (USD/JPY, USD/CNH). A **synthetic cross** is not yet exercised —
   `PriceSource::Synthetic { invert_quote }` exists and `docs/CONTEXT.md` names XAU/EUR as the
   natural candidate. A **continuous metals** market is permanently unreachable:
   `initialize_market` rejects `FeedKind::ContinuousIndex` and Phase 0b found no 24/7 metals
   feed. Record that as closed-unreachable rather than pending.

**This needs the admin wallet `7ktphn…BdWs`, which is deliberately not on the VPS.**

**Definition of done:** each step's evidence (signatures, hashes, before/after position state)
recorded in a new `docs/phase-9-report.md`.

## Task 7 — amend the roadmap to what is actually being built

`ARCHITECTURE.md` § 15's Phase 9 row says "≥20 live markets". That is now wrong, and Phase 8's
row says "including a weekend gold trade", which is **permanently unattainable** — Q1b found no
24/7 metals feed in the public Hermes catalogue, so no weekend gold trade can ever happen.

Rewrite both exit criteria to what is being delivered, and say in the document *why* they
changed, with the measurement behind it. A roadmap nobody can satisfy stops being a roadmap.

## Task 8 — the two admin transactions that finish Phase 8's IB work

`solfx-referral` is deployed on devnet and holds **zero accounts**. The SDK client and the
`/partners` page are written and tested; registration and claiming are unwired because there is
nothing to register against.

1. `initialize_referral(override_bps = 2000)` — 20%, per § 8.5's two-level structure.
2. `set_referral_authority(<the config PDA>)` on `solfx-core`, signed by the **core admin** —
   two deliberate steps by two different authorities.

Then wire registration and claiming into `app/src/pages/Partners.tsx`; the builders already
exist in `clients/js/src/referral/instructions.ts` with tests.

Note `fee_split_referral_bps` is **500** on devnet, not the 1000 § 8.3 assumes, so every tier
from Bronze up is currently capped at the pool. `entitlement()` handles it; decide whether 500
is the intended number.

## Task 9 — replace the hand-written referral client with generated code

`clients/js/src/referral/` is hand-written because `codama.json` names only the core IDL, and
**neither program has an IDL account at the classic Anchor address on devnet** — verify that
with `anchor idl fetch` and publish it if missing, because `docs/CONTEXT.md` claims the IDL is
current and the VPS could not find one.

Add `solfx_referral` to `codama.json`, run `npm run generate`, and **delete
`clients/js/src/referral/` in the same commit**, moving its discriminator re-derivation test to
cover the generated output.

---

## Order of work

Task 1 first. Then start **Task 3's 24-hour run** as early as possible and do Tasks 2, 4, 5
while it runs. Tasks 6 and 8 need the admin wallet and a live venue, so batch them. Task 7 is
half an hour and should be done once 6 is known-good. Task 9 is independent.

The last criterion — **30 days with zero fund-loss bugs** — is a calendar, and its clock cannot
start until the rest is in place. Phase 9 is a four-week phase in `ARCHITECTURE.md`'s own
estimate; treat anyone who says it finished in a day as mistaken.
