# Compute Budget

**Measured:** Phase 2, against `solfx_core.so` built with `anchor build` (release profile,
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

Program binary: **472 KB**.

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
Phase 3 measures that against a real Hermes payload.

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
