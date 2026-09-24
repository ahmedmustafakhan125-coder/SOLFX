# NOXFUNDS — compute, packet size and account locks

**Measured:** Stage 6, against `noxfunds.so` and `solfx_core.so` in LiteSVM, EUR/USD, one
funded mandate.
**Source of truth:** [`programs/noxfunds/tests/budgets.rs`](../programs/noxfunds/tests/budgets.rs).
Every figure is asserted against a ceiling in CI, so a regression fails the build rather than
being discovered on a cluster.

Regenerate with:

```bash
anchor build
cargo test -p noxfunds --test budgets -- --nocapture
```

---

## The three ceilings

| | Limit | Source |
|---|---:|---|
| Compute per instruction | 200,000 default, 1,400,000 max | Solana runtime |
| Transaction packet | 1,232 bytes (legacy/v0) | Solana runtime; v1 raises it to 4,096 (SIMD-0296/0385, live on devnet since epoch 1140) |
| Account locks | 64 | `increase_tx_account_lock_limit` (128) is **not** an activated feature |

---

## Measurements

| Instruction | Accounts | Bytes | CU | Ceiling |
|---|---:|---:|---:|---:|
| `funded_open_position` | 21 | 863 | **109,236 – 116,736** | 150,000 |
| `funded_close_position` | 20 | 804 | 82,769 | 100,000 |
| `observe_mandate_equity` (1 position) | 9 | 494 | 23,389 | 30,000 |
| `observe_mandate_equity` (5 positions) | 21 | 634 | 50,704 | 65,000 |
| `observe_mandate_equity` (`MAX_SLOTS` = 8) | 30 | 739 | — | packet only |
| `claim_settlement` | 15 | 692 | 51,136 | 60,000 |
| **Limits** | **64** | **1,232** | **200,000** | |

Bytes are measured the way a client actually builds the transaction — with
`set_compute_unit_limit` and `set_compute_unit_price` in front, because those are not optional
in production and they cost bytes too. The same helper `solfx-core`'s packet test uses
(`Env::measure_keeper_tx`), so the two suites' figures are directly comparable.

### Why `funded_open_position` is given as a range

Measured six times: 109,236 / 110,736 / 112,236 (×3) / 113,736 / 116,736 CU. The steps are
exactly 1,500 apart, which is the cost of one `create_program_address`. `solfx-core` creates
the `Position` and the `TriggerOrder` with `init` and an unstored `bump`, so Anchor runs
`find_program_address`, counting down from 255 until it finds an off-curve address. The tests
use fresh random keypairs, so the canonical bumps — and the number of misses — differ each run.

Two consequences. **A given trade is deterministic**: the seeds are fixed, so one trader's
position always costs the same figure every time. And **the ceiling has to cover the search**,
which is why it sits at 150,000 rather than snugly above one lucky measurement — roughly 22
spare bump iterations.

It is also a live demonstration of why the project's Solana rules §3 says to store bumps. Every
seed constraint NOXFUNDS owns uses `bump = account.bump` and costs a flat ~1,500 once.

### `funded_open_position` is the expensive one, by construction

At the top of its range, 116,736 CU is **58% of the default 200,000 budget** — it lands without a compute-budget raise,
but not with room to spare. That is the price of an invariant rather than an inefficiency: the
instruction validates the mandate's rules against a price it reads itself, then makes two CPIs
into `solfx-core` — `open_position` and `place_trigger_order` — each of which reads the oracle
again. Three oracle loads and two cross-program invocations buy the guarantee that **a funded
position can never exist without its stop-loss**. A client should still set an explicit limit;
the default is a ceiling, not a reservation.

For comparison, bare `solfx-core::open_position` is 59,211 CU. The NOXFUNDS wrapper roughly
doubles it, and the second CPI is most of the difference.

### The account count is 21, not the 22 the plan derived

The Stage 0 derivation listed a `trader_profile` account. It now exists — but it is not on the
open path, because nothing about *opening* a position changes a trader's record. The profile is
written by the close (realised PnL, hold time), by the equity crank (observed drawdown) and by
settlement (the mandate's outcome). Keeping it off the open leaves the most expensive
instruction at 21 accounts instead of 22.

### The crank, not the trade, is what caps concurrency

`observe_mandate_equity` carries one `(position, market, price)` triple per open position in
`remaining_accounts`, so its cost scales with how many positions a mandate holds. At
`MAX_SLOTS = 8` that is 29 accounts and 706 bytes — 57% of the packet, 45% of the lock limit.
There is room for `MAX_SLOTS` to roughly double before the packet becomes the constraint.

The per-position wire cost is far below 3 × 32 bytes because the market and the price accounts
repeat; only the position address is new each time, so each extra position costs one 32-byte
address and three one-byte indices.

A crank that does not fit is a mandate whose equity cannot be checked, which is a mandate that
can be neither breached nor wound down — which is why this is measured at the maximum rather
than at one.

---

## Throughput

Every funded trade write-locks the same SolFX accounts: `lp_pool`, `lp_vault`,
`collateral_vault`, `insurance_vault`, `fee_vault`, `protocol` and the `Market`. Solana caps a
single **writable account** at 12,000,000 CU per block, and that — not the block limit — is
what bounds a venue whose every trade touches one LP pool.

```
12,000,000 CU per writable account per block
   ÷ ~112,000 CU per funded open
   = ~107 funded opens per block
   × 2.5 blocks/second (400 ms slots)
   ≈ 265 funded opens/second
```

Asserted as a floor in `the_write_lock_cap_supports_a_useful_trade_rate`, so a CU regression
surfaces as a throughput claim that stopped being true rather than as a stale number in a
document.

Three things this figure is not:

- **It is not the SolFX ceiling.** Bare `open_position` at 59,211 CU reaches ≈497/second on
  the same arithmetic. NOXFUNDS trades are roughly half as dense because they do twice the
  work.
- **It is not today's operational limit.** The gateway is configured at `SOLFX_GATEWAY_RPS=9`.
  The structural ceiling is two orders of magnitude above what the deployment currently
  permits, which is the right way round.
- **It does not improve with SIMD-0286.** The 100M block limit is irrelevant here; the hot
  account tops out at 12% of it. SIMD-0306 would raise the per-account cap to 24M and is not
  activated.

---

## What is not measured here

- **Stack frame.** Stage 0 measured the two-CPI path against the 4,096-byte BPF frame and
  pre-committed the `#[inline(never)]` split that keeps the two `CpiContext`s from sharing a
  frame. It is not re-measured per instruction.
- **The widest market.** These figures are EUR/USD — one oracle leg. A synthetic, non-USD-quoted
  market adds two more `remaining_accounts`, roughly 66 bytes and one extra
  `load_validated_price` per CPI. `solfx-core`'s packet test covers the widest shape directly.
- **Heap.** Anchor's bump allocator gives 32 KiB; the derived peak was ~4.5 KB.
