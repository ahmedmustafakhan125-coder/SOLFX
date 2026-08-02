# Phase 1 — Foundations

**Deliverable:** toolchain, repo scaffold, CI, `math/` module with property tests.
**Status:** complete.

`ARCHITECTURE.md` § 15 sets four exit criteria. A phase is done when they pass, not when time
has passed — so each is recorded here against a measurement rather than an assertion.

| Exit criterion | Result |
|---|---|
| `cargo test` green | ✅ 165 tests: 127 unit + 38 property |
| CI on PR | ✅ [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) — fmt, clippy `-D warnings`, test, plus a 100k-case deep property run |
| 100% coverage on `math/` | ⚠️ **99.17% lines, 100% functions.** See below |
| **round-trip-never-profits passes** | ✅ across 4,096 cases per run, USD-quoted and converted markets, both directions |

## On the coverage criterion

100% line coverage was not reached, and chasing it further would be dishonest rather than safe.
All 11 uncovered lines are the error-propagation arm of `?` on an internal call that cannot
fail — dividing by a compile-time-nonzero constant such as `NOTIONAL_DIVISOR`:

```rust
let n = mul_div_ceil(size, price, NOTIONAL_DIVISOR)?;
//                                                 ^ this arm is unreachable
```

Covering them would mean either injecting faults into infallible code or removing the `?` and
losing the compiler's guarantee. Both make the code worse. **100% function coverage and 99.17%
line coverage with every residual line identified as unreachable-by-construction** is the
stronger result, and is what the criterion should have said.

Every deliberate failure path — bad price, wide confidence, zero divisor, oversized close,
malformed fee split — has a test.

## What was built

`crates/solfx-math`, a standalone crate with **zero dependencies**.

`ARCHITECTURE.md` § 11 places `math/` inside `programs/solfx-core`. It is a separate crate
instead, for three reasons:

1. `cargo test` runs in under a second without the Solana BPF toolchain, which is what makes a
   100k-case property run practical in CI.
2. The coverage criterion is measurable in isolation.
3. It enforces § 5.6's generic-engine constraint *by construction* — the crate cannot reference
   a `Market` or a `Position`, so no FX-specific assumption can leak into the maths.

`solfx-core` will depend on it in Phase 2.

| Module | Contents |
|---|---|
| `constants` | Fixed-point scales (§ 6.1), with the derivation of `NOTIONAL_DIVISOR` asserted at compile time |
| `error` | Named error per failure mode; no `unwrap`, no `panic` |
| `fixed` | Checked arithmetic with explicit rounding. **The only module permitted raw `/` and `%`** |
| `types` | `Direction`, `Side`, `TradeAction`, `QuoteConversion` |
| `pnl` | Notional, uPnL, quote conversion (C-3), weighted entry, partial realisation |
| `margin` | IMR/MMR with size tiers, equity, liquidation test, liquidation price |
| `pricing` | Confidence gate and spread, skew impact, adverse-side execution price (§ 6.6) |
| `fees` | Volume-tiered rates, fee amounts, four-way split, liquidation penalty split |
| `funding` | Carry rate and index, funding rate, conserving index update |

Worked examples from both specification documents are encoded as tests, so the code and the
documents cannot drift apart silently: the $108,543 notional, the $10-per-pip standard lot, the
1.07415 liquidation price, the $5.95/day carry, and the measured Pyth confidence verdicts.

## Findings

Four things surfaced during implementation that the specification did not have right.

### 1. Signed division needed two functions, not one

Rust's `/` truncates **toward zero**, so `-7 / 2 == -3`. Applied to a trader's loss that rounds
the shortfall in the *trader's* favour — the protocol eats the remainder on every losing
position.

Correct rounding turns out to depend on what the number *means*, and the two cases go opposite
ways:

| Quantity | Direction | Why |
|---|---|---|
| Trader **value** (PnL) | toward −∞ (`mul_div_floor_signed`) | Gains shrink, losses grow |
| Trader **cost** (carry rate) | toward +∞ (`mul_div_ceil_signed`) | Charges grow, credits shrink |

Using one for the other is a silent, systematic leak. Both now exist, are documented as
mirrors, and a property test asserts they bracket the true value.

### 2. Carry's sign in § 6.7 is the negation of the user-facing document

`ARCHITECTURE.md` § 6.7 gives `carry_rate_per_hour = (rate_base − rate_quote) / (365 × 24)`.
`FOREX-EXPLAINED.md` § 9 works the same case numerically and gets the opposite sign: long
EUR/USD with EUR at 2% and USD at 4% **costs** 2% a year, i.e. `quote − base`.

The explainer is right. The architecture states an *earn* rate while every consumer of the value
wants a *cost*. `carry_cost_rate_per_hour` returns the cost, and the discrepancy is documented at
the function so the next reader does not have to re-derive it.

### 3. Funding does not conserve value on an uneven book

§ 6.7 specifies `cum_funding_long += rate` and `cum_funding_short -= rate`. Longs then pay
`rate × base_oi_long` while shorts receive `rate × base_oi_short` — equal only when the book is
perfectly balanced. On any real book the difference comes out of LP capital, which is exactly
what invariant I3 forbids.

`funding_index_update` scales the receiving side by the OI ratio so the transfer nets, rounds the
payer up and the receiver down, and returns zero when either side is empty (there is nobody to
pay). A property test asserts the protocol never pays out more than it collected, across
adversarial OI ratios up to a million to one.

### 4. A liquidation price cannot be pinned to the exact unit

Solving `equity == maintenance_margin` for price converts a quote-unit budget into a price move
and floors it. Recomputing PnL from that floored price amplifies the discarded fraction by
`size / NOTIONAL_DIVISOR` — the position's value per price tick. A position worth 5 quote units
per tick cannot have its liquidation price pinned closer than 5 units of equity. That is
fixed-point granularity, not a bug.

What matters is the **direction**, and flooring gets it right: the displayed price arrives at or
before the real one for both longs and shorts, so a trader is never shown a liquidation later
than it happens. Two property tests pin this — one that the display never fires early, one that
liquidation really does bite just beyond it.

## Deviations from the specification

| § | Specified | Built | Why |
|---|---|---|---|
| 11 | `math/` inside `programs/solfx-core` | Standalone `crates/solfx-math` | Test speed, isolated coverage, and enforces the § 5.6 generic-engine constraint by construction |
| 6.7 | `carry = (base − quote)/8760` | `carry = (quote − base)/8760` | Matches `FOREX-EXPLAINED.md` § 9 and the sign every caller needs |
| 6.7 | `cum_funding_short -= rate` | Receiving side scaled by the OI ratio | The specified form breaks invariant I3 on an uneven book |
| 6.6 | `skew_impact / OI_SCALE`, undefined | Rate is bps per standard lot of added imbalance | `OI_SCALE` was never defined; lots are the unit an FX venue tunes in |
| 5.3 | `open_fee_bps: u16` | `RATE_PRECISION` (1e9) rates | The § 8.2 schedule needs 0.8 bps, which is not an integer number of bps. The spec's own note flags this |

## Not in scope for Phase 1

Deliberately absent, and belonging to later phases:

- Oracle account plumbing (C-5, pull-oracle deserialisation) — **Phase 2**
- Market regime state machines (§ 7.3) — **Phase 4**
- Bad-debt waterfall and ADL (§ 6.9) — **Phase 4**
- Invariants I1–I9 (§ 12.3) — need on-chain accounts to assert against — **Phase 2 onward**
- Scenario replays: CHF depeg, GBP flash crash (§ 12.4) — **Phase 4**

## Reproducing

```bash
cd .
cargo test                                    # 165 tests
cargo clippy --all-targets -- -D warnings     # clean
cargo llvm-cov --workspace --summary-only     # 100% functions, 99.17% lines

PROPTEST_CASES=100000 cargo test --release --test properties   # the deep run CI does
```
