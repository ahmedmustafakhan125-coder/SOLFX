# Phase 5 — LP Vault

**Status:** complete.
**Exit criteria (`ARCHITECTURE.md` § 15):** *"LP accounting exact under adversarial sequences;
I2, I8 hold."*

| Criterion | Result |
|---|---|
| LP accounting exact under adversarial sequences | ✅ 20 tests, invariants after every step |
| I2 (`LpVault.amount == LpPool.aum`) | ✅ asserted after every instruction |
| I8 (`supply > 0 ⟺ aum > 0`) | ✅ added this phase — and it caught a real defect |

**422 tests** (230 math, 192 program). `remove_liquidity` costs 26,890 CU.

---

## What was built

| Instruction | Who | What |
|---|---|---|
| `request_remove_liquidity` | provider | Starts the cooldown. Records a **time**, never a price. |
| `cancel_remove_liquidity` | provider | Abandons it. No penalty. |
| `remove_liquidity` | provider | Settles at today's NAV, minus both fees. |
| `withdraw_treasury_fees` | admin | Sweeps `fee_vault` — and nothing else. |

Plus `solfx-math::lp` (share pricing, NAV, both fees, 8 property tests) and the three
§ 7.2 / § 7.4 vault breakers that Phase 4 deferred because they are pool-relative.

---

## The cooldown is the defence. The exit fee is not.

Threat T6 is the just-in-time attack: deposit ahead of a trader loss the pool is about to
collect, withdraw ahead of a gain it is about to pay, capture the pool's edge without ever
carrying its risk. Every LP who *did* carry it is diluted.

**0.05% does not stop that.** A large enough known move outruns it trivially.

What stops it is that **redemption is priced at NAV when it settles, not when it was
requested.** The attacker must hold through the whole cooldown, exposed to everything that
happens in it — which is precisely the risk the attack was designed to avoid. It converts a
free option into a real position.

Everything else follows. `LpWithdrawRequest` records an unlock timestamp and no price; pricing
a request at request time would hand back exactly the option the cooldown removes.

Two tests attempt the attack from both directions:
`the_jit_attack_cannot_capture_a_gain_without_carrying_risk` and
`an_lp_cannot_exit_ahead_of_a_loss_the_pool_is_about_to_take`. The first is worth reading for
what it does *not* assert: the attacker still earns a share of the gain, because they
genuinely were an LP while it settled. The defence is not that they earn nothing — it is that
they were exposed for a day to earn it.

---

## Two defects the tests found

### 1. I8 caught money nobody could ever claim

When the last LP redeemed, the exit fee stayed in the pool — and the pool now had **zero
shares outstanding**. USDC sitting against no claim, permanently unreachable.

`assert_i8` (`supply > 0 ⟺ aum > 0`) caught it on the first run of the very first test. The
fee is now waived when a redemption empties the pool: it exists to compensate the LPs who
stayed, and there are none.

Not a JIT hole — escaping it requires being the only provider left, and the cooldown is the
defence anyway.

### 2. The skew cap deadlocked an empty market

§ 7.4 blocks opens on the heavy side once
`|oi_long − oi_short| / (oi_long + oi_short) > 0.6`. Taken literally, **the first position in
any market is refused**: one position is 100% one-sided by definition, so a book can never
form.

The same shape as the Phase 2 deviation-gate deadlock — a control that is correct in steady
state and impossible at the boundary.

The ratio is also the wrong question on a small book. What threatens the vault is the
*absolute* directional exposure it carries against its capital: a 100% skew on $100 of open
interest is noise; a 60% skew on $10M is not. So the cap now requires **both** § 7.4's ratio
and an imbalance worth at least 10% of AUM.

---

## Two deliberate deviations from § 8.1

Both are documented at the definition, not just here.

### The exit fee stays in the pool

§ 8.1 lists it as treasury revenue stream 5. It goes to the remaining LPs instead.

The fee's stated purpose is anti-JIT. Paying it to the treasury would mean the protocol
profits from LP churn while the LPs who carried the risk get nothing — and it is *their*
returns the attacker dilutes. Routing it to them aligns the defence with the party being
defended. Cost: ~1% of projected revenue, by § 8.1's own estimate.

### One global high-water mark, not per-LP

A per-LP mark needs per-LP state and an account per provider. One pool-level mark is the
standard vault model and has one consequence worth stating plainly:

> An LP who joins after a drawdown pays no performance fee until the pool recovers its
> previous peak — even on gains that are entirely theirs.

That errs toward the LP, which is the right direction for a fee the protocol charges itself.
The opposite arrangement — billing a recovery that only restores an earlier LP's losses —
would be indefensible. Asserted in
`an_lp_who_joined_after_a_drawdown_pays_nothing_until_the_peak_returns` so it cannot change
silently.

---

## The vault breakers (§ 7.2, § 7.4)

All three are pool-relative, which is why they arrived now rather than in Phase 4: each is a
statement about whether the vault can still stand behind a trade, and the vault's own
instructions did not exist until this phase.

| Breaker | Rule | Notes |
|---|---|---|
| Insurance floor | Below 25% of target, no new positions | Makes the § 6.9 waterfall honest — without it the protocol opens positions it has no reserve behind |
| Skew cap | § 7.4's 0.6 ratio **and** ≥10% of AUM imbalance | Trades that *reduce* skew are never blocked |
| Utilisation | Total notional ≤ a configured multiple of AUM | Measured on notional, not § 7.4's "max loss exposure", which cannot be computed without walking every position |

---

## Deferred, with reasons

| Deferred | To | Why |
|---|---|---|
| Trigger orders (TP/SL) | Phase 7 | Needs its off-chain executor; shipping the on-chain half means orders nothing fires. |
| Admin instructions for the breaker thresholds and funding `k` | Phase 8 | The fields exist and are validated. An instruction to tune them belongs with the frontend that would use it; tests set them directly. |
| Per-user concentration cap (§ 7.4) | Phase 9 | Needs per-user-per-market OI state, which no current account carries. |
| Single-block LP loss > 3% breaker (§ 7.2) | Phase 9 | Needs a per-block baseline the program does not keep. Belongs with monitoring rather than in an instruction. |

---

## Next: Phase 6 — IB programme

`solfx-referral`: immutable binding, per-trade accrual, permissionless claim, tier boundaries.

Exit criteria: *"Rebates accrue and claim correctly; every rebate emits a verifiable event;
tier boundaries tested."*

The groundwork is already in place — `UserAccount::referrer` is written once at creation and
no instruction anywhere changes it, and the referral share is already split off at collection
by `fees::split_fee`. Phase 6 is a **separate program** (§ 5.1) so it can iterate without
touching the financial core, and § 8.5 is blunt about why it matters:

> The smart contract is replicable in a few months by anyone. A network of IBs who trust your
> ledger because they can audit it is not.
