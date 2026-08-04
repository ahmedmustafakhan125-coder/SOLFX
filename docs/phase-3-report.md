# Phase 3 — Position Engine

**Status:** complete.
**Exit criteria (`ARCHITECTURE.md` § 15):** *"Lifecycle tested on a direct pair, a synthetic
cross **and** a non-USD-quoted pair; I1–I5 hold."*

| Criterion | Result |
|---|---|
| Lifecycle on a direct pair | ✅ EUR/USD and XAU/USD, both directions |
| Lifecycle on a synthetic cross | ✅ EUR/GBP composed from EUR/USD ÷ GBP/USD |
| Lifecycle on a non-USD-quoted pair | ✅ USD/INR with its conversion feed |
| I1 — collateral vault balances | ✅ asserted three ways after every instruction |
| I2 — LP vault == AUM | ✅ |
| I4 — open interest matches positions | ✅ base units exactly, quote units within rounding |
| I5 — no size without collateral | ✅ |
| I6, I7 — insurance and total conservation | ✅ (beyond the stated criteria) |
| **Round trip at an unchanged price always loses** | ✅ **on chain, on every market shape** |

---

## What was built

Six trader instructions, one LP instruction, and the settlement layer they share.

```
instructions/trader/
├── flows.rs                the four-vault arithmetic — conservation, unit-tested
├── mod.rs                  settle(): the only function that moves USDC between vaults
├── open_position.rs        + execution_price_for(), shared by every entry point
├── increase_position.rs    weighted entry, rounded against the trader
├── close_position.rs       decrease + close; bad-debt capping
└── adjust_collateral.rs    add / remove margin
instructions/lp.rs          add_liquidity (deposit only — see below)
state/position.rs           Position + Direction
```

`Market` and `Protocol` each gained fields, taken from `_reserved` rather than appended to
the struct's end — the § 5.6 rule exercised for real:

| Struct | Added | `_reserved` |
|---|---|---|
| `Market` | `min_hold_slots`, `open_position_count` | 128 → 120 |
| `Protocol` | `total_referral_accrued`, `total_referral_claimed`, `total_bad_debt` | 128 → 112 |

Account sizes are unchanged, and every pre-existing field keeps its byte offset.

### Tests

| Suite | Count |
|---|---:|
| `solfx-math` unit + property | 209 |
| `solfx-core` unit (incl. `flows`) | 6 |
| `solfx-core` integration | 111 |
| **Total** | **326** |

**`open_position` costs 55,584 CU against § 5.5's 120,000 budget** — 54% headroom. Full table
in [`compute-budget.md`](compute-budget.md).

---

## The test that matters

> *Opening and immediately closing at an unchanged oracle price must always lose money.*

`solfx-math` has property-tested this since Phase 1. Phase 3 proves it **through the real
instructions**, where the spread, both fees, the quote conversion and the vault plumbing all
have to line up — and on every market shape, because the conversion path (C-3) is exactly
where a sign or a rounding direction could flip:

- direct USD-quoted (EUR/USD), long and short
- metals (XAU/USD), long and short
- non-USD-quoted (USD/INR)
- synthetic cross (EUR/GBP)

If it ever passes, there is free money in the protocol and bots will extract it until the
vault is empty.

---

## Four decisions worth attention

### 1. Open interest is removed at the notional it was **added** at

The first implementation recorded OI in USDC on open and removed it in quote units on close.
On EUR/USD those are the same number. On USD/INR they differ by **88x**, and invariant I4
would have failed on any converted market — but only after a trade, which is exactly the kind
of bug that survives a happy-path test.

The fix is `Position::entry_notional`: the USDC figure this position contributed, stored so it
can be removed exactly. Recomputing at close would use the current price *and* the current
conversion rate, so the counter would drift with every price move and I4 would fail on a
market that had merely traded.

### 2. Bad debt is capped and recorded, not absorbed

Phase 4 owns liquidation. Until it exists a position can run past the point its margin covers,
and closing it would owe the pool more than the trader has.

The loss is therefore **capped at what the trader posted**, and the shortfall is written to
`Protocol.total_bad_debt` and emitted as `BadDebtIncurred`. The pool eats it in the meantime —
which is § 6.9's *last* waterfall step, reached first because the two above it are not built
yet. The money is conserved, the shortfall is visible, and Phase 4 inserts the insurance fund
and ADL ahead of it.

The sign here is easy to get wrong. The capped PnL is `-(released - fee)`, which makes the
trader's net movement exactly `-released`: they lose everything they put up and no more.

### 3. `add_liquidity` shipped early; withdrawal deliberately did not

The pool is the counterparty to every trade, so a winning position is paid out of `lp_vault`.
Without a way to fund it, Phase 3 could only test *losing* trades — half the lifecycle, and
the half that never touches the pool's solvency path.

Withdrawal stayed behind, and the split is not laziness. `request_remove_liquidity` /
`remove_liquidity` / `cancel` carry the 24-hour cooldown and exit fee that defend against the
JIT attack (threat T6), where an LP deposits ahead of a known trader loss and withdraws ahead
of a known gain. Shipping the withdrawal path without its defences would be worse than not
shipping it. **Liquidity is one-way until Phase 5 — a safe direction to be incomplete in.**

### 4. `max_position_size` is in base units, and the field name does not say so

The test fixture read it as USDC and every position test failed at once. The bound is compared
against `Position::size_base`, so it is in `BASE_PRECISION` units, and it has to be: a notional
cap would silently tighten on a rally and loosen on a selloff.

The consequence is that a base-unit minimum is asset-specific — a sane floor for EUR/USD is
~40,000x too large for gold, because an ounce is worth ~3,700 euros. So the *money* floor is
`pricing::validate_notional` ($1 of notional), which applies uniformly, and
`min_position_size` exists only to stop dust. Both fields are now documented with their units.

---

## Design notes

**One settlement function.** `trader::settle` is the only code in the program that moves USDC
between vaults, for the same reason `oracle::load_validated_price` is the only code that reads
a price: one place to audit. `flows::compute_flows` computes the four deltas and asserts they
sum to zero *before* any token moves — if USDC has been created or destroyed, the transaction
fails rather than continuing for invariant I7 to discover later.

**The LP fee share is netted against PnL.** The pool both receives its share of the fee and
settles the trader's PnL, in opposite directions. Doing them as two transfers costs ~5k CU and
can order them so the vault is momentarily short of its own obligation. One signed movement is
cheaper and cannot transiently under-fund the pool.

**Removing margin needs a price; adding does not.** Adding is a reclassification inside
`total_user_collateral` — I1 holds trivially. Removing raises leverage, so the position is
re-checked against **initial** margin, not maintenance: a trader must not be able to walk a
position to the edge of liquidation and leave it there. It is also marked on the *adverse*
side, exactly as a close would be, so a trader cannot withdraw against half a spread they have
not paid.

**Weighted entry rounds against the trader.** A long's average entry rounds up, a short's
down. Both mean a slightly worse entry, so topping up in small increments cannot manufacture a
better average — which would otherwise be a free-money edge repeated on every add.

---

## Deferred, with reasons

| Deferred | To | Why |
|---|---|---|
| Liquidation, health-factor keeper path | Phase 4 | The whole risk engine lands together; a liquidation without an insurance fund and ADL is half a mechanism |
| Funding and carry accrual | Phase 4 | `Position` already carries the index snapshots and `AccruedCosts` is already threaded through the margin checks — Phase 4 changes the inputs, not the logic |
| Trigger orders (TP/SL) | Phase 4 | Placing an order nothing can execute is half a feature; `execute_trigger_order` is a keeper instruction |
| LP withdrawal | Phase 5 | See decision 3 |
| Referral accrual → claim | Phase 6 | Accrual is live and recorded per trade (`Protocol.total_referral_accrued`, `FeeCollected` events); only the claim path is missing |
| Volume-tier decay | Phase 6 | `thirty_day_volume` accumulates but never rolls off, so tiers only ratchet down in fee |

---

## Open questions carried forward

| # | Question | Status |
|---|---|---|
| **Q1b** | How is Pyth Indices for metals distributed? | 🔴 Open — commercial, one email to Pyth |
| Q2 | Per-market session boundaries, especially EM local calendars | Needed for Phase 4's regime machine |
| Q4 | Friday-close / Sunday-open gap size — sizes `GapWindow` and the MMR buffer | Needed for Phase 4 |
| — | **Transaction size** with three real Hermes `post_update` payloads | Compute is settled; the 1232-byte limit is not, and LiteSVM cannot test it. Phase 7. |

---

## Next: Phase 4 — Risk engine and regimes

Funding, carry, liquidation, insurance, ADL, and both regime state machines.

Exit criteria: *"CHF-depeg replay survives; a real weekend, an EM holiday, and a full metals
`WeekendMode → PreOpenWindow` cycle all transition correctly."*

Two notes on that criterion, given what Phase 0b measured:

- The **metals `WeekendMode` cycle cannot be tested against a real feed**, because no
  continuous metals feed exists. It can be tested against a synthetic one, and the state
  machine can be proven correct — but it stays unreachable in production until Q1b resolves.
- The **CHF-depeg replay is the real gate.** § 12.4: *if the engine survives it with the
  insurance fund intact, you have something; until then you have a demo.*

The groundwork is in place: `Position` carries the funding and borrow index snapshots,
`margin::AccruedCosts` is already threaded through every margin check with zeroed inputs, and
`solfx-math::funding` has been tested since Phase 1.
