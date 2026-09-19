# Phase 4 Guide — Risk Engine & Market Regimes

**Flowchart:** [`architecture/phase-4.png`](../../architecture/phase-4.png)
**Deliverable:** funding & carry cranks, liquidation, the insurance fund, auto-deleveraging,
both regime state machines, and the three-way oracle gate.
**Exit criteria:** the CHF-depeg replay survives; a weekend, an EM holiday and the
continuous-regime cycle all transition correctly. Met — and writing the replay found **four
real defects** no unit test would have caught (each is called out below where it lives).

---

## 1. `risk.rs` — one definition of "what is this position worth"

Liquidation, ADL and margin withdrawal all judge positions. If they used different maths, a
position could be liquidatable by one measure and healthy by another — so there is exactly
one assessment function.

### `assess(position, market, price) → Health`

Marks the position at the **adverse-side exit price** (the same `execution_price_for` a
close would fill at — marking at the mid would overstate the whole book's solvency by half
a spread), converts through C-3, accrues costs, and returns *two* equities:

```
carry    = entry_notional × (cum_borrow_now − snapshot) / 1e9        (what is owed since entry)
funding  = size × (cum_funding_side_now − snapshot) / 1e9            (signed: light side receives)
close_fee = notional_now × close_fee_rate                            (estimate)

equity              = collateral + uPnL − carry − funding − close_fee   ← §6.5: display & withdrawal checks
liquidation_equity  = collateral + uPnL − carry − funding               ← §6.8: the liquidation test
```

The close fee is excluded from the liquidation test **deliberately**: a liquidation pays the
*penalty*, not a close fee — including both charges twice for one exit and liquidates
solvent positions. `Health::is_liquidatable()` is strict `<` (T7: equality is not
liquidatable, or boundary-sitting positions could be griefed for their penalty).

### `settle_funding` / `settle_carry`

- **Funding moves no tokens.** Both sides' money is already in the collateral vault, so
  settling reallocates between `Position.collateral` and `Market.funding_balance` — the
  paid-but-unclaimed pot that invariant I1 counts. Funding therefore *cannot* touch LP
  capital: invariant **I3 holds structurally**, not by check. Receipts are capped at the
  balance actually held (the cumulative-index model's partial-interval approximation can
  promise slightly more than was collected; the cap closes that for good).
- **Carry is revenue.** It leaves the position's collateral and routes through the same
  55/25/10/10 split as a trading fee at the next settlement. Charges are capped at the
  collateral — a position that cannot pay its carry is a liquidation case, not a negative
  number.

---

## 2. `crank_funding` — the indices advance (§ 6.7)

Permissionless, hourly, **reads no oracle** — both rates derive from open interest and
configured rates, so the crank cannot be blocked by a stale or wide feed and keeps working
through exactly the conditions where funding matters most. A redundant crank in the same
second is a no-op, not an error (keepers run in triplicate by design; racing instances must
not fail each other).

```
carry:    markup (both sides pay)  +  interest differential        → cum_borrow_index  (never decreases)
funding:  rate = clamp(skew_ratio × k, ±cap)  per hour             → cum_funding_long / _short
          heavy side pays; light side's rate scaled by OI ratio so payments == receipts (I3)
```

The two central-bank rates and the markup live as **separate fields** on `Market`
(`rate_base_annual`, `rate_quote_annual`, `carry_rate_per_hour`) because *publishing the
split is the product*: a trader can read the true differential and our markup as separate
line items instead of discovering a blended number after rollover.

Known simplification, documented at the definition: one carry index advanced at the
long-side rate (clamped at zero), so a short never pays less than the markup. Revisit if a
listed market's differential ever dominates its markup.

---

## 3. `crank_market_session` — where C-1 is actually stopped (§ 7.3)

Two **independent** signals, and trading restricts if *either* says closed:

1. **The feed itself** — a validated read succeeds within the staleness window, or the
   market is closed. Handles holidays with no calendar to maintain (the feed just stops).
2. **The per-market UTC calendar** — `calendar_is_open` / `seconds_until_close` over a
   weekly cycle that may wrap the week boundary (interbank: Sunday 21:00 → Friday 21:00).
   Epoch arithmetic note: 1 Jan 1970 was a *Thursday*, so `seconds_into_week` carries a
   4-day offset — and a unit test pins 2026-08-01 to Saturday so a wrong constant fails
   loudly (it already caught one).

**The transition table (`next_status`) is a pure function** of observable signals — a state
machine only reachable through transactions is one whose corners never get tested; this one
has 27 unit tests.

```
Session-bound:  Active ──T−15m──► ReduceOnly ──close/feed-dead──► Halted
                Halted ──open+live──► GapWindow ──5 min──► Active
Crypto:         Active ◄──► Halted on the feed alone (24/7 is the feed's property, not our promise)
Continuous:     Active → WeekendMode (derated) → PreOpenWindow → Active
                — written & tested, unreachable in production pending Q1b
```

Details that carry weight: `ReduceOnly` at T−15 min stops risk being taken that cannot be
managed until Monday; `GapWindow` (5 min, closes+liquidations only) exists because the
weekend's news lands in one tick at reopen; its timer runs on **`status_changed_at`**
(time-in-state), never on crank cadence — a risk control's duration must not depend on
keeper scheduling; a manually-`Halted` market is never reopened by the crank (the operator
halted it for a reason the cranker cannot see).

---

## 4. `PriceGate` — three ways to read a price, each named

The depeg replay exposed that one confidence ceiling **inverts** during a crisis:
confidence blows out during exactly the events that make positions liquidatable (real CHF
spreads went 2 pips → 500+, i.e. 400+ bps), so a single gate means *liquidation stops
working when it is needed* — and a crank that refuses an uncertain price never trips the
breaker, leaving the market Active on a pre-dislocation price (the failure mode looking
exactly like the exploit).

| Gate | Staleness | Confidence | Deviation | Used by |
|---|---|---|---|---|
| `Trading` | strict | market ceiling | enforced | open, close, margin withdrawal |
| `Liquidation` | **strict** | `liquidation_max_conf_bps` (a *tail-event* parameter — 200x the trading ceiling in fixtures; sizing it at "a few times normal" is the same as not having it) | **not re-checked** (the crank already halted and flagged a human; blocking the liquidation too would freeze the position inside the incident) | liquidate, ADL |
| `Observe` | strict | measured only | measured only | the cranks (record → breaker) |

**Staleness is strict in all three.** A stale price is C-1; no argument relaxes it.

---

## 5. `liquidate_position` — the waterfall (§ 6.8–6.9)

```
1  read through PriceGate::Liquidation
2  settle carry & funding FIRST (judge the position after its accrued costs —
   unbilled carry would delay the liquidation to a worse price)
3  assess → require liquidation_equity < maintenance_margin (strict)
4  penalty = min(liq_fee_bps × notional, max(equity, 0))     — never manufacture debt to pay a reward
   split 40 liquidator / 40 insurance / 20 treasury
   reward floored at $1, insurance tops up                    ← replay defect #4: in a severe gap
                                                                equity is wiped, 40% of nothing is
                                                                nothing, and nobody shows up
5  the pool receives AT MOST the position's collateral        ← replay defect #1: the unclamped
                                                                claim drained OTHER traders' money
                                                                from the shared vault; I1 caught it
6  residual (equity − penalty, if positive) → back to the owner's free collateral;
   the account's rent → the owner's wallet.  A liquidation takes what it is owed, not the account.
7  equity < 0 → bad debt:   insurance pays (pool made whole)
                            → uncovered remainder → Market.pending_adl_debt
8  emit PositionLiquidated (equity, penalty, reward, bad debt, insurance draw — the audit trail)
```

Liquidation is permitted in **every** status except `Initialized` — a halt that blocked it
would convert an oracle incident into a solvency incident. Self-liquidation is unprofitable
by construction (T8): the penalty always exceeds the reward, so the owner nets a loss.

`deposit_insurance_fund` is permissionless — there is nothing to gain by funding the
reserve, and requiring an authority would only let it sit under-capitalised while a
multisig assembles. § 6.9's sizing rule: 2% of max OI at launch, grown by the 10% fee
share; **never launch it empty** (the replay's no-insurance variant shows exactly what
happens: the shortfall queues for ADL instead).

## 6. `auto_deleverage` — step 2 of the waterfall

Force-closes a **profitable opposing** position and withholds part of its *profit* to cover
`pending_adl_debt`. The rules that make it defensible:

- **Profit only, never principal** — the deleveraged trader always leaves with at least
  their collateral; if the debt exceeds this winner's profit, the remainder waits for the
  next one.
- Eligibility enforced on-chain (there must *be* uncovered debt; the position must be in
  profit); the § 6.9 *ranking* (most profitable first) is the keeper's job, since no
  instruction can see every position — a mis-ranked pick is a fairness issue visible in the
  event stream, not a solvency issue.
- No close fee — the trader did not choose to close.
- Traders must be told ADL exists **before** they open (§ 6.9). Every venue that hid it and
  then used it was destroyed for it.

---

## 7. The scenario replays (§ 12.4) — the exit gate

*"If the engine survives a synthetic CHF-depeg replay with the insurance fund intact, you
have something. Until then you have a demo."*

| Scenario | What it proves |
|---|---|
| **CHF depeg, 15 Jan 2015** (−30%, no ticks between) | conservation to the unit across five bad-debt liquidations; waterfall order (insurance → ADL queue → LPs); liquidators stayed paid; the protocol reopens and trades afterwards |
| Depeg with an **empty** insurance fund | conservation still holds; the shortfall queues for ADL — defining the behaviour § 6.9 warns against |
| **GBP flash crash, 7 Oct 2016** (−6% wick, recovery) | the deviation breaker halts *instead of* liquidating into the wick; a solvent-throughout trader closes roughly whole after the recovery |
| **Weekend stale-price exploit** (C-1 attempted three ways) | ReduceOnly refuses the pre-close open; Halted refuses the Friday-price open; the staleness gate refuses it even on an un-cranked Active market — two independent defences |
| **Weekend gap** | Fri open → weekend freeze → Sunday reopen 200 pips lower → GapWindow (no new opens) → clean liquidation → Active |
| **EM local hours / holiday** | USD/BRL halts at 21:00 UTC while EUR/USD trades on; a mid-session feed stop halts a market whose calendar says open |

The four defects the replay found (1: unclamped pool claim; 2: one confidence ceiling
blocking crisis liquidation; 3: gated crank never tripping the breaker; 4: zero reward =
no liquidator) are each now a permanent regression test.

**Test cases:** [`../test-cases/phase-4-tests.md`](../test-cases/phase-4-tests.md)
