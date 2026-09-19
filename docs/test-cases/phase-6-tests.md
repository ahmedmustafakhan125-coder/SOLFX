# Phase 6 — Test Cases (30 tests)

**Guide:** [`../guides/phase-6-guide.md`](../guides/phase-6-guide.md) · **Flowchart:** [`architecture/phase-6.png`](../../architecture/phase-6.png)

Phase 6's suite runs **both programs together** in LiteSVM — `solfx-referral` reading
core's real `UserAccount` and claiming through the real CPI. Mocking either side would test
the mock.

§ 8.5 says the moat is not the code but *a network of IBs who trust your ledger because they
can audit it*. Trust is not something a test can assert, so the suite checks the three
structural properties it is made of instead: **clients cannot be reassigned** (the binding is
written once), **nobody can be double-paid or short-changed** (accrual derives from a
monotonic public counter behind an idempotent watermark), and **nobody can be delayed**
(claims are permissionless).

The tier boundaries are pinned exactly, in both the unit and integration layers, because a
boundary that moves under an IB is the dispute the whole design exists to prevent.

**Run:**
```bash
anchor build   # builds BOTH programs
cargo test -p solfx-core --test referral
cargo test -p solfx-math referral
```

## unit: IB tier maths — `crates/solfx-math/src/referral.rs` (9)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `tiers_match_the_published_table` | Tiers match the published table. | — |
| 2 | `tier_boundaries_are_inclusive_at_the_bottom_of_the_higher_tier` | A boundary that moves under an IB is the dispute this design exists to prevent, so each one is pinned exactly. | — |
| 3 | `a_bronze_ib_is_paid_in_full_and_the_rest_sweeps` | Bronze at 8% of fees against a 10% pool: paid in full, 20% of the pool sweeps to treasury. | — |
| 4 | `a_silver_ib_takes_exactly_the_whole_pool` | A silver ib takes exactly the whole pool. | — |
| 5 | `the_top_tiers_are_capped_at_the_pool` | The cap. | — |
| 6 | `a_wider_pool_pays_the_top_tiers_in_full` | Raising the referral split is what actually funds the top tiers — a configuration decision, made when a Gold IB exists. | — |
| 7 | `the_parent_override_is_a_share_of_the_child_not_of_the_trader` | The parent override is a share of the child not of the trader. | — |
| 8 | `degenerate_inputs_return_zero_rather_than_erroring` | Degenerate inputs return zero rather than erroring. | — |
| 9 | `an_entitlement_never_exceeds_the_pool_share_at_any_tier` | An entitlement never exceeds the pool share at any tier. | — |

## compute-unit regression — `programs/solfx-core/tests/compute_budget.rs` (1)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `referral_instructions_stay_within_their_ceilings` | The IB programme (ARCHITECTURE.md § 8.5), including the cross-program claim. | — |

## integration: IB programme — `programs/solfx-core/tests/referral.rs` (20)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `the_referral_programme_registers_with_core_in_two_steps` | Registration takes two steps by two authorities: the referral admin creates the config, then core's admin separately decides to trust its PDA. | — |
| 2 | `anyone_can_register_as_an_introducing_broker` | Anyone can register as an introducing broker. | — |
| 3 | `an_ib_cannot_recruit_themselves` | An ib cannot recruit themselves. | `cannot recruit themselves` |
| 4 | `a_referred_trade_accrues_a_rebate` | The happy path: a referred trader trades, the IB syncs, the rebate is there. | — |
| 5 | `syncing_is_permissionless_and_cannot_double_credit` | The property that makes the ledger trustworthy. | — |
| 6 | `an_ib_cannot_claim_a_trader_who_named_someone_else` | Clients cannot be reassigned. | — |
| 7 | `an_unreferred_trader_accrues_nothing` | An unreferred trader generates nothing for anybody — the share sweeps to the treasury (§ 8.3) rather than being earmarked for an IB who does not exist. | — |
| 8 | `several_traders_aggregate_under_one_ib` | Two traders under one IB both count, and their volumes aggregate for tiering. | — |
| 9 | `the_tier_boundaries_match_the_published_table` | The tier table, pinned exactly. | — |
| 10 | `crossing_a_volume_boundary_promotes_the_ib` | An IB whose referred volume crosses $5M is paid at Silver on the sync that crosses it — 10% of fees rather than 8%, which at the launch split is the whole pool. | — |
| 11 | `a_promotion_is_not_applied_retroactively` | Promotion applies to what is synced afterwards, not retroactively — stated in the code and pinned here, because a silent retroactive recalculation is exactly the kind of surprise that starts disputes. | — |
| 12 | `a_parent_ib_earns_an_override_on_its_sub_brokers` | A sub-broker's parent earns 20% of what the child earned — an override on the child's rebate, not a second bite of the trader's fees. | — |
| 13 | `a_two_level_chain_stays_inside_the_referral_pool` | The chain never costs more than the pool set aside: child + parent together stay inside what the trader's fees contributed. | — |
| 14 | `a_missing_parent_account_does_not_block_the_childs_rebate` | A missing parent account must not block the child's own rebate — the override is simply not paid and stays in the pool. | — |
| 15 | `an_ib_claims_permissionlessly_through_the_cpi` | No approval, no schedule, no counterparty. | — |
| 16 | `claiming_with_nothing_owed_is_refused` | Claiming with nothing owed is refused. | `Nothing to claim` |
| 17 | `one_ib_cannot_claim_anothers_rebate` | One ib cannot claim anothers rebate. | `Nothing to claim` |
| 18 | `a_claim_can_never_exceed_the_referral_pool` | The constraint that protects the treasury. | — |
| 19 | `core_refuses_a_payout_from_an_unregistered_authority` | Core refuses a payout signed by anyone other than the authority its admin registered — the one thing core knows about the referral programme. | — |
| 20 | `the_whole_ib_lifecycle_preserves_every_invariant` | Full lifecycle end to end, with the protocol's invariants holding throughout. | — |

Both programs run together in every integration row: the referral program reads
core's real `UserAccount` and claims through the real CPI.
