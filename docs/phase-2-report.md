# Phase 2 — Vault, Markets, Oracle

**Status:** complete.
**Exit criteria (`ARCHITECTURE.md` § 15):** *"Direct, synthetic, EM and metals-index feeds all
price correctly; I1 holds."*

| Criterion | Result |
|---|---|
| Direct feeds price correctly | ✅ EUR/USD, XAU/USD, USD/JPY, BTC/USD |
| Synthetic feeds price correctly | ✅ EUR/GBP composed from two legs |
| EM / non-USD-quoted feeds price correctly | ✅ USD/INR with its conversion feed |
| Metals-**index** feeds price correctly | ⛔️ **no such feed exists** — see below |
| Invariant I1 holds | ✅ asserted three ways after every instruction |
| Generic-engine constraints (§ 5.6) | ✅ 12 markets, 5 shapes, one unchanged binary |

---

## The one criterion that could not be met, and why that is correct

"Metals-index feeds price correctly" cannot be satisfied, because **the feed does not exist.**
Phase 0b measured all 137 Pyth `Metal` and `Commodities` feeds frozen at the Friday close, and
no metals index is catalogued under any asset type (`oracle-feasibility.md`).

Rather than leave the criterion quietly unmet, Phase 2 encodes the finding:

- `FeedKind::ContinuousIndex` is **retained** in the enum. It costs nothing to reserve, and if
  **Q1b** resolves favourably a market flips regime by changing one config field.
- `initialize_market` **rejects it**, with a named error. A market cannot be listed against a
  24/7 promise its feed does not keep.
- A test asserts the rejection, so the day someone tries to enable it they get an error that
  says why rather than a market that silently freezes every weekend.

The `WeekendMode` and `PreOpenWindow` states, and the four `weekend_*` fields on `Market`,
are all still there and all still dormant. `set_market_status` refuses to enter either state
by hand, because their transitions belong to a Phase 4 regime machine that has nothing to
govern yet.

---

## What was built

```
programs/solfx-core/
├── constants.rs            PDA seeds, protocol-wide ceilings
├── errors.rs               47 named errors + total MathError mapping
├── events.rs               15 events — the indexer's only data source
├── oracle.rs               THE choke point: 6 gates, one path
├── state/
│   ├── protocol.rs         authorities, fee split, I1 running total
│   ├── market.rs           the append-only market row
│   ├── user_account.rs     collateral + immutable referrer binding
│   └── lp_pool.rs          LpPool + InsuranceFund (Phase 5 logic)
└── instructions/
    ├── admin/              initialize_protocol, initialize_market, retuning, handover
    ├── guardian.rs         pause / halt — restrict only, never release
    ├── user.rs             account creation, deposit, withdraw
    └── keeper.rs           crank_market_price
```

Plus `crates/solfx-math/src/oracle.rs` — the pure arithmetic half of § 7.1.

**16 instructions. 9,590 lines: 5,950 source, 3,640 test.**

### Tests

| Suite | Count |
|---|---:|
| `solfx-math` unit | 161 |
| `solfx-math` property | 48 |
| `solfx-core` integration | 86 |
| **Total** | **295** |

Compute-unit baselines are in [`compute-budget.md`](compute-budget.md). Headline:
**the whole validated oracle read costs 11,766 CU**, leaving over 100k of § 5.5's
`open_position` budget for Phase 3.

---

## Five places the specification was wrong, or had to be extended

These are worth attention — all five are decisions where the implementation deviates from
`ARCHITECTURE.md`, deliberately.

### 1. `get_price_no_older_than` leaves a hole. Freshness is now checked at both ends.

§ 7.1 calls the SDK helper, which asks only `publish_time + max_age >= now`. **A price stamped
an hour into the future satisfies that trivially — and keeps satisfying it for the next hour,
regardless of what the market does.** That is the C-1 stale-price exploit reached through a
different door, and it would have been in the codebase from day one.

`validate_publish_time` closes both ends, with a small drift allowance for ordinary
publisher-clock disagreement. `MAX_FUTURE_DRIFT_SECONDS = 5`.

The implementation also calls `get_price_unchecked` and performs all six gates itself, rather
than calling the bundled helper. The helper reaches its staleness check through
`maximum_age.try_into().unwrap()` — a panic path in a dependency. Our inputs can never trigger
it, but the choke point should not depend on that being true.

### 2. The deviation reference must not be self-maintained, or the market wedges shut.

§ 7.1 compares spot against `market.ema_price`, maintained on chain. That creates a deadlock:
refreshing the reference requires reading a price, reading a price requires passing the
deviation gate, and after a large genuine move **nothing passes** — the reference stays stale,
every later price deviates from it, and the market is frozen exactly when it most needs to
reprice.

`PriceFeedMessage` already carries `ema_price` at the same exponent, computed by Pyth. Using
it removes the deadlock, removes a cranking requirement, and removes the rounding drift a
self-maintained integer EMA accumulates. `Market.ema_price` is still stored — as an
observation for the frontend, not as the gate's input.

The gate is also **split from the measurement**. `load_validated_price` rejects on deviation
(trading paths); `observe_market_price` measures and returns it (the crank). § 7.2 says a
deviating market goes to `Halted` with an alert — a different response from rejecting one
trade, and one a cranker forced through the gate could never reach. There is a test for this:
`a_halted_market_can_still_be_repriced`.

### 3. Synthetic confidence compounds **linearly**, not in quadrature.

§ 3.5 gives `√(conf₁² + conf₂²)`. That is the variance-addition result for **independent**
errors. Every cross we would ever compose shares a currency — that is what makes it
composable — so the errors are correlated and independence does not hold.

The implementation uses `conf₁ + conf₂`: the triangle-inequality upper bound, valid under any
correlation including the adversarial case. Quadrature understates the band by up to 29% at
equal leg confidences, and the band is the only thing standing between the vault and latency
arbitrage (C-4).

The cost of being conservative is close to zero — Phase 0b found Pyth's native crosses
*tighter* than the majors they would be composed from, so v1 lists direct feeds only and this
path is reserved for pairs Pyth does not carry.

### 4. `quote_conversion_feed` alone is not enough. The direction has to be stored too.

C-3 specifies a conversion feed on `Market`. A feed id does not say whether to multiply or
divide: USD/JPY quotes *quote units per USD*, EUR/USD quotes *USD per quote unit*, and using
one convention where the other applies mis-prices by the square of the rate.

`Market` therefore carries `quote_conversion_kind` alongside the feed id, and
`initialize_market` requires the two to agree — a declared conversion must have a feed, and a
USD-quoted market must not carry one.

### 5. Anchor 1.x needs every account boxed, and fails unhelpfully when it is not.

`initialize_protocol` carries 13 accounts. Anchor materialises the whole `Accounts` struct on
a 4 KB BPF stack frame, and the first run failed at runtime with:

```
Access violation in stack frame 5 at address 0x200005e28 of size 8
```

which names nothing and looks like a logic bug. The fix is `Box<Account<'info, T>>` on every
account field. **Every account struct in the program is boxed**, and
`compute_budget.rs` asserts the widest instruction's account count so a future addition fails
with a message that says what happened.

Two smaller Anchor 1.x differences, noted because the migration guides do not: `CpiContext::new`
takes a `Pubkey` rather than an `AccountInfo`, and `Price` carries no `ema_price` — the EMA
lives on the underlying `PriceFeedMessage`.

---

## Toolchain, pinned and verified working together

| Component | Version |
|---|---|
| `anchor-lang` / `anchor-cli` | 1.1.2 |
| `anchor-spl` | 1.1.2 |
| `pyth-solana-receiver-sdk` | 2.0.0 |
| `solana-program` (transitive) | 4.1.0 |
| Agave CLI | 3.1.10 |
| Rust | 1.96.1 |
| LiteSVM (tests) | 0.15.2 |

Two dependency traps worth recording:

- **`solana-sdk` cannot be a dev-dependency.** The umbrella crate at 4.1.0 requires
  `solana-short-vec ^3.3.0`; litesvm 0.15.2 pins `~3.2.2`. Depend on the individual crates
  instead — `Pubkey` and `Instruction` still come from `anchor_lang::solana_program`, so only
  one of each type crosses the test boundary.
- **`spl-token` needs `features = ["no-entrypoint"]`.** It exports a symbol named `entrypoint`
  and so does the program; linking a test binary against both fails with
  `duplicate symbol: entrypoint`.

---

## Build layout change

Phase 1 redirected `target-dir` off the slow `/mnt/e` 9p mount. Phase 2 replaced that with a
**symlink**:

```
./target -> $HOME/.cargo-target/solfx
```

`anchor build` looks for `target/deploy/*.so`, `target/deploy/*-keypair.json` and
`target/idl/*.json` at paths relative to the workspace root. A `target-dir` redirect puts them
somewhere Anchor does not look; the symlink keeps them where Anchor expects while the bytes
still land on fast disk.

After a fresh clone:

```bash
mkdir -p ~/.cargo-target/solfx && ln -s ~/.cargo-target/solfx target
```

**Program ID:** `2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi`. The keypair lives at
`target/deploy/solfx_core-keypair.json` and is **not** committed. A fresh clone that needs its
own deployment runs `anchor keys sync` to rewrite `declare_id!`.

---

## What Phase 2 deliberately did not build

| Deferred | To |
|---|---|
| Positions, margin checks, execution pricing | Phase 3 |
| Funding, carry, liquidation, ADL, regime state machines | Phase 4 |
| LP add/remove liquidity (accounts exist, logic does not) | Phase 5 |
| Referral accrual and claim (binding exists, accrual does not) | Phase 6 |

`LpPool`, `InsuranceFund` and their vaults **are** created by `initialize_protocol`, because
that instruction runs once and deferring them would need a second initialisation path. Their
addresses and the LP mint authority are fixed from the first transaction and never move.

---

## Open questions carried forward

| # | Question | Status |
|---|---|---|
| **Q1b** | How is Pyth Indices for metals actually distributed? Confirmed to exist, absent from the public Hermes catalogue. The only route by which weekend metals survives. | 🔴 **Open — commercial, not technical.** One email to Pyth. |
| Q2 | Exact session boundaries per market, especially each EM pair's local calendar. | Needed for Phase 4's regime machine, not for Phase 3. |
| Q4 | Friday-close / Sunday-open gap size — sizes `GapWindow` and the MMR buffer. | Needed for Phase 4. |
| — | Transaction *size* with three real Hermes `post_update` payloads. Compute is settled (§ compute-budget); the 1232-byte limit is not. | Measure in Phase 3. |

---

## Next: Phase 3 — Position engine

Open, increase, decrease, close. Margin, PnL, quote conversion, execution pricing.

Exit criteria: *"Lifecycle tested on a direct pair, a synthetic cross and a non-USD-quoted
pair; I1–I5 hold."*

The foundation is in place for all of it: `solfx-math` already carries `pnl`, `margin`,
`pricing` and `fees` with 209 tests behind them, and the oracle path returns a
`quote_conversion_rate` that nothing consumes yet.
