# Phase 6 — IB Programme (`solfx-referral`)

**Status:** complete.
**Exit criteria (`ARCHITECTURE.md` § 15):** *"Rebates accrue and claim correctly; every
rebate emits a verifiable event; tier boundaries tested."*

| Criterion | Result |
|---|---|
| Rebates accrue correctly | ✅ derived from a public monotonic counter, idempotent |
| Rebates claim correctly | ✅ permissionless, via CPI, bounded by the pool |
| Every rebate emits a verifiable event | ✅ `RebateAccrued` carries the **inputs**, not just the result |
| Tier boundaries tested | ✅ pinned exactly, unit and integration |

**454 tests** (203 math, 251 program). Two programs: `solfx_core.so` 905 KB,
`solfx_referral.so` 257 KB.

---

## The architectural decision that shaped everything

§ 5.1 wants the referral programme separate so the growth engine can iterate while the
financial core freezes for audit. **That is only true if core does not depend on it.**

So the accrual is **pull-based**. Core records two counters on `UserAccount` — an account
every trading instruction already loads — and `solfx-referral` reads them and derives what it
owes. No CPI from core, no extra account on any trading instruction, no new signer.

The measured cost of the entire IB ledger on the trading path is **+180 CU (+0.3%)**:
`open_position` costs 59,886 CU for a referred trader against 59,705 for an unreferred one.

That was the constraint, not a bonus. A growth engine that taxes every trade is a growth
engine that eventually gets switched off — and the push-based alternative (core CPIs into
referral on every settlement) would have added 3–4 accounts to five instructions that already
carry 16, on a transaction that also has to fit up to three Pyth price updates inside 1232
bytes.

---

## A spec inconsistency, reconciled in the open

§ 8.3 sets the referral pool at **10% of fees**. § 8.5 sets IB tiers at **8 / 10 / 13 / 16%
of fees**. Both cannot hold: a Diamond IB cannot be paid 16% out of a pool holding 10%.

`entitlement()` resolves it by paying the tier's share **capped at what the pool holds**,
with the remainder sweeping to the treasury exactly as § 8.3 describes. At the launch split
Bronze and Silver are paid in full; Gold and Diamond are capped at 10%.

Paying the top tiers in full means widening `fee_split_referral_bps` — already an
admin-settable parameter, and `sync_split` re-reads it without an upgrade. So it becomes a
deliberate decision made **when a Gold IB actually exists**, rather than a promise written
into the code before one does.

---

## The three properties the tests are really checking

§ 8.5's claim is that the moat is *a network of IBs who trust your ledger because they can
audit it*. Trust is not assertable, so it decomposes into three structural facts:

| Every IB's complaint | Why it cannot happen here |
|---|---|
| "The broker reassigned my clients" | `UserAccount.referrer` is written once at account creation and **no instruction in either program changes it**. An IB syncing a trader who named someone else is refused by name. |
| "My rebates were miscounted" | Accrual is *derived* from `referral_fees_generated`, a public monotonic counter, behind a `TraderLink` watermark. A stranger can perform the sync; a repeat credits nothing; nothing is ever lost by not syncing. |
| "My payout was delayed" | `claim` has no approval step and no schedule. |

And `RebateAccrued` publishes the **inputs** — watermark delta, volume delta, tier applied —
so an IB recomputes the figure from public data rather than accepting a number.

---

## The constraint that protects the treasury

The fee vault holds treasury money as well as referral money. Core's `pay_referral` enforces:

```
total_referral_claimed + amount <= total_referral_accrued
```

Without it, a bug — or a compromised referral program — could drain the treasury through a
door built for rebates. With it, **the worst a broken referral programme can do is
misallocate the referral pool among IBs**: bad, visible in the event stream, and bounded by
what referred traders actually generated. `a_claim_can_never_exceed_the_referral_pool`
inflates an IB's balance to the entire fee vault and proves core refuses it.

Registration is also deliberately two steps by two authorities: the referral admin creates
the config, core's admin separately decides to trust its PDA. A deployed-but-unregistered
referral program is completely harmless.

---

## One correction to an earlier phase

A Phase 3 test asserted `total_referral_accrued > 0` after a busy session — but its traders
had no referrer. Core now correctly earmarks the referral share **only when a referrer
exists**; otherwise it sweeps to the treasury (§ 8.3). The assertion was inverted to check
the right thing: *an unreferred trader must not accrue a rebate for nobody.*

Invariant **I7** also gained a referral-claims term, alongside liquidator rewards and
treasury sweeps — every route USDC takes out of the protocol now has one.

---

## Deferred, with reasons

| Deferred | To | Why |
|---|---|---|
| 30-day rolling volume window | Phase 8 | Tiers currently use *lifetime* referred volume. A rolling window needs either a cranked decay or per-epoch buckets; the frontend that displays a tier is the right place to decide which. Lifetime volume is monotonic, so no IB is ever demoted by the interim. |
| `referred_trader_count` maintenance | Phase 8 | The field exists; incrementing it needs first-sync detection, which is display-only information. |
| IB dashboard, referral links | Phase 8 | Frontend work; the on-chain ledger is complete. |

---

## Next: Phase 7 — Keepers

The off-chain machinery: liquidator (< 2 s from HF < 1 to landed), trigger executor **plus
the on-chain TP/SL orders** deferred from Phase 4, funding/session crankers, three
independent instances, monitoring. It also settles the last oracle unknown — transaction
*size* with real Hermes payloads, which compute measurements cannot answer.

**Risk register R13 becomes pressing here:** keeper operations need a second engineer.
