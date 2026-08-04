# Phase 4 — Risk Engine and Market Regimes

**Status:** complete.
**Exit criteria (`ARCHITECTURE.md` § 15):** *"CHF-depeg replay survives; a real weekend, an EM
holiday, and a full metals `WeekendMode → PreOpenWindow` cycle all transition correctly."*

| Criterion | Result |
|---|---|
| **CHF-depeg replay survives** | ✅ with the insurance fund intact and every invariant holding |
| A real weekend transitions correctly | ✅ full cycle: `Active → ReduceOnly → Halted → GapWindow → Active` |
| An EM holiday transitions correctly | ✅ local hours *and* a mid-session feed stop |
| Metals `WeekendMode → PreOpenWindow` cycle | ✅ state machine proven — ⚠️ **unreachable in production**, see below |
| Funding, carry, liquidation, insurance, ADL | ✅ |

**381 tests** (209 math, 172 program). `liquidate_position` costs **59,223 CU** against
§ 6.8's hard 200,000 ceiling.

---

## The criterion that cannot be met against a real feed

The metals `WeekendMode → PreOpenWindow` cycle is tested and correct, but **no market can
occupy the continuous regime in production.** Phase 0b found no 24/7 metals feed, and
`initialize_market` rejects `FeedKind::ContinuousIndex` outright.

So the transitions are proven against a synthetic feed and unit-tested exhaustively, and the
variant stays blocked. The day **Q1b** resolves, turning it on is a configuration change
rather than a code change — which is only true because the code was written and tested now.
Code that has never run is code that does not work.

---

## What was built

```
src/risk.rs                          position health; the two equities; funding/carry settlement
src/oracle.rs                        + PriceGate — three ways to read a price
src/instructions/keeper/
├── funding.rs                       crank_funding — funding and carry indices
├── session.rs                       crank_market_session — both regimes, C-1's actual defence
├── liquidate.rs                     liquidate_position + deposit_insurance_fund
├── adl.rs                           auto_deleverage
└── price.rs                         crank_market_price (now trips two breakers)
tests/risk_engine.rs                 23 tests
tests/scenarios.rs                   7 historical replays
```

`Market` and `Protocol` took eight more fields from `_reserved` (§ 5.6 again):
`funding_balance`, `funding_rate_k`, `rate_base_annual`, `rate_quote_annual`,
`liquidation_max_conf_bps`, `pending_adl_debt`, `status_changed_at`, `last_session_crank_ts`;
and `total_liquidator_paid`. Account sizes unchanged.

---

## Four defects the scenario replays found

§ 12.4 is blunt about why these exist:

> *If the engine survives a synthetic CHF-depeg replay with the insurance fund intact, you
> have something. Until then you have a demo.*

Writing that replay found four real defects. **None would have been caught by a unit test**,
because each only appears when a 30% gap, a blown-out confidence band and an empty position
coincide — which is exactly the combination a crisis produces.

### 1. The pool's claim was not capped at the position's collateral

A bad-debt liquidation computed the pool's claim as `collateral − equity`. When equity went
negative that exceeded the collateral, and `settle` moved the difference out of the collateral
vault — **which holds every other trader's money too**.

Invariant I1 caught it immediately, and only because the harness checks it after every
instruction rather than at the end of a scenario. The claim is now capped at what the position
actually holds; the remainder is bad debt and goes to the waterfall.

### 2. The confidence gate blocked liquidation during the depeg

§ 7.1 gates every price on `conf / price <= max_conf_bps`, and for *opening* that is exactly
right (C-4). Applying the same ceiling to liquidation inverts the logic:

> **Confidence blows out during precisely the events that make positions liquidatable.**

During the real depeg, quoted spreads went from ~2 pips to 500+ — over 400 bps of uncertainty
on a 1.20 price. Under one ceiling, every liquidation on the day it mattered would have been
refused, and the positions would have kept falling with nobody able to close them.

The fix is an explicit three-way `PriceGate`:

| Gate | Staleness | Confidence | Deviation | Used by |
|---|---|---|---|---|
| `Trading` | strict | market ceiling | enforced | open, close, margin withdrawal |
| `Liquidation` | strict | **liquidation ceiling** | **not checked** | liquidate, ADL |
| `Observe` | strict | measured only | measured only | the cranks |

Deviation is not re-checked for liquidation because the crank has *already* halted the market
on it and flagged it for a human. Blocking the liquidation too would freeze the position in
the state the halt was called to contain.

**Staleness stays strict everywhere.** It is C-1, and it is the one gate no argument justifies
relaxing.

`Market::liquidation_max_conf_bps` is a **tail-event parameter**, not a normal-day one. The
harness sets it 200x the trading ceiling; the depeg scenario sets it higher still. Sizing it
at "a few times normal" is the same as not having it.

### 3. A price too uncertain to record never tripped the breaker

The crank is what halts a market. Gating the crank on confidence meant a blown-out price was
*refused* rather than recorded — so the market stayed `Active`, holding a last-known price
from before the dislocation.

The failure mode looks exactly like the exploit it is meant to prevent.

The crank now observes and trips a breaker on **wide confidence as well as deviation**. Three
Phase 2 oracle tests changed to assert the new contract; the rejection they used to check now
lives on the trading path, where it belongs.

### 4. A liquidator paid nothing does not show up

In a severe gap the position's equity is entirely wiped, so the penalty is zero and the
liquidator's 40% share of zero is zero. Nobody runs the liquidation. The position stays open
and its shortfall grows.

That is § 6.8's stated failure — *an unprofitable liquidation is an unliquidated position, and
unliquidated positions are how vaults die* — reached from the opposite direction. Not because
the reward was set too low, but because there was nothing left to take it from.

The **insurance fund now backstops the reward** to a $1 floor. The arithmetic is not close: a
dollar to close a position now, against a shortfall that grows every minute it stays open.

---

## Design decisions worth review

### Funding never touches LP capital, by construction

§ 6.7 requires funding to be a pure transfer between traders (invariant I3). Rather than
enforce that with a check, it is true structurally: **funding moves no tokens at all.** Both
sides' money is already in the collateral vault, so settling reallocates between
`Position::collateral` and `Market::funding_balance` and nothing crosses a vault boundary.
Funding cannot reach LP capital because it never goes near the LP vault.

`funding_balance` holds what payers have paid and receivers have not yet claimed — still user
money, just not yet attributed. **Invariant I1 counts it.**

Receipts are capped at that balance. `funding_index_update` floors the receiving side so
payouts cannot exceed collections *at crank time*, but over an interval that weakens: a
position present for only part of it still accrues the whole delta, which is the standard
cumulative-index approximation. Capping at what is actually held closes the gap for good.

### Two equities, both correct

`ARCHITECTURE.md` gives two formulas that differ, and the difference is deliberate:

- **§ 6.5** — includes the close fee. What a *voluntary* close returns. Used for display and
  for the margin-withdrawal check.
- **§ 6.8** — excludes it. The liquidation test.

Excluding it is not a concession. At liquidation the position pays the **penalty**, not a
close fee; charging both would bill twice for one exit and liquidate solvent positions.

### The `GapWindow` timer measures time in state, not time since crank

The first implementation derived it from `last_session_crank_ts`, which would have made a risk
control's duration depend on how often keepers happened to run. `Market::status_changed_at`
fixes it, and a test asserts the window length is independent of crank cadence.

### Carry advances on one index at the long-side rate

A single index cannot represent two different rates, and splitting it would double the storage
and snapshot bookkeeping on every position. `advance_carry_index` clamps at zero rather than
running backwards, so a short is never charged less than the markup. **Revisit this if the
interest differential ever dominates the markup on a listed market** — it is the one place
where the simplification could bite.

---

## Deferred, with reasons

| Deferred | To | Why |
|---|---|---|
| Trigger orders (TP/SL) | Phase 7 | A trader convenience, not a risk control. Absent from Phase 4's exit criteria, and the executor is Phase 7 — shipping the on-chain half now would mean orders nothing fires. |
| LP withdrawal | Phase 5 | Needs the cooldown and exit fee that stop the JIT attack (T6). Liquidity stays one-way, which is the safe direction to be incomplete in. |
| Vault-utilisation and skew circuit breakers (§ 7.2) | Phase 5 | Both are LP-vault-relative; they belong with the vault's own instructions. |
| Admin instruction for funding `k` and interest rates | Phase 8 | The fields exist and are validated; an instruction to set them belongs with the frontend that would use it. Tests patch them directly. |
| Size-tiered MMR verification at scale | Phase 9 | `mmr_multiplier_bps` is implemented and unit-tested; the load test that exercises $5M+ notionals is Phase 9. |

---

## Open questions carried forward

| # | Question | Status |
|---|---|---|
| **Q1b** | How is Pyth Indices for metals distributed? | 🔴 Open — commercial. The continuous regime is written, tested and blocked pending it. |
| Q2 | Exact session boundaries per EM market | 🟡 The machine works; the *data* is provisional. Phase 9's listing checklist confirms each calendar against live data before `initialize_market`. |
| Q4 | Friday-close / Sunday-open gap size | 🟡 Sizes `GAP_WINDOW_SECONDS` and the MMR buffer. Both are constants that can be retuned without code. |
| — | Transaction size with three real Hermes payloads | 🟡 Compute is settled. LiteSVM writes price accounts directly, so the 1232-byte limit is still unexercised. Phase 7. |
| — | Carry index at the long-side rate only | 🟢 Documented above. Harmless while the markup dominates. |

---

## Next: Phase 5 — LP vault

Add/remove liquidity, LP token accounting, the withdrawal cooldown, exit fee, and fee
distribution.

Exit criteria: *"LP accounting exact under adversarial sequences; I2, I8 hold."*

`add_liquidity` already exists from Phase 3 and I2 is already asserted after every
instruction. Phase 5's real work is the **withdrawal** path and its defences — the 24-hour
cooldown and exit fee that stop an LP depositing ahead of a known trader loss and withdrawing
ahead of a known gain (threat T6). "Adversarial sequences" in the exit criteria means exactly
that attack.
