# Phase 2 — Test Cases (131 tests)

**Guide:** [`../guides/phase-2-guide.md`](../guides/phase-2-guide.md) · **Flowchart:** [`architecture/phase-2.png`](../../architecture/phase-2.png)

Phase 2's suite moves from pure maths to the **real BPF binary**: every integration
test runs the built program in-process via LiteSVM, and the harness re-asserts the
invariants after *every* instruction (not at scenario end — that is how the unclamped-claim
defect was caught within one instruction in Phase 4).

Oracle conditions are **synthesised, not fetched**: the harness writes `PriceUpdateV2`
accounts owned by the real Pyth receiver program, which lets the suite reproduce exactly
what Phase 0b measured — a Friday close read 21 hours later, USD/IDR's 30-bps band, a
future-dated timestamp — none of which a live feed can produce on demand. Anchor's
ownership check is not bypassed; the program validates these accounts exactly as in
production.

Negative tests assert the **named error**, not just failure — a test that only asserts
"some error" still passes when the program starts rejecting for the wrong reason.

**Run:**
```bash
anchor build   # required first: the suite loads target/deploy/solfx_core.so
cargo test -p solfx-core --test protocol --test markets --test collateral --test oracle
```

## unit: oracle maths — `crates/solfx-math/src/oracle.rs` (33)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `normalizes_a_real_eur_usd_quote` | The canonical case: every FX and metals feed in the measured set publishes at -8. | — |
| 2 | `normalizes_usd_jpy_without_overflowing_i64` | Normalizes usd jpy without overflowing i64. | — |
| 3 | `normalizes_gold_near_four_thousand` | Normalizes gold near four thousand. | — |
| 4 | `exponent_of_negative_nine_is_the_identity` | Exponent of negative nine is the identity. | — |
| 5 | `exponent_below_negative_nine_divides` | Exponent below negative nine divides. | — |
| 6 | `positive_exponent_multiplies` | Positive exponent multiplies. | — |
| 7 | `non_positive_price_is_rejected` | Non positive price is rejected. | `MathError::InvalidPrice` |
| 8 | `price_that_underflows_to_zero_is_rejected` | A price so small it normalises away is a broken feed, not a cheap asset. | `MathError::InvalidPrice` |
| 9 | `absurd_exponents_error_rather_than_wrap` | Absurd exponents error rather than wrap. | `MathError::Overflow` |
| 10 | `conf_rounds_up_while_price_rounds_down` | Confidence rounds up where price rounds down. | — |
| 11 | `zero_confidence_survives_normalisation` | Zero confidence survives normalisation. | — |
| 12 | `conf_normalises_at_the_same_scale_as_price` | Conf normalises at the same scale as price. | — |
| 13 | `conf_bps_matches_the_measured_gold_figure` | Conf bps matches the measured gold figure. | — |
| 14 | `confidence_ceiling_rejects_a_wide_band` | Confidence ceiling rejects a wide band. | `MathError::ConfidenceTooWide` |
| 15 | `conf_bps_rounds_up_so_a_sub_bp_band_is_never_free` | Conf bps rounds up so a sub bp band is never free. | — |
| 16 | `non_positive_price_cannot_be_validated` | Non positive price cannot be validated. | `MathError::InvalidPrice` |
| 17 | `fresh_price_passes` | Fresh price passes. | — |
| 18 | `stale_price_is_rejected_at_the_boundary` | Stale price is rejected at the boundary. | `MathError::PriceTooStale` |
| 19 | `a_friday_close_read_on_saturday_is_rejected` | The gap get_price_no_older_than leaves open. | `MathError::PriceTooStale` |
| 20 | `future_dated_price_is_rejected` | Future dated price is rejected. | `MathError::PriceFromFuture` |
| 21 | `freshness_does_not_overflow_at_the_extremes` | Freshness does not overflow at the extremes. | `MathError::PriceTooStale`<br>`MathError::PriceFromFuture` |
| 22 | `deviation_is_symmetric_in_direction` | Deviation is symmetric in direction. | — |
| 23 | `deviation_gate_trips_above_the_ceiling` | Deviation gate trips above the ceiling. | `MathError::DeviationTooLarge` |
| 24 | `chf_depeg_scale_move_trips_the_breaker` | EUR/CHF fell ~30% on 15 Jan 2015. | `MathError::DeviationTooLarge` |
| 25 | `deviation_rejects_a_non_positive_reference` | Deviation rejects a non positive reference. | `MathError::InvalidPrice` |
| 26 | `composes_eur_gbp_by_division` | Composes eur gbp by division. | — |
| 27 | `composes_gbp_jpy_by_multiplication` | Composes gbp jpy by multiplication. | — |
| 28 | `composed_confidence_is_the_linear_sum_not_the_quadrature_sum` | The documented deviation from § 3.5: linear, not quadrature. | — |
| 29 | `sub_basis_point_leg_confidence_is_not_erased` | A sub-basis-point leg confidence must survive composition. | — |
| 30 | `composed_price_inherits_the_staler_leg_timestamp` | Composed price inherits the staler leg timestamp. | — |
| 31 | `composed_price_is_subject_to_the_confidence_ceiling` | Composed price is subject to the confidence ceiling. | `MathError::ConfidenceTooWide` |
| 32 | `composition_that_underflows_to_zero_is_rejected` | Composition that underflows to zero is rejected. | `MathError::InvalidPrice` |
| 33 | `pow10_is_exact_and_bounded` | Pow10 is exact and bounded. | `MathError::Overflow` |

## property — `crates/solfx-math/tests/properties.rs` (10)

**oracle**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `exponent_of_negative_nine_is_the_identity` | The exponent that matches PRICE_PRECISION must leave the mantissa untouched. | — |
| 2 | `normalisation_is_monotonic_in_price` | Normalisation must preserve ordering. | — |
| 3 | `confidence_never_normalises_below_price` | Price floors and confidence ceils. | — |
| 4 | `a_non_zero_confidence_never_rounds_to_zero_bps` | A non-zero confidence must never round away to nothing. | — |
| 5 | `freshness_accepts_exactly_the_configured_window` | The freshness window is closed at both ends, and open exactly on the boundary. | — |
| 6 | `deviation_is_symmetric_about_the_reference` | Deviation must not depend on which side of the reference the price fell. | — |
| 7 | `composition_never_narrows_the_confidence_band` | Composition must never report a tighter band than either leg carried. | — |
| 8 | `multiplicative_composition_is_commutative` | Multiplicative composition is order-independent. | — |
| 9 | `divide_then_multiply_returns_to_the_original_price` | Dividing by a leg and multiplying it back must return to the starting price, within the truncation two floored divisions can introduce. | — |
| 10 | `no_oracle_input_can_panic` | Every oracle entry point returns an error rather than panicking, for any input. | — |

## integration: collateral & I1 — `programs/solfx-core/tests/collateral.rs` (14)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `deposit_and_withdraw_preserve_the_invariant` | Deposit and withdraw preserve the invariant. | — |
| 2 | `the_invariant_holds_across_interleaved_users` | Many users, interleaved, with the invariant re-checked after each step. | — |
| 3 | `cannot_withdraw_more_than_the_free_balance` | Cannot withdraw more than the free balance. | — |
| 4 | `cannot_withdraw_from_an_empty_account` | Cannot withdraw from an empty account. | `Insufficient free collateral` |
| 5 | `zero_amount_transfers_are_rejected` | Zero amount transfers are rejected. | `Amount must be greater than zero` |
| 6 | `withdrawals_survive_a_global_pause` | The load-bearing claim. | — |
| 7 | `deposits_survive_a_global_pause` | Adding collateral reduces risk, so blocking it during a pause would push positions toward liquidation during exactly the incident the pause was called for. | — |
| 8 | `cannot_deposit_from_someone_elses_token_account` | Cannot deposit from someone elses token account. | — |
| 9 | `cannot_withdraw_into_someone_elses_account_using_their_pda` | Cannot withdraw into someone elses account using their pda. | — |
| 10 | `a_token_account_for_the_wrong_mint_is_rejected` | A token account for the wrong mint is rejected. | — |
| 11 | `the_referrer_is_bound_at_creation` | The referrer is written once at account creation and there is no instruction anywhere in the protocol that changes it. | — |
| 12 | `a_user_cannot_refer_themselves` | A user cannot refer themselves. | `Parameter out of range` |
| 13 | `an_account_cannot_be_created_twice` | An account cannot be created twice. | — |
| 14 | `lifetime_totals_track_every_movement` | Lifetime totals track every movement. | — |

## compute-unit regression — `programs/solfx-core/tests/compute_budget.rs` (3)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `compute_budgets_stay_within_their_ceilings` | Compute budgets stay within their ceilings. | — |
| 2 | `position_instructions_stay_within_their_ceilings` | The position lifecycle, measured separately because it is the budget § 5.5 actually sets: open_position around 120k CU, liquidation under 200k. | — |
| 3 | `the_widest_instruction_still_fits_its_stack_frame` | The account-count guard. | — |

## integration: market listing — `programs/solfx-core/tests/markets.rs` (32)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_listed_market_starts_untradeable` | A listed market starts untradeable. | — |
| 2 | `many_markets_list_against_one_unchanged_binary` | The § 5.6 property, demonstrated: twelve instruments across four price/quote shapes, all against one deployment of one binary. | — |
| 3 | `market_indices_must_be_sequential` | Market indices must be sequential. | — |
| 4 | `a_stranger_cannot_list_a_market` | A stranger cannot list a market. | — |
| 5 | `rejects_an_all_zero_feed_id` | Phase 0b found eight EM symbols listed in Pyth's catalogue with publish_time == 0 — registered names with no data behind them. | `Feed id must not be all zeroes` |
| 6 | `continuous_index_markets_are_blocked_pending_q1b` | The measurement that killed the weekend product, encoded as a constraint. | — |
| 7 | `a_synthetic_market_requires_both_legs` | A synthetic market requires both legs. | `Feed id must not be all zeroes` |
| 8 | `a_direct_market_rejects_a_secondary_feed` | An account a market does not use is an account nobody audits. | `Feed id must not be all zeroes` |
| 9 | `a_non_usd_quoted_market_requires_a_conversion_feed` | Correction C-3: for USD/INR the PnL formula produces Rupees. | `Feed id must not be all zeroes` |
| 10 | `a_usd_quoted_market_rejects_a_conversion_feed` | A usd quoted market rejects a conversion feed. | `Feed id must not be all zeroes` |
| 11 | `the_conversion_direction_is_recorded_alongside_the_feed` | The conversion direction is recorded alongside the feed. | — |
| 12 | `rejects_zero_leverage` | Rejects zero leverage. | — |
| 13 | `rejects_leverage_above_the_protocol_ceiling` | 500x is what GMTrade offers. | — |
| 14 | `rejects_maintenance_margin_at_or_above_initial_margin` | A position that is liquidatable the instant it opens is not a position. | — |
| 15 | `rejects_an_initial_margin_that_contradicts_the_leverage_cap` | imr_bps and max_leverage state the same constraint twice. | — |
| 16 | `rejects_a_liquidation_fee_at_or_above_the_maintenance_margin` | The penalty must leave something behind. | — |
| 17 | `rejects_a_staleness_window_beyond_the_ceiling` | Every listable feed publishes at a 1 s cadence. | — |
| 18 | `rejects_a_zero_staleness_window` | Rejects a zero staleness window. | — |
| 19 | `rejects_a_confidence_ceiling_beyond_the_protocol_limit` | Rejects a confidence ceiling beyond the protocol limit. | — |
| 20 | `rejects_inconsistent_position_size_bounds` | Rejects inconsistent position size bounds. | — |
| 21 | `rejects_an_invalid_session_calendar` | Rejects an invalid session calendar. | — |
| 22 | `rejects_an_empty_symbol` | Rejects an empty symbol. | — |
| 23 | `rejects_an_oversized_symbol` | Rejects an oversized symbol. | — |
| 24 | `risk_parameters_can_be_retuned_in_place` | Risk parameters are per-market data, never constants. | — |
| 25 | `a_retune_into_an_invalid_envelope_is_rejected_whole` | A partial update must not be able to leave a market in a state nobody validated. | — |
| 26 | `the_carry_rate_is_stored_on_chain_and_readable` | The carry rate is the interest-rate differential plus the protocol markup. | — |
| 27 | `admin_can_move_a_market_through_the_tradeable_states` | Admin can move a market through the tradeable states. | — |
| 28 | `continuous_regime_states_cannot_be_set_by_hand` | Both belong to the continuous regime, which no market can currently occupy. | `Invalid market status transition` |
| 29 | `delisting_is_terminal` | Delisting is terminal. | `Invalid market status transition` |
| 30 | `only_active_and_weekend_mode_permit_opening` | The allow-list in MarketStatus::allows_open is deliberately narrow: a status added later is closed to new positions until someone writes it in on purpose. | — |
| 31 | `liquidation_remains_possible_in_every_state_except_initialized` | An underwater position must stay liquidatable even while the market is halted. | — |
| 32 | `a_direct_market_reports_no_synthetic_source` | A direct market reports no synthetic source. | — |

## integration: oracle gates — `programs/solfx-core/tests/oracle.rs` (26)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_direct_market_prices_correctly` | Exponent normalisation, end to end. | — |
| 2 | `gold_prices_correctly_at_its_own_scale` | Gold at 4046.95 — the Friday close recorded in oracle-feasibility.md, and the tightest feed measured in the whole set at 1.43 bps p50. | — |
| 3 | `a_high_valued_quote_pair_prices_without_overflow` | USD/JPY at 157.20 must not overflow i64 at PRICE_PRECISION — eleven orders of magnitude of headroom, per § 6.1. | — |
| 4 | `a_crypto_market_prices_through_the_same_path` | The generic-engine proof (§ 5.6): crypto needs a regime, not new maths. | — |
| 5 | `a_friday_close_read_on_saturday_is_rejected` | The measurement that ended the weekend product, as a regression test. | `Oracle price is stale` |
| 6 | `the_staleness_window_is_exact_at_its_boundary` | The staleness window is exact at its boundary. | `Oracle price is stale` |
| 7 | `a_future_dated_price_is_rejected` | The gap the SDK helper leaves open. | `Oracle price is dated in the future` |
| 8 | `a_price_goes_stale_as_the_clock_advances` | A price that was fresh when posted goes stale as the clock advances. | `Oracle price is stale` |
| 9 | `a_band_wider_than_the_market_ceiling_halts_the_market` ◆ | USD/IDR measured 30.11 bps p95 and was excluded from the listable set on exactly this test. | — |
| 10 | `a_band_inside_the_ceiling_does_not_halt` | A band inside the ceiling does not halt. | — |
| 11 | `the_confidence_ceiling_is_per_market` | Gold's ceiling is tighter than EUR/USD's, because its measured band is. | — |
| 12 | `a_valid_update_for_another_feed_is_rejected` | A real, fresh, fully verified update for the wrong instrument must not price this market. | — |
| 13 | `a_partially_verified_update_is_rejected` | Partial verification lowers the number of Wormhole guardians that must collude to forge a price. | — |
| 14 | `a_large_dislocation_trips_the_breaker_and_halts_the_market` | EUR/CHF fell ~30% in minutes on 15 January 2015 and bankrupted brokers including Alpari UK. | — |
| 15 | `a_halted_market_can_still_be_repriced` | The reason the crank *observes* rather than *gates*: if refreshing the reference required passing the deviation check, a market could never recover from a genuine large move. | — |
| 16 | `a_move_inside_the_deviation_limit_does_not_halt` | A move inside the deviation limit does not halt. | — |
| 17 | `a_feed_without_an_ema_still_prices` | A feed with no EMA yet must not be permanently untradeable — § 7.1 skips the check when the reference is not established. | — |
| 18 | `a_synthetic_market_composes_both_legs` | EUR/GBP = EUR/USD ÷ GBP/USD. | — |
| 19 | `a_synthetic_market_without_its_second_leg_is_rejected` | A synthetic market without its second leg is rejected. | — |
| 20 | `a_synthetic_market_rejects_when_either_leg_is_stale` | § 7.2: a synthetic market rejects if either leg fails validation. | `Oracle price is stale` |
| 21 | `composed_confidence_is_gated_on_the_sum_not_the_legs` | Confidence compounds. | — |
| 22 | `a_non_usd_quoted_market_requires_its_conversion_feed` | USD/INR is the shape that makes C-3 mandatory rather than optional: PnL lands in Rupees, and booking it as USDC would mis-price every position by a factor of ~88. | — |
| 23 | `supplying_an_unused_price_account_is_rejected` | An account the market does not use is an account nobody audits. | — |
| 24 | `a_price_account_not_owned_by_the_pyth_receiver_is_rejected` | An attacker-owned account holding perfectly-shaped PriceUpdateV2 bytes must not be readable as a price. | — |
| 25 | `a_delisted_market_cannot_be_cranked` | A delisted market cannot be cranked. | `Market is not active` |
| 26 | `anyone_can_crank` | Cranking is permissionless: a market whose price only its operator can record has a single point of failure, and § 9.2 requires a resilient keeper market. | — |

## integration: protocol & authority — `programs/solfx-core/tests/protocol.rs` (13)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `initialize_protocol_writes_every_field` | Initialize protocol writes every field. | — |
| 2 | `rejects_a_collateral_mint_with_the_wrong_decimals` | USDC has six decimals and QUOTE_PRECISION is 1e6. | — |
| 3 | `rejects_a_fee_split_that_does_not_total_one_hundred_percent` | Rejects a fee split that does not total one hundred percent. | — |
| 4 | `rejects_a_treasury_share_larger_than_the_lp_share` | Under-paying LPs is the single most common cause of death for pool-backed perp protocols (§ 8.3): no liquidity, no depth, no traders, no fees. | — |
| 5 | `protocol_cannot_be_initialised_twice` | Protocol cannot be initialised twice. | — |
| 6 | `admin_handover_requires_the_new_authority_to_sign` | Two steps, because a single-step transfer to a mistyped address is unrecoverable: the protocol would be left with an admin key nobody holds and every risk parameter frozen. | — |
| 7 | `a_third_party_cannot_accept_a_pending_handover` | A third party cannot accept a pending handover. | — |
| 8 | `accepting_with_no_handover_pending_fails` | Accepting with no handover pending fails. | `No admin transfer is pending` |
| 9 | `the_old_admin_loses_authority_after_handover` | The old admin loses authority after handover. | `Signer is not the protocol admin` |
| 10 | `guardian_can_pause_but_not_unpause` | Guardian can pause but not unpause. | — |
| 11 | `a_stranger_cannot_pause` | A stranger cannot pause. | `Signer is not the guardian` |
| 12 | `guardian_can_halt_a_single_market` | Guardian can halt a single market. | — |
| 13 | `guardian_has_no_instruction_that_moves_funds` | The guardian key holds no spending authority at all. | — |
