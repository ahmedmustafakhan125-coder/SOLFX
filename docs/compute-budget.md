# Compute Budget

**Measured:** Phase 3, against `solfx_core.so` built with `anchor build` (release profile,
`lto = "fat"`, `codegen-units = 1`).
**Source of truth:** [`programs/solfx-core/tests/compute_budget.rs`](../programs/solfx-core/tests/compute_budget.rs).
Every figure below is asserted against a ceiling in CI, so a regression fails the build
rather than being discovered in production.

Regenerate with:

```bash
anchor build
cargo test -p solfx-core --test compute_budget -- --nocapture
```

---

## Why this file exists

`ARCHITECTURE.md` § 5.5 sets the budgets the protocol has to hit — `open_position` around
120k CU, liquidation under 200k — and § 14.3 makes a CU regression check a CI gate.

The reason is narrow and specific: **CU regressions do not fail loudly.** An instruction that
grows from 180k to 210k works perfectly in development and then stops landing during the
congestion spike that made it urgent. For a liquidator that is not a performance problem, it
is bad debt (§ 6.8: *an unprofitable liquidation is an unliquidated position, and unliquidated
positions are how vaults die*).

---

## Phase 2 measurements

| Instruction | CU | Ceiling | Notes |
|---|---:|---:|---|
| `initialize_protocol` | 64,800 | 200,000 | 13 accounts; creates 4 token accounts, a mint and 3 PDAs. One-time. |
| `initialize_market` (direct) | 14,645 | 60,000 | The cost of listing a market. Not a hot path, but it is the § 5.6 claim's price tag. |
| `initialize_market` (synthetic) | 14,698 | 60,000 | +53 CU over direct — the second feed id is stored, not read. |
| `initialize_market` (converted) | 14,648 | 60,000 | Non-USD-quoted. Effectively identical. |
| `set_market_status` | 9,014 | 30,000 | |
| `deposit_collateral` | 16,904 | 60,000 | Includes the SPL Token CPI. |
| `withdraw_collateral` | 16,938 | 60,000 | Includes the CPI plus PDA-signed transfer. |
| **`crank_market_price` (1 feed)** | **11,766** | 60,000 | **The full § 7.1 validation stack.** See below. |
| `crank_market_price` (2 feeds) | 15,394 | 80,000 | Synthetic composition. |
| `crank_market_price` (converted) | 13,438 | 80,000 | Primary + quote-conversion feed. |
| `emergency_pause` | 4,972 | 30,000 | |
| `unpause` | 4,971 | 30,000 | |
| `halt_market` | 9,016 | 30,000 | |

## Phase 3 measurements — the position lifecycle

These are the budgets § 5.5 actually sets. Each carries the full stack: a validated oracle
read, execution pricing, margin checks, the four-vault settlement, and up to three SPL Token
CPIs.

| Instruction | CU | § 5.5 budget | Ceiling | Headroom |
|---|---:|---:|---:|---:|
| **`open_position`** (direct) | **55,584** | ~120,000 | 120,000 | **54%** |
| `open_position` (synthetic) | 59,211 | — | 140,000 | 58% |
| `open_position` (converted) | 57,729 | — | 140,000 | 59% |
| `increase_position` | 52,437 | — | 120,000 | 56% |
| `decrease_position` | 53,296 | — | 120,000 | 56% |
| `close_position` | 51,724 | — | 120,000 | 57% |
| `add_position_collateral` | 40,779 | — | 60,000 | 32% |
| `remove_position_collateral` | 45,248 | — | 80,000 | 43% |
| `add_liquidity` | 20,612 | — | 60,000 | 66% |

Program binary: **683 KB**.

### What the numbers say about Phase 4

`open_position` lands at **55,584 CU against a 120,000 budget** — less than half. That matters
because Phase 4's liquidation is the instruction with a hard ceiling: § 6.8 requires it under
200k CU, because *an unprofitable liquidation is an unliquidated position, and unliquidated
positions are how vaults die.*

Liquidation does strictly more than `close_position` (51,724 CU): the same oracle read, the
same settlement, plus a health-factor computation, a penalty split three ways and an extra
transfer. On these numbers that is comfortably inside 200k — the CU budget is not what will
make Phase 4 hard.

### Where the cost actually is

| Component | ~CU |
|---|---:|
| Validated oracle read (1 feed) | 11,800 |
| Each additional feed | 3,600 |
| Each SPL Token CPI | 5,000–6,000 |
| Margin, PnL and fee arithmetic | ~8,000 |
| Anchor account deserialisation (15 boxed accounts) | ~15,000 |

The settlement's netting of the LP fee share against PnL (see `trader::flows`) saves one CPI
per trade — roughly 5k CU, or 10% of a close.

---

## The number that matters: the oracle path costs ~12k CU

`crank_market_price` with one feed runs the entire validated read — ownership, feed identity,
verification level, two-sided freshness, exponent normalisation, confidence ceiling and EMA
deviation — plus an account write and two events, for **11,766 CU**.

The marginal cost of each extra feed is about **3,600 CU**:

| Feeds read | CU | Δ |
|---:|---:|---:|
| 1 | 11,766 | — |
| 2 | 15,394 | +3,628 |

That has a direct consequence for Phase 3. § 5.5 budgets `open_position` at ~120k CU and
worries that a synthetic, non-USD-quoted market needing three price updates in one
transaction might not fit. On the program side it does, comfortably: three validated reads
cost roughly 19k CU, leaving over 100k of the § 5.5 budget for margin arithmetic, fee
splitting and account writes.

**The transaction-size constraint is a separate question and is not settled by this table.**
Each `post_update` is its own instruction (~45k CU by § 5.5's estimate) and carries a signed
Wormhole payload; the limit that binds first is the 1232-byte transaction size, not compute.

**Still unmeasured after Phase 3.** The LiteSVM suite writes `PriceUpdateV2` accounts
directly, so it never builds a real `post_update` and never exercises the size limit. Phase 3
confirms only that the *compute* fits. Measuring the size needs a real Hermes payload against
devnet, which is Phase 7's keeper work — and if it does not fit, the fallback is already in
the design: post the updates in a preceding transaction and read the cached accounts, which
is what `max_staleness_seconds` exists to make safe.

---

## Ceilings are tripwires, not targets

Each ceiling sits well above its measurement. They are set to catch a **step change** — a new
account in the struct, an unbounded loop, an accidental clone of a large struct — not to
squeeze the last hundred units out of a handler.

If a change legitimately pushes an instruction past its ceiling, raise the ceiling **and**
update the table above in the same commit. A ceiling silently raised to make CI pass is worse
than no ceiling, because it looks like a check.

---

## The stack-frame constraint

Separate from compute, and it bites earlier than you would expect.

Solana gives each BPF stack frame **4 KB**, and Anchor materialises the whole `Accounts`
struct on it. `InitializeProtocol` carries 13 accounts including a `Protocol`, two `Mint`s,
four `TokenAccount`s, an `InsuranceFund` and an `LpPool`. On the first run it failed at
runtime with:

```
Access violation in stack frame 5 at address 0x200005e28 of size 8
```

which names nothing useful and is easy to mistake for a logic bug.

**The fix is `Box<Account<'info, T>>` on every account field**, which moves the data to the
heap and leaves a pointer on the stack. Every account struct in `solfx-core` is boxed.
`compute_budget.rs` also asserts the account count of the widest instruction, so a future
addition fails with a message that says what happened instead of an access violation.

---

## History

| Phase | Change |
|---|---|
| 2 | Baselines established. Oracle read path measured at 11,766 CU. |
| 3 | Position lifecycle added. `open_position` at 55,584 CU, 54% under the § 5.5 budget. Binary 472 KB → 683 KB. |
