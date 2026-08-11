# Phase 4 — Test Cases (52 tests)

**Guide:** [`../guides/phase-4-guide.md`](../guides/phase-4-guide.md) · **Flowchart:** [`../diagrams/phase-4.png`](../diagrams/phase-4.png)

Phase 4's suite is in three layers. **Unit** — the session state machine is a pure
function, so its 27 transition tests need no SVM (the calendar tests pinned 2026-08-01 to a
Saturday and caught a wrong epoch constant on first run). **Integration** — liquidation,
funding, carry, insurance, ADL through the real instructions. **Scenario replays (§ 12.4)**
— the exit gate: real crises through the real engine, asserting the protocol stays *correct*
(conservation to the unit, waterfall order, liquidators paid, protocol usable afterwards),
not that nothing goes wrong.

Writing the CHF-depeg replay found four real defects — each row marked ◆ below is the
permanent regression test for one of them.

**Run:**
```bash
anchor build
cargo test -p solfx-core --test risk_engine --test scenarios
cargo test -p solfx-core --lib   # session state machine
```

## unit: session state machine — `programs/solfx-core/src/instructions/keeper/session.rs` (21)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `the_week_starts_on_sunday` | The week starts on sunday. | — |
| 2 | `the_interbank_week_is_closed_all_saturday` | The measurement that ended the weekend product, as a calendar assertion. | — |
| 3 | `the_interbank_week_opens_sunday_evening` | The interbank week opens sunday evening. | — |
| 4 | `the_interbank_week_closes_friday_evening` | The interbank week closes friday evening. | — |
| 5 | `midweek_is_open` | Midweek is open. | — |
| 6 | `an_em_market_follows_its_own_local_hours` | Phase 0b measured USD/BRL trading 14:00–21:00 UTC — nothing like the interbank week. | — |
| 7 | `seconds_until_close_counts_down_within_the_session` | Seconds until close counts down within the session. | — |
| 8 | `a_closed_market_reports_no_countdown` | A closed market reports no countdown. | — |
| 9 | `a_degenerate_calendar_is_always_open_not_never_open` | A configuration mistake must not brick a market permanently. | — |
| 10 | `an_open_market_stays_active` | An open market stays active. | — |
| 11 | `approaching_the_close_goes_reduce_only` | Approaching the close goes reduce only. | — |
| 12 | `a_dead_feed_halts_the_market` | The C-1 test. | — |
| 13 | `a_live_feed_outside_session_hours_still_halts` | The other half of C-1: a live feed outside session hours still halts. | — |
| 14 | `reopening_goes_through_the_gap_window_not_straight_to_active` | Reopening goes through the gap window not straight to active. | — |
| 15 | `the_gap_window_clears_once_its_time_is_up` | The gap window clears once its time is up. | — |
| 16 | `the_gap_window_length_is_independent_of_crank_cadence` | The window's length must not depend on how often keepers happen to run. | — |
| 17 | `a_halted_market_does_not_reopen_while_either_signal_is_bad` | A halted market does not reopen while either signal is bad. | — |
| 18 | `crypto_never_leaves_active_while_its_feed_lives` | Crypto never leaves active while its feed lives. | — |
| 19 | `even_crypto_halts_when_its_feed_stops` | 24/7 is a property of the feed, never a promise the protocol makes on its behalf. | — |
| 20 | `the_continuous_regime_derates_then_converges_then_reopens` | Unreachable in production — Phase 0b found no continuous metals feed, and initialize_market rejects the variant. | — |
| 21 | `a_continuous_market_with_a_stale_index_halts` | A continuous market with a stale index halts. | — |

## compute-unit regression — `programs/solfx-core/tests/compute_budget.rs` (1)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `risk_engine_instructions_stay_within_their_ceilings` | The risk engine (ARCHITECTURE.md § 6.7, § 6.8). | — |

## integration: risk engine — `programs/solfx-core/tests/risk_engine.rs` (23)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_healthy_position_cannot_be_liquidated` | A healthy position must not be liquidatable. | — |
| 2 | `an_underwater_long_is_liquidated_and_the_liquidator_is_paid` | $10,854 of notional on $150 of margin is ~72x. | — |
| 3 | `an_underwater_short_is_liquidated` | A short is liquidated by a rise — the mirror. | — |
| 4 | `the_liquidation_penalty_is_split_three_ways` | The penalty splits liquidator 40% / insurance 40% / treasury 20% (§ 6.8 step 6). | — |
| 5 | `a_halted_market_still_permits_liquidation` | An underwater position must stay liquidatable while the market is halted. | — |
| 6 | `self_liquidation_is_not_profitable` | Threat T8: liquidating your own position must never be profitable. | — |
| 7 | `the_insurance_fund_absorbs_bad_debt` | A gap straight through the liquidation price leaves the position owing more than its margin. | — |
| 8 | `an_empty_insurance_fund_queues_the_shortfall_for_adl` | With the insurance fund empty, the shortfall queues for auto-deleveraging instead. | — |
| 9 | `auto_deleveraging_socialises_a_shortfall_onto_a_winner` | ADL takes profit from a winner, never principal, and only while a shortfall is unpaid. | — |
| 10 | `auto_deleveraging_is_rejected_when_nothing_is_owed` | Auto deleveraging is rejected when nothing is owed. | — |
| 11 | `anyone_can_top_up_the_insurance_fund` | Anyone can top up the insurance fund. | — |
| 12 | `carry_accrues_over_time` | Carry is interest on borrowed notional, charged in both directions. | — |
| 13 | `funding_is_zero_on_a_one_sided_book` | Funding is zero while only one side of the book is populated. | — |
| 14 | `a_skewed_book_makes_the_heavy_side_pay` | With both sides populated and the book skewed long, longs pay and shorts receive. | — |
| 15 | `funding_never_touches_lp_capital` | Funding must never touch LP capital (invariant I3). | — |
| 16 | `a_repeated_funding_crank_in_the_same_second_is_a_no_op` | Redundant keepers are the design (§ 9.2 wants three instances). | — |
| 17 | `carry_is_charged_when_the_position_closes` | Carry is settled out of the position at close, and it is revenue: it reaches the LP vault and the treasury rather than staying with the trader. | — |
| 18 | `a_dead_feed_halts_the_market_and_blocks_new_positions` | The C-1 test, end to end. | — |
| 19 | `a_live_feed_on_a_saturday_still_halts_the_market` | A live feed outside session hours still halts. | — |
| 20 | `a_market_walks_the_full_weekly_cycle` | The full weekly cycle: open midweek, wind down before the close, halt over the weekend, reopen through a gap window, then resume. | — |
| 21 | `an_em_market_halts_outside_its_local_hours` | Phase 0b measured USD/BRL trading 14:00–21:00 UTC. | — |
| 22 | `a_halted_market_freezes_voluntary_exits_but_not_liquidation` | § 7.3 defines Halted as *"nothing but liquidations of already-underwater positions. | `Market is halted` |
| 23 | `a_crypto_market_ignores_the_calendar_but_not_its_feed` | Crypto has no session, so it never halts on the calendar — only on its feed. | — |

## scenario replay — `programs/solfx-core/tests/scenarios.rs` (7)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `chf_depeg_the_engine_stays_solvent_and_correct` ◆ | CHF depeg, 15 January 2015. | — |
| 2 | `chf_depeg_without_an_insurance_fund_queues_the_shortfall_instead` ◆ | The same event with the insurance fund empty — the configuration § 6.9 warns against. | — |
| 3 | `gbp_flash_crash_the_deviation_breaker_halts_rather_than_trading_the_wick` ◆ | GBP/USD fell ~6% in two minutes and recovered most of it within the hour. | — |
| 4 | `the_weekend_stale_price_exploit_is_refused` | The C-1 exploit, attempted end to end. | — |
| 5 | `a_weekend_gap_is_handled_through_the_gap_window` | Monday's gap arrives while positions are frozen through the weekend. | — |
| 6 | `an_em_market_is_shut_outside_local_hours_while_majors_keep_trading` | An emerging-market pair whose local exchange is shut while the interbank week runs on. | — |
| 7 | `a_local_holiday_halts_the_market_even_though_the_calendar_says_open` | A local holiday, which is where the two signals earn their keep. | — |

◆ = permanent regression test for a defect the CHF-depeg replay found.
