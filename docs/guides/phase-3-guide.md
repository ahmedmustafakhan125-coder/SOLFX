# Phase 3 Guide — the Position Engine

**Flowchart:** [`../diagrams/phase-3.png`](../diagrams/phase-3.png)
**Deliverable:** the full trade lifecycle — open, increase, decrease, close, margin
adjustment — plus the four-vault settlement layer and (early) `add_liquidity`.
**Exit criteria:** the lifecycle works on a direct pair **and** a non-USD-quoted pair;
invariants I1–I5 hold. Met; the synthetic-cross case also passes even though Phase 0b moved
it off the critical path.

---

## 1. `Position` — the account

`["position", user_account, market_index, nonce]`, isolated margin (ADR-004): the account
holds its **own** collateral, moved out of `UserAccount.free_collateral` at open. Fields
worth explaining:

| Field | Why it exists |
|---|---|
| `size_base` | base-currency units (1 lot = 1e14) — lots are presentation, never engine units |
| `entry_price` | at 1e9; re-weighted on increase |
| `collateral` | the isolated margin. I1 counts it |
| `entry_notional` | **OI is removed at the value it was added at.** Recomputing notional at close on a converted market would drift by the FX rate — ×88 on USD/INR — and invariant I4 (`Σ positions == market OI`) would break silently |
| `cum_funding_entry`, `cum_borrow_entry` | the cumulative-index snapshots — the O(1) funding model |
| `opened_at_slot` | the § 6.6 minimum-hold guard's clock |
| `referrer` | copied from the user account at open, so the rebate stream (Phase 6) never needs a second lookup |
| `signed_size()`, `Direction::flip()` | helpers for skew arithmetic |

---

## 2. `open_position` — every check, in order, and why the order

1. **`market.status.allows_open()` + not paused.** Cheapest checks first; and the status
   gate is the session machine's enforcement point (C-1 ends here).
2. **`load_validated_price` — Trading gate.** One feed for direct, two for synthetic, plus
   one for quote conversion. All six oracle gates per feed.
3. **Execution price** (`execution_price_for`): oracle ± (base + confidence + skew) on the
   **adverse side** — the § 6.6 anti-toxic-flow layer. Exposed as a helper because
   `risk::assess` (Phase 4) must mark positions at the *same* price a close would fill at:
   two definitions of "the price" would mean a position liquidatable by one measure and
   healthy by the other.
4. **Slippage** (`validate_slippage`): buy fills at or under `max_price`, sell at or over.
   The trader's protection — the one check that points *their* way.
5. **Notional + conversion (C-3):** `size × exec / 1e12`, then `QuoteConversion` into USDC.
   From here on, every figure is in collateral units.
6. **Floors and bounds:** notional ≥ $1 (T14 — below it, fee and spread can round to zero
   and a round trip becomes free); size within the market's `[min, max]`.
7. **Margin:** `collateral ≥ initial_margin` (ceil) and `leverage ≤ effective_max_leverage`
   — both per-market data (EM 20x, majors 50x — the Phase 0b tiers as configuration).
8. **Fee:** `notional × open_fee_rate` (ceil, min 1), split 55/25/10/10.
9. **Settle** (below) — the only place tokens move.
10. **OI:** `add_open_interest` (quote *and* base units — quote for caps, base for skew and
    funding), then `check_oi_cap` on the resulting book. Phase 5 adds the skew/utilisation/
    insurance breakers at the same point.
11. **Write the PDA, emit `PositionOpened`.** Events are the indexer's only data source —
    an unemitted mutation is a hole in the audit trail.

Any failure is a **named** error (`SlippageExceeded`, `InsufficientMargin`,
`LeverageTooHigh`, `MarketClosedForOpens`…) — § 7.2 requires the frontend be able to say
*what* was refused.

---

## 3. `flows.rs` + `settle` — conservation by construction

The financial heart of the phase. `compute_flows(fee, pnl, split)` produces the complete
set of inter-vault transfers for a settlement, and **`Flows::is_conservative()` is asserted
before any token moves**: every unit leaving one vault arrives in another — nothing minted,
nothing burned. That is invariant **I7 enforced up front**, rather than discovered in a
test after the fact.

Two mechanics worth knowing:

- **The LP leg is netted.** The pool both receives its 55% fee share and pays/receives the
  trader's PnL; netting the two into one transfer saves an SPL CPI (~5k CU, ~10% of a
  close) and reduces the failure surface.
- **`settle` is shared.** Open, close, decrease, liquidation (Phase 4) and ADL all route
  through the same `SettlementInput` → transfers path. There is exactly one place money
  moves between vaults; nothing else holds a vault signer.

### Where each fee share goes

```
fee ──► LP vault (55%)          — the pool must be paid or liquidity leaves
    ──► treasury (25%)          — revenue (+ the rounding dust: conservative remainder)
    ──► insurance (10%)         — the § 6.9 reserve grows on every trade
    ──► referral pool (10%)     — accrues against Position.referrer (Phase 6 claims it)
```

---

## 4. The rest of the lifecycle

**`close_position` / `decrease_position`.** Enforce `min_hold_slots` first (§ 6.6: a
same-slot round trip must never scalp a spread narrower than the true market's), fill on
the adverse side, realise PnL (`proportional_pnl` for partials — floored, so slicing never
beats one close), charge the close fee on the closed notional, settle (pool claim =
−trader PnL), remove OI **at `entry_notional`**, decrement counters, return rent on full
close. Status gate: `allows_close()` — `ReduceOnly` and `GapWindow` permit exits;
**`Halted` freezes voluntary exits but never liquidation** (§ 7.3: letting someone close
*into* a dislocated price would let them realise a number nobody can verify).

**`increase_position`.** New fill priced with full gates, entry re-weighted
(`weighted_entry_price`, adverse rounding), margin re-checked **on the whole position** at
the new entry.

**`add_position_collateral` / `remove_position_collateral`.** Pure reclassification
between `free_collateral` and the position — no tokens cross the vault boundary, so I1
holds trivially. Removal is the guarded direction: the position must remain at or above
**initial** margin (not merely maintenance — a trader must not park a position at the edge
of liquidation), within leverage, and not currently liquidatable. Phase 4 upgraded this
check to `risk::assess` so it prices accrued carry/funding too.

**`add_liquidity` (shipped early).** The pool is the counterparty (ADR-001); a winning
close pays *from* the LP vault, so without deposits Phase 3 could only test losing trades —
half the lifecycle, and the half that doesn't touch solvency. Share maths lives in Phase 5's
`solfx-math::lp`; withdrawal deliberately waited for its T6 defences.

---

## 5. Worked example — the whole path with real numbers

Long 0.1 lot EUR/USD, oracle 1.08543, base spread 1 bp, conf 1.28 bps → mult 1.0, no skew:

```
total_spread = 1 + 2 (conf ceil) = 3 bps        (conf_bps ceils 1.28 → 2)
exec  = 1.08543 × 1.0003        = 1.08576  (buy side, adverse)
notional = 1e13 × 1.0857576e9 / 1e12 ≈ $10,857.58
IMR (2%)  = $217.16 (ceil)  → collateral $250 accepted, leverage 43x ≤ 50x
fee (1 bp) = $1.09 (ceil)   → LP $0.599 / treasury $0.273 / insurance $0.109 / referral $0.109
                              (floors; treasury takes the remainder unit)
```

Close immediately at the same oracle: exit fills at 1.08510 (sell side). The trader crosses
the 3 bp spread twice (≈ $6.51) and pays two fees (≈ $2.17) with zero price movement —
**the round trip always loses**, which is the property that keeps the vault alive.

**Test cases:** [`../test-cases/phase-3-tests.md`](../test-cases/phase-3-tests.md)
