# Phase 7 — Keepers

**Status:** complete on chain and in code; **two exit criteria remain open pending devnet.**
**Exit criteria (`ARCHITECTURE.md` § 15):** *"Sub-2s liquidations under load; survives an
instance being killed; verified firing during a live weekend."*

| Criterion | Result |
|---|---|
| Survives an instance being killed | ✅ no in-flight state to drain; a restart rebuilds the whole view in one pass |
| Sub-2s liquidations under load | ⚠️ **argued and bounded, not measured.** The budget is 400ms + block time; the size and CU bounds that make it achievable are pinned by tests. No end-to-end measurement under load. |
| Verified firing during a live weekend | ❌ **not performed.** Needs wall-clock time across a real Friday close and Sunday open. |

**490 tests** (248 math, 231 core, 11 keeper), all green. `solfx_core.so` 980 KB,
`solfx_referral.so` 257 KB.

Both open criteria are devnet observations rather than things a test can assert, and devnet is
the next step regardless. They are listed here rather than quietly folded into "complete".

---

## What this phase is for

Every phase before this one made a dangerous action **possible for anyone to perform**.
Liquidation, trigger execution and the cranks are all permissionless. That is the safety
property the whole design rests on.

But *possible* is not *happens*:

> **A permissionless instruction nobody is paid to call is a promise with no mechanism behind
> it.**

Phase 7 is the mechanism.

---

## The decision that shaped the phase

**The keeper does not reimplement the risk engine.** It calls `solfx_core::risk::assess` and
`solfx_core::oracle::load_price_for_liquidation` — the same functions, from the same source,
that the program runs on chain.

The alternative is a second, unaudited risk engine. It only has to disagree by one rounding
step, and the two ways it can disagree are not equally survivable:

| Divergence | Consequence |
|---|---|
| Keeper thinks liquidatable, chain disagrees | Rejected transactions. Visible, cheap. |
| Chain thinks liquidatable, keeper disagrees | **The keeper sits quietly on a position that should have closed.** Nothing errors, no dashboard turns red, the keeper reports itself healthy — until it is a bad debt. |

Sharing the code makes the second one unrepresentable. The keeper's answer can differ from the
chain's only where its *inputs* differ, which is a staleness question with a bounded answer
rather than a logic question with an unbounded one.

The keeper shares the code but never the authority. Every condition is re-checked on chain, so
a drifted mirror costs one rejected transaction and never a wrong outcome. That is also what
makes `processed` commitment the right choice: waiting for `confirmed` means reacting to a
book two blocks stale, which during a fast move is the difference between a penalty and a bad
debt. Being optimistic costs a failed transaction; being slow costs the pool.

---

## A design that got replaced mid-phase

Trigger execution originally paid the keeper **$0.25 in USDC** out of the trader's free
collateral. It worked. It was wrong in two ways at once, and the tests found both:

1. It needed a keeper token account and a vault transfer, pushing `ExecuteTriggerOrder` to 18
   accounts — past the BPF 4KB stack frame. The failure surfaced as
   `Access violation in stack frame 5`, the same class of failure Phase 1 hit on
   `initialize_protocol`.
2. It moved USDC, which meant a new term in invariant I7 and a new way for the accounting to
   be wrong.

The replacement is `close = keeper`: **the order account's rent is the fee.**

| Property | Why it matters |
|---|---|
| The trader pre-funded it at placement | The party who wanted the order pays for it |
| The protocol pays nothing | No subsidy, no budget to run out |
| No token account | Two fewer accounts — back inside the stack frame |
| No USDC moves | **Invariant I7 is untouched** |
| ~0.0016 SOL | Comfortably above the transaction cost |

Worth naming as a pattern: the constraint (a stack frame) and the accounting concern (a new
I7 term) pointed at the same fix. The version that removed a mechanism was better than the
version that added one.

A second detail fell out of it. A trigger may be **partial**, so the position cannot use a
`close =` constraint — that fires unconditionally and would delete a position that still has
size. The position is closed conditionally in the handler instead. Anchor 1.1.2's
`exit_with_expected_owner` guards on `is_closed`, so this is the supported form rather than a
trick.

---

## Why trigger orders waited a phase

The account and instructions were ready in Phase 4 and were deliberately held back:

> **An order that nothing fires is worse than no order at all.** A trader who places a
> stop-loss and believes they are protected, on a protocol with no executor running, is worse
> off than one who knows they must watch the position themselves.

The order, the instructions and the keeper that fires them landed together.

---

## Two things the keeper does that a naive one would not

**It sorts rather than scans.** In a cascade — the CHF depeg of Phase 4's replay, or any
Sunday-open gap — hundreds of positions cross the line in one second and blockspace is scarce.
A keeper firing in account-map order is submitting in an order uncorrelated with how much money
is at stake, so which ones land is effectively random. Ranking by notional at risk means the
ones that land first are the ones whose failure would cost most.

**It reports near misses.** A run of positions sitting at 1.1× maintenance for an hour is the
signal that the next gap will be expensive — and it is invisible if the keeper only logs what
it fired.

There is also an action cap of eight per pass. **A cap, not a target:** an unbounded burst
from one signer gets rate-limited and dropped, so the keeper would land *fewer* liquidations
than if it paced itself. The next pass is 400ms away.

---

## The measurement behind the Pyth design

Pyth's pull model is usually described as "the caller posts the price". That is right for a
trader and wrong for a keeper. Every SolFX feed is a Pyth **price feed account** — a PDA of
the push-oracle program, kept current by Pyth's own publishers — so the keeper only has to
*reference* it.

Measured on the widest market the protocol can produce (synthetic **and** non-USD-quoted:
three distinct oracle legs), with the compute-budget instructions included because they are
not optional in production:

| Instruction | Size | Limit |
|---|---:|---:|
| `liquidate_position` | 758 bytes | 1232 |
| `execute_trigger_order` | 661 bytes | 1232 |
| `crank_market_price` | 395 bytes | 1232 |
| `crank_market_session` | 331 bytes | 1232 |
| `crank_funding` | 296 bytes | 1232 |

A transaction that does not fit must be split, and splitting a liquidation means holding
intermediate state between two transactions that may land in different blocks. During the
congestion spike that made the liquidation necessary — the only time it matters — the second
half is exactly the one that fails to land. A signed Wormhole update would not fit alongside
seventeen accounts, which is why this design works at all.

The address is also **derived** from `market.pyth_feed_id`, never configured. Listing a market
must not mean editing every keeper's address book, because a keeper that missed the edit goes
quiet on exactly the market nobody is watching yet.

---

## Compute budgets

| Instruction | Measured | Ceiling |
|---|---:|---:|
| `place_trigger_order` | 21,838 CU | 60,000 |
| `execute_trigger_order` | 56,713 CU | 200,000 |

`execute_trigger_order` is measured with a **stranger** firing it, since that is the call that
has to land in a fast market.

---

## The watchdog, and a gap it closes

§ 7.2 and correction C-1 make the protocol refuse to trade on a stale or wide feed. That is
correct, and from the outside it is **indistinguishable from the protocol being broken**:
traders see rejected orders and the operator sees no errors at all, because nothing errored.

The watchdog is read-only and sends nothing. It separates three things that look identical
from inside the protocol:

| Signal | Meaning | Detectable on chain? |
|---|---|---|
| Lag | The publisher has stopped | Yes — § 7.2 already refuses |
| **Divergence** | The feed is *wrong* | **No.** A single oracle cannot tell it is the one that is wrong. |
| Confidence | The market is genuinely uncertain | Yes, and correctly |

The middle row is why the loop exists. A feed can advance its timestamp while publishing a
drifted price; no staleness check catches that, and Hermes is the only outside opinion
available.

---

## What is honestly still open

**The cranks are unpaid.** `crank_market_price`, `crank_funding` and `crank_market_session`
are permissionless but nothing compensates the caller, so in practice the operator runs them.
It is safe — none can be called profitably or harmfully by a stranger — but *"the operator
must run this"* is a weaker promise than *"anyone is paid to"*. The rent-based tip that
triggers now use would apply directly and is not yet done.

**No bootstrap tooling.** `scripts/deploy-localnet.sh` deploys both programs and now clones
Pyth's programs from devnet (a bare validator has no Pyth on it, so without that the whole
protocol is unreachable), but it does **not** initialise the protocol or list markets. Those
need a USDC mint, an admin, feed ids and a full risk envelope — deployment decisions a script
should not invent. This is the next concrete blocker for devnet.

**Two exit criteria need devnet**, as recorded at the top.

---

## Files

| Path | What |
|---|---|
| `programs/solfx-core/src/state/trigger_order.rs` | `TriggerKind` (the four cases) and the `TriggerOrder` account |
| `programs/solfx-core/src/instructions/trader/trigger.rs` | place / cancel / execute |
| `programs/solfx-core/tests/triggers.rs` | 14 integration tests |
| `crates/solfx-keeper/src/book.rs` | the mirrored book, and why it shares the program's code |
| `crates/solfx-keeper/src/danger.rs` | the sorted danger list and near-miss band |
| `crates/solfx-keeper/src/services.rs` | liquidator, triggers, cranks, watchdog |
| `crates/solfx-keeper/src/pyth.rs` | feed-account derivation and the Hermes client |
| `scripts/run-keeper.sh` | starts the keepers against a cluster |
