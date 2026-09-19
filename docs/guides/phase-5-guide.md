# Phase 5 Guide — the LP Vault

**Flowchart:** [`../diagrams/phase-5.dot`](../diagrams/phase-5.dot)
**Deliverable:** the withdrawal path with its T6 defences, the LP fee pair, the pool-relative
circuit breakers, and treasury sweeps. Plus `solfx-math::lp`, where the share arithmetic
lives under property tests.
**Exit criteria:** *"LP accounting exact under adversarial sequences; I2, I8 hold."* Met —
and I8, added this phase, caught a real defect on its first run.

---

## 1. `solfx-math::lp` — the share arithmetic

**The one property everything serves:** a deposit or withdrawal must never move NAV per
share in the *actor's* favour. The failure mode is not a crash — it is a slow leak where
LPs quietly earn less than they should, which is exactly the kind of bug that survives to
production. Hence property tests over the whole parameter space, not spot values.

| Function | Formula | Rounds | Favours |
|---|---|---|---|
| `nav_per_share(aum, supply)` | `aum × 1e6 / supply` (empty pool ≡ $1.00 par) | down | — |
| `shares_for_deposit(amount, aum, supply)` | `amount × supply / aum` (1:1 into an empty pool) | **down** | existing LPs |
| `usdc_for_shares(shares, aum, supply)` | `shares × aum / supply` | **down** | remaining LPs |
| `exit_fee(gross, bps)` | `gross × bps / 1e4` | **up** | remaining LPs |
| `performance_fee(shares, nav, hwm, bps)` | `bps × shares × max(nav − hwm, 0) / 1e6 / 1e4` | **up** | treasury |

Same rule as `fixed.rs` for traders, applied to LPs: every rounding goes against the party
initiating the action.

Worked example — the pool doubled ($1,000 AUM, 500 shares → NAV $2.00): a $100 deposit
mints **50** shares, not 100; redeeming 50 shares returns $100. A latecomer pays the
going price; an early LP's gain cannot be diluted away. Redeeming the *entire* supply
returns the *entire* AUM — no dust may be stranded where nobody can claim it (this pairs
with invariant I8 below).

Properties pinned: deposits never dilute the pool; withdrawals never dilute those who stay;
**deposit-then-immediately-redeem never profits** (the LP mirror of the trader round-trip
test — an LP pays no spread, so rounding is the only wall); splitting a redemption never
beats one redemption; a non-zero exit fee charges at least one unit and never exceeds the
payout; no input panics.

---

## 2. The exit path — and why the *cooldown* is the defence

**Threat T6 (just-in-time liquidity):** watch for a trade the pool is about to win, deposit
in front of it, withdraw the moment it settles — capture the pool's edge without carrying
its risk, diluting every LP who did. The mirror: request out ahead of a loss.

**The exit fee does not stop this.** 0.05% is trivially outrun by any decent known move.
What stops it: **redemption is priced at NAV when it *settles*, not when it was
requested.** The attacker must hold through the whole cooldown, exposed to everything in
it — precisely the risk the attack exists to avoid. It converts a free option into a real
position. Everything else follows from that one sentence:

### `request_remove_liquidity(shares)`
Creates `LpWithdrawRequest` (`["lp_withdraw", authority]`) recording **`unlock_at = now +
cooldown` and nothing else — a time, never a price** (a price snapshot would hand back
exactly the option the cooldown removes). One request per provider (a second would need a
second clock and allow rolling a matured request forward forever). Shares are *not*
escrowed: settlement burns from the provider's own account, so moving the tokens away
simply makes the request unsettleable — cancellation by construction, zero extra code.
Works while paused (queueing an exit takes no risk; a pause must not trap LP capital).

### `cancel_remove_liquidity`
Free, rent back. Staying in the pool is the outcome the cooldown *wants*; charging for a
change of mind would push LPs to complete exits they no longer want.

### `remove_liquidity(min_usdc_out)`
After `unlock_at` (exact to the second — the boundary test checks ±1 s):

```
gross           = shares × aum / supply                     (settle-time NAV, floored)
exit fee        = 0.05% of gross (ceil)  → STAYS IN THE POOL
performance fee = 10% × gain above the high-water mark      → TREASURY
payout          = gross − both fees    (must be ≥ min_usdc_out — the LP's slippage guard;
                                        a refused settlement leaves the request intact)
burn shares → pay USDC → ratchet the HWM if the pool sits at a new peak
```

**Deliberate deviation #1 — the exit fee stays in the pool** (§ 8.1 lists it as treasury
stream 5). It is an anti-JIT device and the LPs who stayed are the party being defended;
routing it to the treasury would profit the protocol from LP churn while the diluted party
gets nothing. Cost: ~1% of projected revenue, by § 8.1's own estimate.

**Deliberate deviation #2 — one global high-water mark, not per-LP.** Per-LP marks need
per-LP state. The consequence, stated plainly and pinned by a test: an LP who joins after a
drawdown pays no performance fee until the previous peak returns, even on gains entirely
theirs. That errs toward the LP — the right direction for a fee the protocol charges
itself; the reverse (billing a recovery that only restores an earlier LP's losses) would be
indefensible.

**What I8 caught (defect):** when the *last* LP redeemed, the exit fee stayed behind in a
pool with zero shares — USDC nobody could ever claim. `supply > 0 ⟺ aum > 0` failed on the
first run of the first test. Fix: the fee is waived when a redemption empties the pool
(there is nobody left to compensate). Not a JIT hole — escaping it requires being the last
provider standing, and the cooldown is the actual defence anyway.

---

## 3. The vault circuit breakers (§ 7.2 / § 7.4)

Deferred from Phase 4 because all three are statements about the *pool*, and the pool's own
instructions did not exist yet. All are checked in `open_position` on the resulting book,
all admin-tunable data, all zero-disabled.

### `check_utilisation` — the pool cannot back unlimited OI
`Σ(oi_long + oi_short) ≤ max_utilisation_bps × AUM`. Measured on **notional**, not § 7.4's
"max loss exposure" — max loss is unknowable without walking every position; notional is
the conservative figure an instruction can actually compute. An empty pool backs nothing
(also stops trading a market before anyone has provided liquidity).

### `check_skew_cap` — funding is too slow a lever at extremes
§ 7.4's rule (`|long − short| / (long + short) > 0.6` ⇒ heavy side closed) **plus an
absolute floor: the imbalance must exceed 10% of AUM**.

The floor exists because the raw ratio *deadlocks an empty market* — the first position is
100% one-sided by definition, so no book could ever form (defect; the same shape as
Phase 2's deviation-gate deadlock: correct in steady state, impossible at the boundary).
It is also the better question: what threatens the vault is absolute exposure against
capital — a 100% skew on $100 of OI is noise; 60% on $10M is not. Trades that *reduce*
skew are never blocked: a cap that stopped the correcting trade would entrench the
imbalance it exists to fix.

### `check_insurance_floor` — no reserve, no new risk
Below 25% of the fund's target, new positions are refused. This is what makes the § 6.9
waterfall honest: without it the protocol keeps writing risk it has no reserve behind, and
the first gap skips straight past insurance to ADL and the LPs — the two steps the fund
exists to shield.

---

## 4. `withdraw_treasury_fees` — and the non-custodial claim, by construction

Admin sweeps the **fee vault only**. There is no instruction anywhere in the program that
lets *any* key sign a transfer out of the collateral, LP or insurance vaults — the claim
"your money never leaves your account" is enforced by the absence of a code path, not by
policy. `Protocol.total_treasury_withdrawn` records the outflow so invariant I7 (every
USDC accounted for: trader in/out, LP in/out, insurance in, liquidator rewards out,
treasury out) still balances to the unit.

**Test cases:** [`../test-cases/phase-5-tests.md`](../test-cases/phase-5-tests.md)
