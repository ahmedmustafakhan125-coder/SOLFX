# Phase 7 Guide — Keepers

**Flowchart:** [`../diagrams/phase-7.png`](../diagrams/phase-7.png)
**Deliverable:** trigger orders on chain (take-profit / stop-loss), and
[`crates/solfx-keeper`](../../crates/solfx-keeper) — the off-chain process that fires them,
liquidates, and cranks.
**Exit criteria:** *"Sub-2s liquidations under load; survives an instance being killed;
verified firing during a live weekend."*

---

## 1. What this phase is actually for

Every phase before this one made a dangerous action **possible for anyone to perform**.
Liquidation is permissionless. Trigger execution is permissionless. The cranks are
permissionless. That is the safety property the whole design rests on: no privileged operator
stands between a trader and the closing of their position.

But *possible* is not *happens*.

> **A permissionless instruction nobody is paid to call is a promise with no mechanism behind
> it.**

Phase 7 is the mechanism. It is the phase where the protocol stops being a set of correct
rules and starts being a system that runs.

---

## 2. The one design decision worth reading

The keeper **does not reimplement the risk engine**. It calls `solfx_core::risk::assess` and
`solfx_core::oracle::load_price_for_liquidation` — the same functions, compiled from the same
source, that the program runs on chain.

This is not a convenience. Consider the alternative: a keeper with its own margin maths is a
**second, unaudited risk engine**. The two only have to disagree by a single rounding step for
one of two things to happen:

| Divergence | Consequence |
|---|---|
| Keeper thinks liquidatable, chain disagrees | Wasted transactions, all rejected. Annoying, visible, cheap. |
| Chain thinks liquidatable, keeper disagrees | **The keeper sits quietly on a position that should have been closed.** Invisible until it becomes a bad debt. |

The second is the one that costs money, and it is the one that is hardest to notice — nothing
errors, no dashboard turns red, the keeper reports itself healthy. Sharing the code makes it
**unrepresentable**. The keeper's answer can differ from the chain's only where its *inputs*
differ, which is a staleness question with a bounded, measurable answer, rather than a logic
question with an unbounded one.

What the keeper does **not** share is authority. Every condition is re-checked on chain. A
mirror that has drifted costs one rejected transaction and never a wrong outcome.

That property is also what makes `processed` commitment safe. A liquidator waiting for
`confirmed` reacts to a book two blocks stale, which during a fast move is the difference
between a penalty and a bad debt. Being optimistic costs a failed transaction; being slow
costs the pool.

---

## 3. Trigger orders (on chain)

### 3.1 Why they waited a phase

The account and instructions were ready in Phase 4. They were deliberately held back:

> **An order that nothing fires is worse than no order at all.** A trader who places a
> stop-loss and believes they are protected, on a protocol with no executor running, is worse
> off than one who knows they must watch the position themselves.

So the account, the instructions, and the keeper that fires them landed together.

### 3.2 The four cases

`TriggerKind::is_met` covers four combinations, written out rather than folded into a sign
trick:

| Kind | Direction | Fires when |
|---|---|---|
| TakeProfit | Long | `price >= trigger` |
| TakeProfit | Short | `price <= trigger` |
| StopLoss | Long | `price <= trigger` |
| StopLoss | Short | `price >= trigger` |

A stop-loss wired backwards would close winning positions and hold losing ones — and would
read to a trader as a market-conditions complaint, not a bug. That is why all four are tested
in both directions (fires when it should, *and* refuses when it should not).

Comparisons are **inclusive**: a trader who sets a stop at 1.0800 expects it to fire at
1.0800.

### 3.3 Placement refuses an already-met trigger

A take-profit *below* a long is already met. It would fire on the next keeper pass — that is
not an order, it is a delayed market close with extra steps. `TriggerAlreadyMet` says so at
placement rather than surprising the trader a second later.

### 3.4 The tip is the account's rent

This is the part worth understanding, because it replaced a worse design mid-phase.

The first version paid the keeper **$0.25 in USDC** from the trader's free collateral. It
worked, and it was wrong in two ways at once:

1. It needed a keeper token account and a vault transfer, which pushed `ExecuteTriggerOrder`
   to 18 accounts — **past the BPF 4KB stack frame**, which the tests caught as
   `Access violation in stack frame 5`.
2. It moved USDC, which meant a new term in invariant I7 and a new way for the accounting to
   be wrong.

The replacement: `close = keeper`. The order account's **rent** is the fee.

| Property | Why it matters |
|---|---|
| The trader pre-funded it at placement | The party who wanted the order pays for it |
| The protocol pays nothing | No subsidy, no budget to run out |
| No token account | Two fewer accounts — back inside the stack frame |
| No USDC moves | **Invariant I7 is untouched** |
| ~0.0016 SOL | Comfortably above the transaction cost |

When the trigger closes the position entirely, the *position* account's rent goes to the
keeper too, bringing the total to roughly 0.0037 SOL. The trader would have reclaimed that on
a manual close, so an automated close costs them about that much — which is the price of not
having to be awake, and what makes a stranger's bot worth running.

### 3.5 A trigger is not a promised price

The condition is checked against the **oracle** price. The close then fills at the *execution*
price, which crosses the adverse spread and, in a gap, may be far past the trigger.

Every broker works this way. The UI must say so plainly, because the alternative is a trader
who believes they were promised a price and discovers otherwise during the one event where it
matters. `a_gap_fills_far_past_the_trigger_and_says_so` pins this behaviour so it cannot
quietly change.

---

## 4. The keeper (off chain)

One process, four loops, sharing one mirrored book.

### 4.1 The refresher — two speeds

| Loop | Interval | Reads |
|---|---|---|
| Slow | 30s | Everything: markets, positions, trigger orders, user authorities |
| Fast | 400ms | Oracle accounts and the clock only |

The split exists because prices move every few hundred milliseconds and the set of open
positions does not. **The fast loop sets the liquidation latency floor**, so the "sub-2s"
target is really a statement about `scan_ms` plus one block.

The slow loop is a **full re-read**, never an incremental patch. Incremental state that can
silently miss an update is exactly how a keeper ends up not liquidating a position it believes
it already closed. The whole book is small enough that correctness is worth far more than the
saved bandwidth.

### 4.2 The danger list — sorted, not scanned

The naive keeper iterates positions and fires at each liquidatable one. It works until the day
it matters.

In a real cascade — the CHF depeg of Phase 4's replay, or any Sunday-open gap — hundreds of
positions cross the line in the same second. Blockspace during that minute is scarce and
expensive, so a keeper firing in account-map order is submitting transactions in an order
**uncorrelated with how much money is at stake**. Which ones land is effectively random.

Ranking by *notional at risk* means the ones that land first are the ones whose failure would
cost the pool most.

Below the line sits the **near-miss band** (health factor under 1.5 by default). It exists for
a different reason: an operator needs to see pressure *building*, not only the liquidations
that resulted. A run of positions sitting at 1.1× maintenance for an hour is the signal that
the next gap will be expensive — and it is invisible if the keeper only logs what it fired.

### 4.3 The action cap

Eight actions per pass. **A cap, not a target.** Without it a cascade becomes an unbounded
burst of transactions from one signer, which the RPC rate-limits and the leader drops — so the
keeper lands *fewer* liquidations than if it had paced itself. The next pass is 400ms away.

### 4.4 The crank order

Session → price → funding. Session decides whether the market is open, and the other two
behave differently on either side of that. Cranking price first would mark the market with the
*previous* session's parameters.

### 4.5 The feed watchdog

Read-only. Sends nothing. It compares each on-chain price account against Hermes and reports
three different things that look identical from inside the protocol:

| Signal | Meaning | Is it a reason to stop trading? |
|---|---|---|
| **Lag** | The sponsored publisher has stopped | Yes — and § 7.2 already does |
| **Divergence** | The feed is *wrong* | Yes, but nothing on chain can detect it |
| **Confidence** | The market is genuinely uncertain | Yes, and correctly |

The middle one is the reason this loop exists. A feed can advance its timestamp while
publishing a price that has drifted from the market — no staleness check catches that, and a
single oracle cannot tell that it is the one that is wrong. Hermes is the only outside opinion
available.

The alarm threshold is 50bps, deliberately generous: the two are sampled at different
instants, so a fast market produces a real difference that is nobody's fault. Alarming on
sampling jitter would make the alarm worthless.

### 4.6 Going quiet

If the book is more than 20 seconds stale, every service stands down for that pass.

A keeper whose RPC has died still holds a plausible-looking snapshot and will happily keep
deciding from it. Going quiet is correct: the protocol is permissionless, so *someone else's*
keeper is still running — whereas transactions built from a ten-minute-old book are wrong in a
way that costs money.

---

## 5. Why the keeper posts no prices

Pyth's pull model is usually described as "the caller posts the price". That is true of a
trader placing an order, and it is the wrong model for a keeper.

Every SolFX feed is a Pyth **price feed account**: a PDA of the push-oracle program
(`pythWSnsw…`), seeded `[shard_id_le, feed_id]`, kept current by Pyth's own publishers and
readable by anyone. So the keeper only has to **reference** the account.

Two consequences:

**The address is derived, never configured.** Listing a market must not also mean editing
every keeper's address book — a keeper that missed the edit goes quiet on exactly the market
nobody is watching yet.

**The transaction fits in one packet.** Measured on the widest market the protocol can produce
(a synthetic pair *and* non-USD-quoted — three oracle legs):

| Instruction | Size | Limit |
|---|---:|---:|
| `liquidate_position` | 758 bytes | 1232 |
| `execute_trigger_order` | 661 bytes | 1232 |
| `crank_market_price` | 395 bytes | 1232 |
| `crank_market_session` | 331 bytes | 1232 |
| `crank_funding` | 296 bytes | 1232 |

This is a hard requirement, not a nice-to-have. A transaction that does not fit must be split,
and splitting a liquidation means holding intermediate state between two transactions that may
land in different blocks. During the congestion spike that made the liquidation necessary —
the only time it matters — the second half is exactly the one that fails to land.

A signed Wormhole update would not fit alongside seventeen accounts. That is the whole reason
this design works.

---

## 6. Compute budgets

| Instruction | Measured | Ceiling |
|---|---:|---:|
| `place_trigger_order` | 21,838 CU | 60,000 |
| `execute_trigger_order` | 56,713 CU | 200,000 |

`execute_trigger_order` costs slightly more than a voluntary close because it also validates
and closes the order account. It sits comfortably under § 5.5's 200k requirement for
keeper-path instructions.

---

## 7. Running it

```bash
# 1. Deploy. Clones Pyth's programs from devnet — a bare validator has no Pyth on it,
#    so without this the whole protocol is unreachable.
scripts/deploy-localnet.sh

# 2. Initialise the protocol and list markets (see docs/DEPLOY.md — these are
#    deployment decisions, not something a script should invent).

# 3. Start the keepers.
scripts/run-keeper.sh --reward-token-account <the keeper's USDC account>

# Or watch what it would do without sending anything:
scripts/run-keeper.sh --dry-run
```

Services can be selected individually with `--service liquidator --service triggers
--service cranks --service watchdog`. They are separate services rather than one loop because
their failure modes are not alike: a liquidator that stops is an emergency, a trigger executor
that stops annoys traders, and a funding crank that stops for a minute costs nothing. An
operator should be able to alert on the first without being woken by the third.

---

## 8. What is honestly still open

Three things, recorded rather than glossed:

**The cranks are unpaid.** `crank_market_price`, `crank_funding` and `crank_market_session`
are permissionless but nothing compensates the caller, so in practice the operator runs them.
It is safe — none can be called profitably or harmfully by a stranger — but *"the operator
must run this"* is a weaker promise than *"anyone is paid to"*, and this phase should not
pretend otherwise. Adding a tip on the same rent-based model as triggers is the obvious fix
and is not yet done.

**"Verified firing during a live weekend" is not met.** That criterion requires wall-clock
time against a real cluster across a real Friday close and Sunday open. Everything it depends
on is tested — session transitions, gap windows, weekend parameters, stale-feed refusal — but
the criterion itself is a devnet observation, not a test, and it has not been performed.

**"Sub-2s liquidations under load" is argued, not measured.** The latency budget is `scan_ms`
(400ms) plus block time, and the transaction-size and compute-unit bounds that make it
achievable are pinned by tests. But an end-to-end measurement under real load on devnet has
not been done.

Both open criteria are devnet work, which is the next step regardless.
