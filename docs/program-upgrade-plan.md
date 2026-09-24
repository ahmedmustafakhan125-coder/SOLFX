# The `solfx-core` upgrade — trigger binding, entry orders, and NOXFUNDS' mandatory stop

Written 2026-09-14, after the user authorised a program change. Everything in
`programs/solfx-core/` has been off limits since the pre-devnet audit; this is the case for
lifting that, scoped so the diff stays small and reviewable.

Four changes. Two are required by NOXFUNDS, one is a live bug, one is a data gap that only
shows up when you try to score a trader.

**The MCP had nothing on two of the three design questions here** — it confirmed Anchor's
account layout is `[8-byte discriminator][repr(C) T]` and that `Account<T>` validates a
*minimum* data length, which is the fact change 1 rests on, but it returned nothing on the
reserved-bytes migration pattern and nothing on binding a child PDA to a parent instance.
Those two are reasoned from this codebase and stated as such.

---

## 1. Bind a trigger to the position instance — the bug

### What happens today

Observed on devnet 2026-09-13:

```
20:15:42  OpenPosition         ok   58zFoneb…   a fresh ETH long at 2509.61
20:16:01  ExecuteTriggerOrder  ok   3zzyraTM…   closed it, 19 seconds later
```

The order that fired was a take-profit at 2500 left behind by an ETH **short** closed
earlier. A position's PDA is `["position", user_account, market_index, nonce]`, and the
client picks the lowest free nonce, so reopening on the same market lands on the **same
address**. `close_position` does not cancel outstanding triggers, and `TriggerOrder` records
nothing that distinguishes one occupant of that address from the next. The old order
attached itself to the new position carrying the previous direction's meaning, and a
take-profit on a long fires when the price *rises* past it — so it was already met the
instant the position existed.

Every layer behaved correctly. The account model let two different logical objects share one
identity, which the project's Solana rules § 3 already names as a bug rather than a collision
to handle later.

### The fix

`Position` already stores `opened_at_slot: u64`, for the minimum-hold check. It is monotonic
and unique per instance — two positions cannot occupy one PDA in the same slot, because the
first must be closed and the minimum-hold rule forbids reopening that fast.

`TriggerOrder` ends in `_reserved: [u8; 32]`. Take eight of them:

```rust
pub bump: u8,
/// The `opened_at_slot` of the position this order was placed against.
///
/// Without it a trigger cannot tell one occupant of its position PDA from the next, and a
/// stop left behind by a closed position silently arms itself against whatever opens at the
/// same address. Measured on devnet 2026-09-13; see docs/program-upgrade-plan.md § 1.
pub position_opened_at_slot: u64,
pub _reserved: [u8; 24],
```

- `place_trigger_order` stamps `position.opened_at_slot`.
- `execute_trigger_order` requires `trigger.position_opened_at_slot == position.opened_at_slot`
  and returns a new error otherwise.
- `cancel_trigger_order` is untouched, so rent on a stale order is still reclaimable by its
  owner.

**No migration.** `InitSpace` yields 8 + 24 = 32, the same total, so the account size does
not change and Anchor's minimum-length check still passes on accounts written by the old
program. Existing orders carry zeros in those bytes, which cannot equal any real slot, so
they refuse to execute. **Fail-closed is the correct default here** — the alternative is
grandfathering in exactly the orders that caused the bug.

### What could go wrong

If `opened_at_slot` were ever zero for a real position the check would reject a legitimate
order. It is set from `Clock::get()?.slot` at open and no cluster returns slot 0 after
genesis, but the test must assert it explicitly rather than assume.

---

## 2. Entry orders — the limit order SolFX does not have

### Why it is not a `TriggerKind`

`TriggerOrder` is keyed at `["order", position, order_id]`. A limit entry has no position
yet, so it cannot use that seed. It needs its own account and its own PDA:

```
EntryOrder at ["entry", user_account, market_index, order_id]
```

### Shape

| Field | Why |
|---|---|
| `user_account`, `authority`, `market_index` | ownership and routing |
| `order_id: u8` | several resting orders per market |
| `direction`, `size_base`, `collateral` | the position to create |
| `limit_price: i64` | fills at or better |
| `price_limit: i64` | the slippage bound `open_position` already demands — there is no "disabled" |
| `expires_at: i64` | a resting order with no expiry is a liability the trader forgets |
| `nonce: u8` | which position slot to open into |
| `bump`, `_reserved` | |

Three instructions: `place_entry_order`, `cancel_entry_order`, `execute_entry_order`.

### The risk that has to be measured first

`execute_entry_order` must do everything `open_position` does **plus** read and close the
entry order. `open_position` is already sixteen accounts, and this repo hit BPF frame errors
at thirteen and at eighteen. Adding two pushes into the region where it broke before.

**Measure the frame and the packet before writing the logic**, the same way NOXFUNDS Stage 0
is specified. If it does not fit, the fallback is that the keeper submits a normal
`open_position` on the trader's behalf — which requires a delegated authority and is a
materially larger design, so it is worth knowing early.

Collateral must be **reserved** at placement, not at fill. An entry order that fills into an
account with no free collateral is an order that silently does nothing, and a trader who
placed three of them expects three fills or three refusals, not a race.

---

## 3. Atomic open-with-stop — what NOXFUNDS actually needs

NOXFUNDS' `Mandate` carries `stop_loss_price` marked *"MANDATORY — not an argument the trader
may omit"*, and Stage 1's exit criterion is that a compliant trade lands **with its stop
attached atomically**.

### The part that already works

`TriggerOrder.authority` is "the only one that can cancel it". Under NOXFUNDS the authority
is the mandate PDA, not the trader, so **the trader already cannot cancel or move the stop**
— NOXFUNDS simply never exposes an instruction that does. No program change is needed for
immutability. That is worth stating plainly because it looked like the hard part and is not.

### The part that does not

Atomicity across `open_position` + `place_trigger_order` is fine in one transaction, but
NOXFUNDS reaches them by CPI from a wrapper that is already modelled at ~21 accounts against
a frame that broke at 18. Two CPIs mean the union of both account sets.

So: **`open_position_with_stop`** — one instruction that opens and places the stop, sharing
the accounts both already need. It cuts the wrapper's account count rather than adding to it,
and it is a better primitive for ordinary SolFX traders too, who today can be filled and then
fail to place their stop.

This is the change most likely to be defeated by the stack frame. Measure before building.

---

## 4. Events rich enough to score a trader

NOXFUNDS ranks traders on take-profit and stop-loss behaviour: how many hit, for how much,
and how far in pips.

`TriggerOrderExecuted` carries `kind`, `trigger_price`, `oracle_price` and `size_base`. It
does **not** carry the fill price, the entry price, or the realised P&L. Those exist in the
`PositionDecreased` emitted by the same transaction, so today the figures are only
recoverable by joining two events on a transaction signature — fragile for an indexer, and
impossible for the "every figure links to the on-chain event that produced it" promise in
NOXFUNDS Part 9.

Add three fields to `TriggerOrderExecuted`:

```rust
pub exec_price: i64,      // what it actually filled at, after the spread
pub entry_price: i64,     // so the move is computable from this event alone
pub realized_pnl: i64,    // USDC, signed
```

**Additive only.** the project's Solana rules § 8: adding a field is fine, changing or
removing one is a breaking change for Phase 8.

Pips stay off chain. `price_delta_to_pips` already exists in `solfx-math` and pip size is a
presentation concept the frontend owns — putting it on chain would bake a display convention
into consensus state.

---

## Order of work

1. **§ 1, the binding fix.** Smallest, highest value, unblocks nothing else but stops a live
   bug. Ship alone if the rest slips.
2. **§ 4, the events.** Additive, no logic change, no frame risk.
3. **§ 3 spike** — measure `open_position_with_stop`'s frame and packet. Decision point.
4. **§ 2 spike** — measure `execute_entry_order`'s frame and packet. Decision point.
5. Implement whichever of 3 and 4 fit; redesign whichever does not.

## Definition of done, per the project rules

- `program_autofixer` to a clean pass on every file touched
- every account: owner, instance, signer, mutability
- new errors are real variants in `errors.rs`, named, not invented at the call site
- `assert_invariants()` called in every new test — a new instruction that does not call it is
  outside the I1, I2, I4–I8 guarantee
- `compute_budget.rs` gets a ceiling for every new instruction, so a regression fails CI
- `cargo fmt`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` green with
  no validator running

## What this costs

The program has never been upgraded since the markets now trading were listed, and the
extensibility claim in § 12.5 rests on that. Upgrading is not free: the deployed binary's
SHA-256 changes, `docs/CONTEXT.md`'s "byte-identical to devnet" line stops being true until
re-verified, and the 97.14 % coverage figure has to be re-measured. Budget for all three.

**Nothing automated runs `anchor deploy` or `anchor upgrade`.** The commands get printed; the
owner deploys.
