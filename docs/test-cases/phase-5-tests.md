# Phase 5 — Test Cases (42 tests)

**Guide:** [`../guides/phase-5-guide.md`](../guides/phase-5-guide.md) · **Flowchart:** [`../diagrams/phase-5.png`](../diagrams/phase-5.png)

Phase 5's exit criterion is *"LP accounting exact under adversarial sequences"*, so
the suite is built around the adversary: threat T6 attempted from both directions (deposit
ahead of a win; request out ahead of a loss), interleaved multi-provider sequences with
invariants after every step, and the share-maths properties (never dilute, instant
round-trip never profits, splitting never beats one redemption).

Invariant **I8** (`supply > 0 ⟺ aum > 0`) was added this phase and caught stranded USDC on
its first run — the exit-fee-waiver test is its permanent regression.

**Run:**
```bash
anchor build
cargo test -p solfx-core --test liquidity
cargo test -p solfx-math lp
```

## unit: lp share maths — `crates/solfx-math/src/lp.rs` (13)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `an_empty_pool_is_worth_par` | An empty pool is worth par. | — |
| 2 | `the_first_deposit_mints_one_to_one` | The first deposit mints one to one. | — |
| 3 | `a_deposit_into_a_profitable_pool_buys_fewer_shares` | A deposit into a profitable pool buys fewer shares. | — |
| 4 | `redemption_returns_the_share_of_aum` | Redemption returns the share of aum. | — |
| 5 | `redeeming_the_whole_supply_returns_the_whole_pool` | Redeeming the whole supply returns the whole pool. | — |
| 6 | `redeeming_more_than_exists_is_rejected` | Redeeming more than exists is rejected. | `MathError::InvalidParameter` |
| 7 | `a_deposit_never_raises_nav_per_share_for_the_depositor` | The anti-dilution property, at a value chosen to force rounding. | — |
| 8 | `a_withdrawal_never_lowers_nav_per_share_for_those_who_stay` | A withdrawal never lowers nav per share for those who stay. | — |
| 9 | `the_exit_fee_rounds_up` | The exit fee rounds up. | — |
| 10 | `no_performance_fee_below_the_high_water_mark` | No performance fee below the high water mark. | — |
| 11 | `the_performance_fee_is_ten_percent_of_the_gain_above_the_mark` | The performance fee is ten percent of the gain above the mark. | — |
| 12 | `an_lp_who_joined_after_a_drawdown_pays_nothing_until_the_peak_returns` | The documented limitation, asserted so it cannot change silently. | — |
| 13 | `degenerate_inputs_do_not_panic` | Degenerate inputs do not panic. | `MathError::Overflow` |

## property — `crates/solfx-math/tests/properties.rs` (8)

**liquidity pool**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_deposit_never_dilutes_the_existing_pool` | The property the whole module exists for. | — |
| 2 | `a_withdrawal_never_dilutes_the_remaining_pool` | The mirror: a withdrawal must never dilute those who stay. | — |
| 3 | `depositing_and_redeeming_immediately_never_profits` | The LP mirror of round_trip_at_the_same_price_always_loses. | — |
| 4 | `redeeming_everything_returns_everything` | Redeeming the entire supply must return the entire pool — no dust may be stranded where nobody can ever claim it. | — |
| 5 | `splitting_a_redemption_never_beats_one_redemption` | Splitting a redemption into two must never beat doing it in one go. | — |
| 6 | `the_exit_fee_rounds_up_but_never_takes_everything` | The exit fee always rounds toward the remaining pool, and never exceeds the payout. | — |
| 7 | `the_performance_fee_only_applies_above_the_mark` | The performance fee is zero at or below the mark, and monotonic above it. | — |
| 8 | `no_lp_input_can_panic` | No LP input can panic, for any combination. | — |

## compute-unit regression — `programs/solfx-core/tests/compute_budget.rs` (1)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `liquidity_instructions_stay_within_their_ceilings` | The LP vault (ARCHITECTURE.md § 5.4 LP, § 8.1 streams 4–5). | — |

## integration: LP vault — `programs/solfx-core/tests/liquidity.rs` (20)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_provider_can_deposit_wait_and_redeem` ◆ | A provider can deposit wait and redeem. | — |
| 2 | `redemption_before_the_cooldown_elapses_is_refused` | The cooldown is the defence. | — |
| 3 | `redemption_without_a_request_is_refused` | Redemption without a request is refused. | — |
| 4 | `a_cancelled_request_cannot_be_settled` | A cancelled request cannot be settled. | — |
| 5 | `cancelling_is_free_and_the_provider_keeps_their_shares` | Cancelling costs nothing. | — |
| 6 | `two_outstanding_requests_are_refused` | Two outstanding requests are refused. | — |
| 7 | `requesting_more_shares_than_held_is_refused` | Requesting more shares than held is refused. | — |
| 8 | `the_jit_attack_cannot_capture_a_gain_without_carrying_risk` | The attack, attempted directly. | — |
| 9 | `an_lp_cannot_exit_ahead_of_a_loss_the_pool_is_about_to_take` | The mirror attack: withdraw ahead of a loss the pool is about to take. | — |
| 10 | `min_usdc_out_protects_a_provider_from_settling_into_a_collapse` | Slippage protection on the exit. | — |
| 11 | `the_exit_fee_accrues_to_the_providers_who_stayed` | The exit fee stays in the pool. | — |
| 12 | `no_performance_fee_is_charged_without_a_gain` | No performance fee while the pool is at or below its high-water mark. | — |
| 13 | `a_performance_fee_is_charged_on_a_real_gain` | A performance fee is charged once the pool has genuinely made money, and it goes to the treasury rather than staying in the pool. | — |
| 14 | `lp_accounting_survives_interleaved_providers` | Many providers, interleaved deposits and redemptions, with the invariants re-checked after every step. | — |
| 15 | `a_late_provider_does_not_dilute_the_early_one` | A provider who joins after the pool has made money buys fewer shares for the same money. | — |
| 16 | `a_depleted_insurance_fund_blocks_new_positions` | § 7.2: an insurance fund below its floor means the protocol has no reserve behind new risk, so it stops taking any. | — |
| 17 | `the_skew_cap_closes_the_heavy_side_but_not_the_light_one` ◆ | § 7.4: past the skew cap the heavy side closes to new opens. | — |
| 18 | `the_utilisation_ceiling_blocks_opens_the_pool_cannot_back` | § 7.2: a pool cannot credibly stand behind unlimited open interest. | — |
| 19 | `the_admin_can_sweep_the_fee_vault_and_only_the_fee_vault` | The admin can sweep the treasury's own fees — and nothing else. | — |
| 20 | `a_stranger_cannot_sweep_the_treasury` | A stranger cannot sweep the treasury. | — |

◆ = permanent regression test for a defect this phase's invariants caught.
