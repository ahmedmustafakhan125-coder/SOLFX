# Phase 6 Guide — the IB Programme (`solfx-referral`)

**Flowchart:** [`../diagrams/phase-6.png`](../diagrams/phase-6.png)
**Deliverable:** a **second on-chain program** holding the introducing-broker ledger, plus
the `solfx-math::referral` tier arithmetic and the two counters in core that feed it.
**Exit criteria:** *"Rebates accrue and claim correctly; every rebate emits a verifiable
event; tier boundaries tested."* All met — 20 integration tests running **both programs
together**, plus 9 unit tests on the tier maths.

---

## 1. Why this phase is the moat

§ 8.5 is direct about it: XM and Exness did not win on spreads or platform quality. They won
on **introducing-broker networks** — affiliates paid a per-lot rebate to bring in traders.
And every IB in that industry has the same complaint:

> **The broker controls the ledger.** Rebates get miscounted, clients get reassigned, payouts
> get delayed or clawed back, and the IB has no way to verify any of it.

*"The smart contract is replicable in a few months by anyone. A network of IBs who trust your
ledger because they can audit it is not."*

Trust is not a feature you can assert, so the design reduces it to three structural
properties, each of which a legacy broker **cannot** offer:

| Complaint | Structural fix | Test |
|---|---|---|
| Clients get reassigned | `UserAccount.referrer` is written once at account creation, and no instruction in *either* program changes it | `an_ib_cannot_claim_a_trader_who_named_someone_else` |
| Rebates get miscounted | Accrual is **derived** from a public monotonic counter, not asserted by us — anyone can recompute it | `a_referred_trade_accrues_a_rebate` |
| Payouts get delayed | `claim` is permissionless: no approval, no schedule, no counterparty | `an_ib_claims_permissionlessly_through_the_cpi` |

---

## 2. The architecture: the dependency points one way

§ 5.1 requires a separate program so the growth engine can iterate while the financial core
stays frozen for audit. **That only works if core does not depend on it.**

```
solfx-core                                   solfx-referral
──────────                                   ──────────────
UserAccount.referral_fees_generated  ◄─read─  sync_trader   (owner-checked by Anchor)
UserAccount.lifetime_volume          ◄─read─
Protocol.referral_authority (a pubkey)
pay_referral(amount)                 ◄─CPI──  claim         (signed by the config PDA)
```

Core has **no tier table, no IB accounts, and no idea what a rebate is.** It holds the money
and honours a signature, subject to one arithmetic constraint. The referral program owns
100% of the policy and can be rewritten, re-audited or replaced without reopening the core.

### What it costs the trading path: +180 CU (+0.3%)

Recording a rebate is two `checked_add`s on `UserAccount` — an account the instruction had
already loaded. **No CPI, no extra account, no new signer.** `open_position` for a referred
trader costs 59,886 CU against 59,705 unreferred.

That was the design constraint, not a happy accident: a growth engine that taxes every trade
is a growth engine that eventually gets switched off. It is also why accrual is *pull-based*
(the referral program reads a counter) rather than *push-based* (core calls out on every
trade).

---

## 3. `solfx-math::referral` — the tier arithmetic

### `IbTier::for_volume(thirty_day_volume)`

§ 8.5's table, with boundaries **inclusive at the bottom of the higher tier** — exactly $5M
is Silver, not Bronze. Stated explicitly and pinned by a test, because *a boundary that moves
under an IB is precisely the dispute this whole design exists to prevent.*

| Tier | Referred 30-day volume | Share of fees |
|---|---|---:|
| Bronze | < $5M | 8% |
| Silver | $5M – $25M | 10% |
| Gold | $25M – $100M | 13% |
| Diamond | > $100M | 16% |

### `entitlement(pool_share, pool_split_bps, tier)` — and a spec inconsistency, reconciled

**§ 8.3 sets the referral pool at 10% of fees. § 8.5 sets tiers at 8/10/13/16% of fees.
Those two numbers do not both hold** — a Diamond IB cannot be paid 16% out of a pool holding
10%.

The honest resolution:

```
fees_generated = pool_share × 10_000 / pool_split_bps
entitlement    = min( fees_generated × tier_bps / 10_000 ,  pool_share )
```

The IB is paid their tier's share of fees, **capped at what the pool actually holds**, and
the remainder sweeps to the treasury exactly as § 8.3 describes ("unclaimed portion sweeps
to treasury"). At the launch split:

| Tier | Wants | Gets (10% pool) | Sweeps |
|---|---|---|---|
| Bronze | 8% of fees | 8% (80% of pool) | 20% of pool |
| Silver | 10% | 10% (whole pool) | — |
| Gold | 13% | **capped at 10%** | — |
| Diamond | 16% | **capped at 10%** | — |

Funding the top tiers in full means widening `fee_split_referral_bps`, which is already an
admin-settable parameter — so it becomes a deliberate configuration decision made *when a
Gold IB actually exists*, rather than a promise written into the code before one does.
`sync_split` re-reads the split so a widened pool takes effect without an upgrade.

Rounds **down**: the protocol never over-pays a rebate on a rounding boundary, and the
difference stays in the pool for the next claim.

### `parent_override(child_earned, override_bps)` — the two-level network

§ 8.5 asks for sub-IBs because that is what existing FX affiliate networks expect, and
matching their mental model lowers switching cost. The parent earns **20% of what the child
earned** — *not* a second bite of the trader's fees — and it is capped at the budget left
after the child's cut, so **a two-level chain can never cost more than a one-level one**
(`a_two_level_chain_stays_inside_the_referral_pool`).

---

## 4. The program, instruction by instruction

### State

- **`ReferralConfig`** (`["referral_config"]`) — admin, the core `Protocol` it draws from,
  `override_bps`, a mirror of core's `pool_split_bps`, running totals. **This PDA is the
  authority core is told to trust**, and the one that signs claims.
- **`IbAccount`** (`["ib", authority]`) — `parent` (immutable, like a trader's referrer),
  `unclaimed`, lifetime earned/claimed, `referred_volume` (drives the tier),
  `referred_pool_share`.
- **`TraderLink`** (`["link", ib, user_account]`) — the **watermark**: how much of that
  trader's lifetime contribution has already been credited. This one account is what makes
  syncing idempotent.

### `initialize_referral` + core's `set_referral_authority`

**Two steps, by two different authorities.** The referral admin creates the config; core's
admin separately decides to trust its PDA. Neither can do the other's half, which is what
keeps a deployed-but-unregistered referral program completely harmless. Core launches with
`referral_authority == Pubkey::default()` — payouts disabled.

### `register_ib(parent)`

Permissionless — anyone may become an IB. Earning anything still requires a trader to have
*named* them at account creation, so open registration costs nothing. Self-recruitment is
refused. The parent link is written once and is immutable thereafter, for the same reason the
trader binding is: a network whose parent links could be edited would have exactly the trust
problem this design exists to remove.

### `sync_trader` — permissionless and idempotent

The heart of the phase.

```
delta_pool   = user.referral_fees_generated − link.credited_pool_share
delta_volume = user.lifetime_volume         − link.credited_volume
require(delta > 0)

ib.referred_volume += delta_volume        ← volume first, so an IB who crosses a boundary
tier                = IbTier::for_volume(ib.referred_volume)   with THIS sync is paid at the
earned              = entitlement(delta_pool, pool_split, tier) new tier for it
override            = min(20% × earned, delta_pool − earned)  → parent, if supplied

link.credited_* = user.*                  ← advance the watermark
```

Properties that matter:

- **Anyone may call it.** The IB, the trader, a keeper, a stranger. Nothing about the ledger
  depends on us being online — that is what removes the payout schedule an IB would otherwise
  have to trust.
- **It cannot double-credit.** The watermark means a repeat call credits nothing
  (`Nothing new to sync`), and never calling loses nothing, because core's counters keep
  accumulating.
- **The counters cannot be forged.** `UserAccount` is read as
  `Account<'info, solfx_core::state::UserAccount>`, so Anchor verifies the owner is
  `solfx-core`.
- **Promotion is not retroactive.** An IB crossing into Gold is paid Gold on what they sync
  afterwards. Stated in the code and pinned by a test, because silently recomputing history
  at each boundary is both unaffordable on chain and the kind of surprise that starts
  disputes.
- **A missing parent account does not block the child.** The override is simply not paid and
  stays in the pool — requiring the account would let an absent parent hold a sub-broker's
  own rebate hostage, which is the wrong failure.

### `claim` → CPI into `solfx_core::pay_referral`

No approval, no schedule, no counterparty. The config PDA signs; core checks two things and
nothing else:

1. the signer is the authority its admin registered, and
2. **`total_referral_claimed + amount ≤ total_referral_accrued`.**

That second constraint is what protects the treasury. The fee vault holds treasury money as
well as referral money, and nothing about a rebate entitles an IB to reach it. With the check
in place, **the worst a broken or compromised referral program can do is misallocate the
referral pool among IBs** — bad, visible in the event stream, and bounded by what referred
traders actually generated. The test
`a_claim_can_never_exceed_the_referral_pool` inflates an IB's balance to the whole fee vault
and proves core refuses it.

---

## 5. Verifiability — the event that makes the pitch true

`RebateAccrued` carries the **inputs as well as the result**: the watermark delta, the volume
delta, and the tier applied. An IB does not have to accept a number — they can recompute it
from public on-chain data and see exactly how it was reached. That, plus the immutable
binding, is the whole of what makes the ledger auditable rather than merely public.

**Test cases:** [`../test-cases/phase-6-tests.md`](../test-cases/phase-6-tests.md)
