# Phase 1 — Test Cases (166 tests)

**Guide:** [`../guides/phase-1-guide.md`](../guides/phase-1-guide.md) · **Flowchart:** [`../diagrams/phase-1.dot`](../diagrams/phase-1.dot)

Phase 1's suite is the foundation everything later trusts: **166 tests** over the pure
maths — unit tests for known values (every worked example from the architecture is a test),
and property tests for invariants across the whole parameter space (4,096 cases locally,
100,000 in CI's deep run — `PROPTEST_CASES=100000`).

The design rule: a unit test checks *a* value; a property test checks *the shape of the
function* — monotonicity, antisymmetry, rounding direction, conservation. The expensive
bugs (a rounding that flips at one size, an overflow only at gold's scale) live where only
properties look.

**The flagship:** `round_trip_at_the_same_price_always_loses`. Open and immediately close at
an unchanged oracle price, across sizes × prices × spreads × fee tiers × both directions:
the trader must always end with less. If it ever fails, there is free money and bots drain
the vault. Its twin runs the same property through the C-3 quote-conversion path.

**Run:**
```bash
cargo test -p solfx-math
PROPTEST_CASES=100000 cargo test --release -p solfx-math --test properties
```

## unit: constants — `crates/solfx-math/src/constants.rs` (5)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `notional_divisor_is_1e12` | Notional divisor is 1e12. | — |
| 2 | `signed_mirrors_match_their_unsigned_originals` | The signed mirrors must never drift from the unsigned originals. | — |
| 3 | `one_standard_lot_is_1e14_base_units` | One standard lot is 1e14 base units. | — |
| 4 | `one_pip_is_100_000_price_units` | A pip on a 4-decimal pair is 1e-4, so 1e5 units at PRICE_PRECISION = 1e9. | — |
| 5 | `price_precision_leaves_headroom_in_i64` | USD/JPY near 157 must sit far inside i64. | — |

## unit: error — `crates/solfx-math/src/error.rs` (1)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `every_variant_has_a_distinct_message` | Every variant must render something a user could act on. | — |

## unit: fees — `crates/solfx-math/src/fees.rs` (17)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `fee_tiers_match_the_architecture_schedule` | Fee tiers match the architecture schedule. | — |
| 2 | `fee_tier_boundaries_are_exclusive_upper` | Fee tier boundaries are exclusive upper. | — |
| 3 | `fee_rate_falls_monotonically_with_volume` | Fee rate falls monotonically with volume. | — |
| 4 | `one_bps_on_one_lot_is_about_eleven_dollars` | C-2's target: 1 bp per side on a 1-lot EUR/USD position ($108,543) is ~$10.85. | — |
| 5 | `round_trip_cost_matches_the_explainer` | FOREX-EXPLAINED.md § 11: round trip at 1 bp/side is ~$22 on a standard lot. | — |
| 6 | `sub_basis_point_rates_are_expressible` | The 0.8 bps tier is the reason rates are not stored in whole basis points. | — |
| 7 | `fees_round_up` | Fees round up. | — |
| 8 | `no_positive_notional_trade_is_free` | Threat T14: no positive-notional trade may ever be free. | — |
| 9 | `zero_notional_or_zero_rate_charges_nothing` | Zero notional or zero rate charges nothing. | — |
| 10 | `launch_split_is_valid_and_lp_led` | Launch split is valid and lp led. | — |
| 11 | `a_split_that_does_not_total_100_percent_is_rejected` | A split that does not total 100 percent is rejected. | `MathError::InvalidParameter` |
| 12 | `split_matches_the_configured_percentages` | Split matches the configured percentages. | — |
| 13 | `split_conserves_every_unit` | Invariant I7: every unit collected must land somewhere. | — |
| 14 | `split_dust_goes_to_treasury` | Dust from flooring must accrue to the protocol, never away from it. | — |
| 15 | `liquidation_penalty_splits_forty_forty_twenty` | Liquidation penalty splits forty forty twenty. | — |
| 16 | `liquidation_penalty_split_conserves_every_unit` | Liquidation penalty split conserves every unit. | — |
| 17 | `liquidator_reward_dwarfs_transaction_cost` | ARCHITECTURE.md § 6.8: a 40% share of a 0.5% penalty on a $10,000 position is $20 — far above the ~$0.002 compute cost, which is what keeps liquidations profitable during the congestion spikes that cause them. | — |

## unit: fixed (rounding core) — `crates/solfx-math/src/fixed.rs` (14)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `floor_and_ceil_agree_on_exact_division` | Floor and ceil agree on exact division. | — |
| 2 | `ceil_rounds_up_on_remainder` | Ceil rounds up on remainder. | — |
| 3 | `signed_floor_rounds_toward_negative_infinity_not_toward_zero` | The bug this module exists to prevent: / truncates toward zero, which would round a trader's loss in the trader's favour. | — |
| 4 | `signed_ceil_grows_charges_and_shrinks_credits` | The mirror of the above: a signed *cost* must round the other way, growing a charge and shrinking a credit. | — |
| 5 | `signed_floor_and_ceil_bracket_the_true_value` | Floor and ceil must bracket the true value, never both land on the same side. | — |
| 6 | `signed_floor_shrinks_gains_and_grows_losses` | Signed floor shrinks gains and grows losses. | — |
| 7 | `divide_by_zero_is_an_error_not_a_panic` | Divide by zero is an error not a panic. | `MathError::DivideByZero` |
| 8 | `negative_divisor_is_rejected` | Negative divisor is rejected. | `MathError::InvalidParameter` |
| 9 | `signed_ceil_rejects_degenerate_divisors` | Signed ceil rejects degenerate divisors. | `MathError::DivideByZero`<br>`MathError::Overflow` |
| 10 | `overflow_is_an_error_not_a_wrap` | Overflow is an error not a wrap. | `MathError::Overflow` |
| 11 | `abs_handles_i128_min_without_panicking` | Abs handles i128 min without panicking. | `MathError::Overflow` |
| 12 | `narrowing_casts_reject_rather_than_truncate` | Narrowing casts reject rather than truncate. | `MathError::Overflow` |
| 13 | `positive_to_u128_enforces_its_precondition` | Positive to u128 enforces its precondition. | `MathError::InvalidPrice` |
| 14 | `clamp_symmetric_bounds_both_directions` | Clamp symmetric bounds both directions. | `MathError::InvalidParameter` |

## unit: funding — `crates/solfx-math/src/funding.rs` (23)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `long_carry_matches_the_explainer_worked_example` | FOREX-EXPLAINED.md § 9: long EUR/USD with EUR at 2% and USD at 4% pays 2%/year. | — |
| 2 | `short_carry_has_the_opposite_sign` | The same position shorted receives the differential instead. | — |
| 3 | `a_matched_pair_never_costs_the_protocol` | The two sides cannot be exact mirrors under protocol-favourable rounding, but the pair must never net *against* the protocol: it charges the long at least as much as it credits the short. | — |
| 4 | `markup_is_charged_in_both_directions` | The markup is a charge on both sides — that is the revenue stream, and § 6.7 requires it be visible as its own line item. | — |
| 5 | `equal_interest_rates_leave_only_the_markup` | Equal rates on both currencies leave only the markup. | — |
| 6 | `carry_index_advances_proportionally_to_time` | Carry index advances proportionally to time. | — |
| 7 | `carry_index_never_moves_backwards` | Carry index never moves backwards. | — |
| 8 | `daily_carry_matches_the_explainer` | $108,543 of notional at 2%/year costs about $5.95/day — the figure in FOREX-EXPLAINED.md § 9. | — |
| 9 | `accrued_carry_rounds_up` | Accrued carry rounds up. | — |
| 10 | `funding_rate_is_zero_on_a_balanced_book` | Funding rate is zero on a balanced book. | — |
| 11 | `funding_rate_is_positive_when_longs_are_crowded` | Funding rate is positive when longs are crowded. | — |
| 12 | `funding_rate_is_negative_when_shorts_are_crowded` | Funding rate is negative when shorts are crowded. | — |
| 13 | `funding_rate_respects_the_cap` | Funding rate respects the cap. | — |
| 14 | `funding_is_zero_when_a_side_is_empty` | Funding is a transfer between traders. | — |
| 15 | `funding_update_charges_the_heavy_side` | Funding update charges the heavy side. | — |
| 16 | `funding_conserves_value_between_the_two_sides` | Invariant I3: funding must net to zero across the market — and any rounding dust must favour the protocol, never leave it short. | — |
| 17 | `funding_update_is_empty_without_elapsed_time_or_rate` | Funding update is empty without elapsed time or rate. | — |
| 18 | `accrued_funding_is_signed` | Accrued funding is signed. | — |
| 19 | `accrued_funding_rounds_toward_the_protocol` | A payment rounds up in magnitude, a receipt rounds down — both favour the protocol. | — |
| 20 | `skew_ratio_spans_minus_one_to_one` | Skew ratio spans minus one to one. | — |
| 21 | `skew_cap_threshold_is_detectable` | § 7.4 blocks opens on the heavy side past a 0.6 skew ratio. | — |
| 22 | `utilisation_rounds_up_and_rejects_zero_aum` | Utilisation rounds up and rejects zero aum. | `MathError::DivideByZero` |
| 23 | `utilisation_cap_threshold_is_detectable` | § 7.2 rejects new opens on the crowded side past 80% utilisation. | — |

## unit: margin — `crates/solfx-math/src/margin.rs` (22)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `mmr_tiers_match_the_architecture_table` | Mmr tiers match the architecture table. | — |
| 2 | `mmr_tier_boundaries_are_exclusive_upper` | Boundaries are exclusive-upper: exactly $100k is already in the next tier. | — |
| 3 | `mmr_multiplier_is_monotonic_in_notional` | Mmr multiplier is monotonic in notional. | — |
| 4 | `initial_margin_matches_the_explainer_worked_example` | FOREX-EXPLAINED.md § 8: 1 lot EUR/USD at 1.0850 needs $2,170 initial at 50x. | — |
| 5 | `maintenance_margin_applies_the_size_tier` | $108.5k lands in the 1.5x tier, so 1% MMR becomes an effective 1.5%. | — |
| 6 | `maintenance_margin_untiered_below_100k` | Under $100k the tier multiplier is 1.0x, so MMR is the raw percentage. | — |
| 7 | `margin_requirements_round_up` | Margin requirements round up. | — |
| 8 | `zero_margin_ratio_is_rejected` | Zero margin ratio is rejected. | `MathError::InvalidParameter` |
| 9 | `equity_subtracts_every_cost` | Equity subtracts every cost. | — |
| 10 | `negative_funding_increases_equity` | Funding is signed: a position on the light side of the book receives it. | — |
| 11 | `equity_can_go_negative` | Equity can go negative. | — |
| 12 | `liquidation_boundary_is_strict` | A loose comparison here is liquidation griefing (threat T7). | — |
| 13 | `negative_equity_is_always_liquidatable` | Negative equity is always liquidatable, even against a zero margin requirement. | — |
| 14 | `health_factor_reads_one_at_the_boundary` | Health factor reads one at the boundary. | — |
| 15 | `health_factor_handles_degenerate_inputs` | Health factor handles degenerate inputs. | — |
| 16 | `health_factor_and_liquidatable_agree` | The displayed health factor and the actual liquidation test must never disagree, or the UI shows "safe" on a position a keeper is about to close. | — |
| 17 | `effective_leverage_rounds_up` | Effective leverage rounds up. | `MathError::DivideByZero` |
| 18 | `liquidation_price_matches_the_explainer_worked_example` | FOREX-EXPLAINED.md § 8: long 1 lot at 1.0850 with $2,170 margin and $1,085 maintenance liquidates at 1.07415 — a 1% move. | — |
| 19 | `short_liquidation_price_is_above_entry` | A short is liquidated by a rise, so its liquidation price sits above entry. | — |
| 20 | `accrued_costs_pull_liquidation_price_toward_entry` | Accrued costs eat the buffer, so liquidation moves closer to entry. | — |
| 21 | `liquidation_price_rejects_degenerate_inputs` | Liquidation price rejects degenerate inputs. | `MathError::DivideByZero`<br>`MathError::InvalidPrice` |
| 22 | `liquidation_price_clamps_instead_of_going_negative` | An already-underwater long clamps at 1 rather than returning a negative price. | — |

## unit: pnl — `crates/solfx-math/src/pnl.rs` (19)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `notional_matches_architecture_worked_example` | ARCHITECTURE.md § 6.2 worked check: 1 lot EUR/USD at 1.08543 = $108,543.00 | — |
| 2 | `upnl_matches_architecture_worked_example` | ARCHITECTURE.md § 6.4 worked check: 1 lot, +10 pips = +$100.00 | — |
| 3 | `one_pip_on_one_standard_lot_is_ten_dollars` | FOREX-EXPLAINED.md § 2: one standard lot moves $10 per pip. | — |
| 4 | `short_pnl_mirrors_long` | Short pnl mirrors long. | — |
| 5 | `usd_inr_pnl_converts_from_rupees_to_usdc` | Correction C-3, the case that mis-prices by 88x if the conversion is skipped. | — |
| 6 | `usd_jpy_pnl_converts_from_yen_to_usdc` | Usd jpy pnl converts from yen to usdc. | — |
| 7 | `usd_per_quote_multiplies_instead_of_dividing` | The other conversion direction: a market quoted in EUR, converted via EUR/USD. | — |
| 8 | `none_conversion_is_the_identity` | None conversion is the identity. | — |
| 9 | `cost_and_pnl_conversions_round_in_opposite_directions` | Costs round up, payouts round down. | — |
| 10 | `negative_pnl_conversion_rounds_away_from_zero` | A loss must round further negative, never toward zero. | — |
| 11 | `zero_or_negative_prices_are_rejected` | Zero or negative prices are rejected. | `MathError::InvalidPrice` |
| 12 | `every_conversion_direction_rejects_a_nonpositive_rate` | Both conversion directions must reject a bad rate, not just the first one. | `MathError::InvalidPrice` |
| 13 | `composed_collateral_helpers_apply_the_conversion` | The composed helpers must carry the conversion through, not just the primitives. | — |
| 14 | `weighted_entry_rejects_two_empty_legs` | Weighted entry rejects two empty legs. | `MathError::DivideByZero` |
| 15 | `weighted_entry_sits_between_the_two_prices` | Weighted entry sits between the two prices. | — |
| 16 | `weighted_entry_rounds_against_the_trader` | Rounding on a top-up must be adverse: up for a long, down for a short. | — |
| 17 | `proportional_pnl_splits_and_never_over_realises` | Proportional pnl splits and never over realises. | — |
| 18 | `proportional_pnl_rejects_closing_more_than_exists` | Proportional pnl rejects closing more than exists. | `MathError::InvalidParameter`<br>`MathError::DivideByZero` |
| 19 | `conversion_uses_price_precision_scale` | PRICE_PRECISION is imported for the conversion maths; assert the scale we assume. | — |

## unit: pricing — `crates/solfx-math/src/pricing.rs` (24)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `confidence_bps_matches_the_measured_eurusd_figure` | Confidence bps matches the measured eurusd figure. | — |
| 2 | `confidence_bps_rounds_up` | Confidence rounds up so a borderline feed reads as the wider of the two. | — |
| 3 | `confidence_gate_rejects_only_above_the_ceiling` | Confidence gate rejects only above the ceiling. | `MathError::ConfidenceTooWide` |
| 4 | `measured_feed_verdicts_reproduce` | oracle-feasibility.md: USD/IDR failed at 30.11 bps p95 against a 25 bps limit. | `MathError::ConfidenceTooWide` |
| 5 | `confidence_spread_scales_with_the_multiplier` | Confidence spread scales with the multiplier. | — |
| 6 | `skew_delta_is_positive_when_worsening_the_imbalance` | Skew delta is positive when worsening the imbalance. | — |
| 7 | `skew_delta_handles_crossing_through_zero` | Crossing through balance: a 150 short against a 100 long leaves net −50, so the imbalance shrinks by 50. | — |
| 8 | `skew_impact_is_zero_when_improving_the_book` | Skew impact is zero when improving the book. | — |
| 9 | `skew_impact_scales_with_lots_of_imbalance` | Rate 1e6 (0.001 in RATE_PRECISION) = 0.001 bps per lot; 100 lots -> 0.1 bps, which ceils to 1. | — |
| 10 | `skew_impact_is_disabled_by_a_zero_rate` | Skew impact is disabled by a zero rate. | — |
| 11 | `total_spread_sums_components_and_bounds_them` | Total spread sums components and bounds them. | `MathError::InvalidParameter` |
| 12 | `execution_price_is_always_adverse` | The core anti-arbitrage property: a buyer pays more than mid, a seller receives less. | — |
| 13 | `execution_price_applies_the_exact_spread` | 10 bps of 1.08543 is 0.00108543, so buy 1.08651543 and sell 1.08434457. | — |
| 14 | `zero_spread_fills_at_mid` | Zero spread is the only case where a fill happens at mid — and the fee layer still makes the round trip loss-making. | — |
| 15 | `any_nonzero_spread_moves_the_price_at_least_one_unit` | Ceiling rounding guarantees at least one unit of adverse movement, however small the spread. | — |
| 16 | `a_price_too_small_to_carry_the_spread_is_rejected` | The guarantee above has a floor: a price small enough that the spread consumes it entirely cannot produce a bid, and is rejected rather than filled at zero. | `MathError::InvalidPrice` |
| 17 | `execution_price_rejects_a_spread_that_would_zero_the_bid` | Execution price rejects a spread that would zero the bid. | `MathError::InvalidParameter` |
| 18 | `execution_price_rejects_a_zero_result` | At the maximum permitted spread a price of 1 unit would floor the bid to zero. | `MathError::InvalidPrice` |
| 19 | `execution_price_rejects_nonpositive_oracle_input` | Execution price rejects nonpositive oracle input. | `MathError::InvalidPrice` |
| 20 | `notional_floor_is_enforced` | Notional floor is enforced. | `MathError::NotionalTooSmall` |
| 21 | `slippage_bound_is_directional` | Slippage bound is directional. | `MathError::InvalidPrice` |
| 22 | `pip_conversion_matches_the_explainer` | Pip conversion matches the explainer. | — |
| 23 | `pip_conversion_handles_jpy_scale` | A JPY pair's pip is the 2nd decimal, not the 4th. | — |
| 24 | `pip_conversion_rejects_zero_pip_size` | Pip conversion rejects zero pip size. | `MathError::DivideByZero` |

## unit: types — `crates/solfx-math/src/types.rs` (3)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `direction_signs_are_opposite` | Direction signs are opposite. | — |
| 2 | `side_resolution_is_always_adverse` | The trader must cross the adverse side on every one of the four transitions. | — |
| 3 | `open_and_close_cross_opposite_sides` | Opening and closing the same position must cross opposite sides — otherwise a round trip could avoid the spread entirely. | — |

## property — `crates/solfx-math/tests/properties.rs` (38)

**the test that matters**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `round_trip_at_the_same_price_always_loses` | The single most valuable test in the suite (ARCHITECTURE.md § 12.2). | — |
| 2 | `round_trip_never_profits_on_a_non_usd_quoted_market` | The same round trip on a non-USD-quoted market. | — |

**PnL**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `pnl_is_antisymmetric` | A long and a short of the same size, entry and exit must be equal and opposite, to within one unit of protocol-favourable rounding. | — |
| 2 | `pnl_is_zero_at_the_entry_price` | Closing at the entry price yields exactly zero, in both directions. | — |
| 3 | `pnl_is_monotonic_in_price` | PnL must move the right way: a long gains as price rises, a short loses. | — |
| 4 | `partial_closes_never_realise_more_than_the_whole` | Splitting a close into parts must never realise more than closing all at once. | — |
| 5 | `weighted_entry_stays_within_bounds` | A weighted entry always lands between the two prices it averages. | — |
| 6 | `cost_conversion_never_undercuts_payout_conversion` | Converting a cost and converting a payout must never favour the trader: for the same magnitude, the cost conversion is always at least the payout conversion. | — |

**pricing**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `the_book_is_never_crossed` | The book must never be crossed: the price a buyer pays always exceeds the price a seller receives, so there is no risk-free spread to capture. | — |
| 2 | `a_wider_spread_is_never_better_for_the_trader` | A wider spread is always at least as adverse — never accidentally better. | — |
| 3 | `a_nonzero_spread_always_bites` | Any non-zero spread must move the price by at least one unit, or a sub-unit spread rounds away and hands out free fills. | — |
| 4 | `improving_the_book_is_never_penalised` | Trading toward a more balanced book is never charged skew impact. | — |
| 5 | `confidence_spread_is_monotonic` | Confidence is the oracle's own uncertainty; the spread it produces must scale with it. | — |

**fees**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `fees_always_round_toward_the_protocol` | Fees round toward the protocol: the charge is never below the exact value, and never overshoots it by a whole unit. | — |
| 2 | `no_positive_notional_trade_is_ever_free` | Threat T14: no trade with positive notional may ever be free. | — |
| 3 | `fees_are_monotonic_in_rate` | A higher rate never charges less. | — |
| 4 | `fee_splits_conserve_every_unit` | Invariant I7: a split must conserve every unit collected. | — |
| 5 | `split_never_overpays_the_lp_share` | Rounding dust from a split must accrue to the protocol, so the LP share is never more than its exact entitlement. | — |
| 6 | `penalty_splits_conserve_and_never_overpay` | A liquidation penalty split must also conserve, and never pay the liquidator more than their 40% entitlement. | — |
| 7 | `fee_rate_never_rises_with_volume` | The published schedule must never charge a higher rate to a higher-volume trader. | — |

**margin**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `margin_requirements_round_up` | Margin requirements round up, so the protocol always holds at least the exact figure. | — |
| 2 | `mmr_multiplier_never_falls_with_size` | A bigger position never has a smaller MMR multiplier. | — |
| 3 | `health_factor_agrees_with_the_liquidation_test` | The displayed health factor and the liquidation decision must never disagree — otherwise the UI shows "safe" on a position a keeper is about to close. | — |
| 4 | `costs_only_ever_reduce_equity` | Every accrued cost strictly reduces equity. | — |
| 5 | `the_displayed_liquidation_price_is_conservative` | The displayed liquidation price must be conservative: at that price the position is still (just) alive, and real liquidation happens a tick or two beyond it. | — |
| 6 | `liquidation_bites_just_beyond_the_displayed_price` | One tick beyond the displayed price, the position really is liquidatable — the display is tight, not arbitrarily early. | — |

**funding**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `funding_never_pays_out_more_than_it_collects` | Invariant I3: funding is a transfer between traders and must never cost the protocol. | — |
| 2 | `funding_always_charges_the_crowded_side` | The heavy side always pays and the light side always receives. | — |
| 3 | `funding_rate_respects_its_cap` | The funding rate never exceeds its per-market cap in either direction. | — |
| 4 | `the_carry_index_never_moves_backwards` | The carry index is monotonic. | — |
| 5 | `a_matched_carry_pair_never_costs_the_protocol` | A matched long/short pair must never cost the protocol carry. | — |
| 6 | `accrued_carry_rounds_up` | Carry is a charge, so it rounds up — never below the exact value. | — |

**fixed-point primitives**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `floor_and_ceil_bracket_the_exact_quotient` | Ceil is never below floor, and never more than one unit above it. | — |
| 2 | `signed_floor_always_rounds_down` | Signed floor rounds toward −∞ for every sign, unlike Rust's /. | — |
| 3 | `signed_ceil_always_rounds_up` | Signed ceil rounds toward +∞ for every sign. | — |
| 4 | `narrowing_never_silently_truncates` | Narrowing conversions must reject anything they cannot represent, never truncate. | — |
| 5 | `clamp_is_idempotent_and_bounded` | Clamping is idempotent and always lands inside the bound. | — |

**cross-module consistency**

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `liquidator_reward_stays_above_transaction_cost` | A trade's cost must always exceed the compute cost of liquidating it, or liquidations stop being worth running — and unliquidated positions are how vaults die (ARCHITECTURE.md § 6.8). | — |
