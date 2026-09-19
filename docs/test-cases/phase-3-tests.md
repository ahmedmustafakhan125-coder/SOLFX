# Phase 3 — Test Cases (31 tests)

**Guide:** [`../guides/phase-3-guide.md`](../guides/phase-3-guide.md) · **Flowchart:** [`architecture/phase-3.png`](../../architecture/phase-3.png)

Phase 3's suite drives the full position lifecycle through the real binary: open,
increase, decrease, close, margin adjustment — on a direct pair, a synthetic cross and a
non-USD-quoted (C-3) market, in both directions. Invariants I1–I5 are re-asserted after
every instruction; the settlement layer's conservation (`Flows::is_conservative`) has its
own unit tests because it is checked *before* any token moves.

The on-chain twin of the flagship property — `a_round_trip_at_an_unchanged_price_always_loses`
— runs against every market shape, proving the property survives account plumbing, CPIs and
fee routing, not just the arithmetic.

**Run:**
```bash
anchor build
cargo test -p solfx-core --test positions
```

## unit: settlement flows — `programs/solfx-core/src/instructions/trader/flows.rs` (5)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_fee_with_no_pnl_leaves_the_trader_and_lands_in_the_vaults` | A fee with no pnl leaves the trader and lands in the vaults. | — |
| 2 | `a_winning_trade_draws_from_the_pool` | The B-book mechanic: a winning trader is paid by the pool. | — |
| 3 | `a_losing_trade_pays_the_pool` | A losing trade pays the pool. | — |
| 4 | `conservation_holds_across_awkward_fee_sizes` | Rounding must not create or destroy a unit. | — |
| 5 | `a_zero_fee_still_settles_pnl` | A zero fee still settles pnl. | — |

## integration: position lifecycle — `programs/solfx-core/tests/positions.rs` (26)

| # | Test case | Verifies | Expected rejection |
|---:|---|---|---|
| 1 | `a_long_opens_with_the_expected_entry_margin_and_fee` | A long opens with the expected entry margin and fee. | — |
| 2 | `a_short_fills_below_the_mid` | Opening a short is a sell, so it fills below the mid — the mirror of the long. | — |
| 3 | `a_winning_long_is_paid_by_the_pool` | A long that is right: the pool pays. | — |
| 4 | `a_losing_long_pays_the_pool` | A long that is wrong: the pool receives. | — |
| 5 | `a_round_trip_at_an_unchanged_price_always_loses` | Open and immediately close at an unchanged oracle price must always lose money. | — |
| 6 | `a_non_usd_quoted_market_sizes_in_usdc_not_in_quote_currency` | USD/INR is the shape that makes C-3 mandatory. | — |
| 7 | `a_round_trip_on_a_converted_market_also_loses` | A round trip on a converted market must lose money too. | — |
| 8 | `a_synthetic_cross_completes_a_full_lifecycle` | EUR/GBP composed from EUR/USD ÷ GBP/USD. | — |
| 9 | `increasing_a_position_moves_the_entry_price_against_the_trader` | Increasing a position moves the entry price against the trader. | — |
| 10 | `a_partial_close_realises_a_proportional_share` | A partial close realises a proportional share. | — |
| 11 | `margin_can_be_added_and_removed` | Margin can be added and removed. | — |
| 12 | `margin_cannot_be_removed_below_the_initial_requirement` | Removing margin must leave the position meeting the *initial* margin requirement, not merely the maintenance one — otherwise a trader could walk a position to the edge of liquidation and leave it there. | — |
| 13 | `a_slippage_bound_rejects_a_worse_fill` | A slippage bound rejects a worse fill. | — |
| 14 | `leverage_above_the_market_cap_is_rejected` | Leverage above the market cap is rejected. | — |
| 15 | `opening_is_blocked_while_the_protocol_is_paused` | Opening is blocked while the protocol is paused. | — |
| 16 | `a_halted_market_still_permits_closing` | A halted market must still let a trader out. | — |
| 17 | `a_minimum_hold_time_blocks_a_same_slot_round_trip` | § 6.6's minimum hold. | — |
| 18 | `a_position_cannot_be_opened_on_a_stale_price` | A position cannot be opened on a stale price. | — |
| 19 | `a_wide_confidence_band_blocks_opening_a_position` | Correction C-4 on the trading path. | — |
| 20 | `another_trader_cannot_close_your_position` | Another trader cannot close your position. | — |
| 21 | `open_interest_tracks_positions_on_both_sides` | Open interest tracks positions on both sides. | — |
| 22 | `the_open_interest_cap_rejects_an_oversized_book` | The open interest cap rejects an oversized book. | — |
| 23 | `invariants_hold_across_a_busy_multi_market_session` | The invariant sweep across a busy protocol: two traders, two markets, both directions, partial and full closes, with everything re-checked after each step. | — |
| 24 | `one_trader_can_hold_several_positions_in_one_market` | A trader can hold several positions in the same market, and each is independently collateralised (ADR-004). | — |
| 25 | `the_first_liquidity_deposit_mints_one_to_one` | The first liquidity deposit mints one to one. | — |
| 26 | `lp_supply_and_aum_are_non_zero_together` | I8: LP supply and AUM are non-zero together. | — |
