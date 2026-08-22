# Phase 7 — Test Cases (27 tests)

**Guide:** [`../guides/phase-7-guide.md`](../guides/phase-7-guide.md) · **Flowchart:** [`../diagrams/phase-7.png`](../diagrams/phase-7.png)

A stop-loss is the one order a trader places **hoping never to need it**, which means the day
it matters is the day nobody is watching. So this suite does not check that trigger orders
exist; it checks the properties that make them *dependable*:

1. **The four directions fire the right way round.** A stop wired backwards closes winners and
   holds losers, and reads to a trader as a market-conditions complaint rather than a bug.
2. **Anyone can fire it.** A trader's stop must not depend on the operator's bot being up.
3. **Somebody is paid to.** A permissionless instruction nobody is paid to call is a promise
   with no mechanism behind it.
4. **It is a trigger, not a promised price.** In a gap it fills far past the trigger.

The keeper's own suite is small on purpose. Most of what a keeper "decides" is not the
keeper's logic at all — it calls `risk::assess` and the oracle gates directly, so those
decisions are already covered by the 26 oracle and 23 risk-engine tests. What is genuinely the
keeper's own is the feed comparison and the address derivation, and that is what is tested
here.

**Run:**
```bash
anchor build
cargo test -p solfx-core --test triggers
cargo test -p solfx-core --test compute_budget
cargo test -p solfx-keeper
```

---

## integration: trigger orders — `programs/solfx-core/tests/triggers.rs` (14)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_trader_can_attach_a_take_profit_and_a_stop_loss` | Both halves of a bracket rest on one position; each stores its own kind, price and owner. | — |
| 2 | `a_trigger_already_met_is_refused_at_placement` | A TP below a long, or an SL above it, is a delayed market close — refused at placement, not on the next keeper pass. | `TriggerAlreadyMet` |
| 3 | `a_short_takes_profit_below_and_stops_above` | The mirror on a short, and that the wrong way round is still refused. | `TriggerAlreadyMet` |
| 4 | `a_trigger_cannot_be_larger_than_the_position` | Size is bounded by the position at placement. | `ReductionExceedsSize` |
| 5 | `only_the_owner_can_cancel_a_trigger` | A stranger cannot remove someone's stop-loss; the owner can, and gets the rent back. | `AuthorityMismatch` |
| 6 | `anyone_can_fire_a_met_trigger_and_is_paid_a_tip` | **The property that matters most.** A stranger — not the operator — fires the order, the position closes, the order is consumed, and the keeper is net *up* after fees. | — |
| 7 | `a_trigger_that_is_not_met_cannot_be_fired` | Price moved toward the trigger but not far enough; the position survives. | `TriggerNotMet` |
| 8 | `a_stop_loss_on_a_long_fires_when_the_price_falls` | A rise does **not** fire it; a fall does. The case a wrong comparison would silently invert. | `TriggerNotMet` on the rise |
| 9 | `a_stop_loss_on_a_short_fires_when_the_price_rises` | The fourth combination, tested both ways for the same reason. | `TriggerNotMet` on the fall |
| 10 | `a_gap_fills_far_past_the_trigger_and_says_so` | A 200-pip gap through the stop realises the *actual* loss. The protocol does not eat the difference and does not pretend the stop held. | — |
| 11 | `a_partial_trigger_scales_out_of_the_position` | Half the size closes; the rest keeps running and the position stays open. | — |
| 12 | `a_halted_market_does_not_fire_triggers` | § 7.3 freezes positions, so a trigger cannot realise a price nobody can verify. Liquidation remains the only thing that moves. | `MarketHalted` |
| 13 | `a_stale_price_cannot_fire_a_trigger` | A trigger reads through the same gates as any trade — a stale feed cannot fire a stop. | `StalePrice` |
| 14 | `a_bracket_leaves_the_other_side_resting` | Firing the TP consumes it and leaves the SL. OCO pairing is the frontend's job, not the program's — and the leftover can be cancelled for its rent. | — |

Every test asserts invariants **I1–I8** after each step.

---

## budgets & packet sizes — `programs/solfx-core/tests/compute_budget.rs` (2)

| # | Test case | Verifies |
|---:|---|---|
| 15 | `keeper_instructions_stay_within_their_ceilings` | `place_trigger_order` 21,838 CU (ceiling 60k); `execute_trigger_order` 56,713 CU (ceiling 200k). Measured with a **stranger** firing it, since that is the call that has to land in a fast market. |
| 16 | `every_keeper_transaction_fits_in_one_packet` | All five keeper instructions serialise inside 1232 bytes on the **widest market the protocol can produce** — synthetic *and* non-USD-quoted, three distinct oracle legs — with the compute-budget instructions included, because those are not optional in production. |

Why 16 is a hard requirement and not a nice-to-have: a transaction that does not fit must be
split, and splitting a liquidation means holding intermediate state between two transactions
that may land in different blocks. During the congestion spike that made the liquidation
necessary — the only time it matters — the second half is the one that fails to land.

This is also the measurement the keeper's Pyth design rests on. A signed Wormhole update does
not fit alongside seventeen accounts, which is why the keeper *references* Pyth's sponsored
price-feed accounts rather than posting its own.

Measured: `liquidate_position` 758 · `execute_trigger_order` 661 · `crank_market_price` 395 ·
`crank_market_session` 331 · `crank_funding` 296.

---

## unit: feed comparison — `crates/solfx-keeper/src/services.rs` (6)

The watchdog's job is to notice that an on-chain price account disagrees with the market. A
feed can advance its timestamp while publishing a *wrong* price — no staleness check catches
that, and a single oracle cannot tell that it is the one that is wrong.

| # | Test case | Verifies |
|---:|---|---|
| 17 | `identical_prices_do_not_diverge` | The baseline: no difference reads as zero. |
| 18 | `a_different_exponent_is_not_a_divergence` | 1.08543 at `1e-5` and at `1e-8` are the same number. **The reason this function exists** — comparing raw mantissas would report a feed whose exponent changed as a catastrophic divergence when nothing moved. |
| 19 | `a_one_percent_gap_reads_as_a_hundred_bps` | The scale is right. |
| 20 | `divergence_is_absolute` | A feed reading low is exactly as broken as one reading high. |
| 21 | `a_zero_reference_is_not_comparable_rather_than_infinite` | Returns `None`, not a division by zero or a meaningless huge number. |
| 22 | `the_alarm_threshold_separates_jitter_from_a_broken_feed` | 10bps must not alarm; 100bps must. The two sources are sampled at different instants, so a fast market produces a real difference that is nobody's fault — alarming on that would make the alarm worthless. |

## unit: Pyth addressing — `crates/solfx-keeper/src/pyth.rs` (5)

| # | Test case | Verifies |
|---:|---|---|
| 23 | `an_unset_feed_is_recognised` | An all-zero feed id means "no second leg"; one non-zero byte is enough to be set. Prevents a direct market chasing a price account that does not exist. |
| 24 | `a_feed_id_determines_its_price_account` | Derivation is stable and collision-free. **The address must be derived, not configured** — otherwise listing a market means editing every keeper's address book, and a keeper that missed the edit goes quiet on exactly the market nobody is watching yet. |
| 25 | `feed_ids_are_hex_encoded_the_way_hermes_wants_them` | 64 bare lowercase hex characters, no `0x`. |
| 26 | `confidence_is_reported_as_a_fraction_of_price` | 1.3bps on a normal major; 499bps during a depeg — the quantity § 7.2 gates trading on and correction C-4 deliberately does *not* gate liquidation on. |
| 27 | `a_zero_price_has_no_meaningful_confidence` | Returns `None` rather than dividing by zero. |

---

## What this suite deliberately does not test

**The keeper's margin arithmetic**, because it has none. It calls `risk::assess` and
`oracle::load_price_for_liquidation` directly — the same functions the program runs. Testing a
keeper-side copy would be testing a copy; the value is that no copy exists.

**The loops themselves.** `run_liquidator` and friends are `tokio` intervals around pure
functions that are tested. A test that spins one up and waits would be testing `tokio`.

**Live-weekend firing and sub-2s latency under load.** Both exit criteria need wall-clock time
against a real cluster. Everything they depend on is covered — session transitions, gap
windows, stale-feed refusal, transaction size, compute units — but the criteria themselves are
devnet observations, not tests, and are recorded as open in the Phase 7 report.
