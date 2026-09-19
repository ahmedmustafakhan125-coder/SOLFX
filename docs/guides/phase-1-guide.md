# Phase 1 Guide — `solfx-math`: the Financial Engine

**Flowchart:** [`../diagrams/phase-1.dot`](../diagrams/phase-1.dot)
**Deliverable:** `crates/solfx-math/` — a pure Rust crate: no floats, no panics, no I/O, no
Anchor. Every value that ever touches a trader's money is computed here.
**Exit criteria:** `cargo test` green; CI on PR; 100% function coverage on the math;
**the round-trip-never-profits property passes**. All met.

---

## 1. Why a separate crate

Three properties fall out of the split, and all three are load-bearing:

1. **Testable without the BPF toolchain.** The whole suite runs in under a second on the
   host, which is what makes 100,000-case property runs practical in CI.
2. **The generic-engine constraint is structural** (ARCHITECTURE § 5.6): this crate cannot
   reference a `Market` or a `Position` because it does not depend on the program. No
   FX-specific assumption can leak into the maths.
3. **One audit surface.** When auditors ask "where can rounding go wrong?", the answer is
   one module (`fixed.rs`) with five functions.

---

## 2. `constants.rs` — the fixed-point scales

Everything is integer arithmetic at fixed scales. Getting these wrong by one power of ten
mis-prices everything, so each constant carries a compile-time assertion or a headroom test.

| Constant | Value | Meaning |
|---|---|---|
| `QUOTE_PRECISION` | 1e6 | USDC native decimals. All collateral, fees, PnL. `1_000_000` = $1.00 |
| `BASE_PRECISION` | 1e9 | Base-currency units. Position size is stored here, never in lots |
| `PRICE_PRECISION` | 1e9 | Quote per 1 base. EUR/USD 1.08543 → `1_085_430_000` |
| `RATE_PRECISION` | 1e9 | Fees/funding/carry rates. Exists because 0.8 bps is **not expressible** in whole bps — and 0.8 bps is a published fee tier |
| `BPS_PRECISION` | 1e4 | Basis points. 10,000 = 100% |
| `NOTIONAL_DIVISOR` | 1e12 | `BASE × PRICE / QUOTE` — converts (base × price) into USDC. Pinned by `const` assertion so it can never drift from its derivation |
| `LOT_SIZE_BASE` | 1e14 | 1 standard lot = 100,000 base units. **Presentation only** — the engine works in base units; the UI divides for display |
| `MIN_NOTIONAL_QUOTE` | 1e6 ($1) | Below this, a 1 bp fee rounds to zero and a round trip can cost nothing — the rounding-loop drain (threat T14). Floor enforced by `pricing::validate_notional` |
| `SECONDS_PER_HOUR` | 3600 | Funding/carry accrue per hour |

**Why 1e9 for price:** one pip (0.0001) = 100,000 units → 10,000 ticks of sub-pip
resolution, so rounding never dominates a 1 bp fee; and USD/JPY at 157 still leaves eleven
orders of magnitude of headroom inside `i64`.

---

## 3. `error.rs` & `types.rs` — the vocabulary

**`MathError`** — nine named failures (`Overflow`, `DivideByZero`, `InvalidPrice`,
`ConfidenceTooWide`, `NotionalTooSmall`, `InvalidParameter`, `PriceTooStale`,
`PriceFromFuture`, `DeviationTooLarge`). No `unwrap`, no `panic` anywhere in the crate — a
failed computation is a *named refusal* the frontend can render, never a crashed
transaction. A unit test asserts every variant renders a distinct message.

**`Direction`** — `Long | Short`, with `sign()` → `+1 | −1`: the multiplier that turns a
price delta into signed PnL.

**`TradeAction`** — `Open | Close`.

**`Side::resolve(direction, action)`** — the four-way mapping that guarantees the trader
always crosses the *adverse* side of the spread:

| | Open | Close |
|---|---|---|
| **Long** | Buy (pay the ask) | Sell (hit the bid) |
| **Short** | Sell (hit the bid) | Buy (pay the ask) |

Open and close always land on opposite sides, so a round trip can never dodge the spread —
one of the two mechanics behind the flagship property test.

**`QuoteConversion`** — correction C-3 as a type: `None` (USD-quoted, the fast path),
`QuotePerUsd` (feed quotes *quote per 1 USD* — USD/JPY 157, USD/INR 88: **divide** by the
rate), `UsdPerQuote` (EUR/USD-style: **multiply**). Storing the direction next to the feed
is what stops a call site guessing whether to multiply or divide.

---

## 4. `fixed.rs` — the rounding core

The only module allowed raw `/` and `%`. Everything else calls these, so there is exactly
one place to audit for rounding bugs.

**The rule: every rounding favours the protocol.**

| Function | Rounds | Use for |
|---|---|---|
| `mul_div_floor(a,b,d)` | down | payouts to the trader |
| `mul_div_ceil(a,b,d)` | up | fees, margin requirements, any cost charged |
| `mul_div_floor_signed(a,b,d)` | toward **−∞** | signed trader **PnL** |
| `mul_div_ceil_signed(a,b,d)` | toward **+∞** | signed *cost rates* charged to the trader |
| `div_ceil` / `div_floor` | up / down | plain division with named direction |

**Why signed PnL needs its own function** — the bug this module exists to prevent:
Rust's `/` truncates toward zero, so `-7 / 2 == -3`. A trader owing 3.5 units would be
charged 3 — the protocol eating the remainder **on every losing position**, forever.
Flooring toward −∞ (`-4`) shrinks gains *and* grows losses: protocol-favourable in both
signs. Its mirror `mul_div_ceil_signed` is for costs (a charge grows, a credit shrinks);
the two must never be interchanged, and the doc comments say which is which.

**Checked narrowing:** `to_u64`, `to_i64`, `i128_to_u128`, `u128_to_i128` fail loudly
instead of truncating. `positive_to_u128` additionally *rejects zero and negatives* — so
"price must be positive" is enforced by the type conversion itself rather than by a check a
caller could forget. `abs_i128` handles `i128::MIN` without panicking; `clamp_symmetric`
bounds funding rates into `[−cap, +cap]`.

---

## 5. `pnl.rs` — notional and profit

### `notional_in_quote(size_base, price)`

```
notional = size_base × price / 1e12          (NOTIONAL_DIVISOR)
```

Worked: 1 lot EUR/USD at 1.08543 → `1e14 × 1.08543e9 / 1e12` = `108_543_000_000` quote
units = **$108,543.00**. This is the number margin, fees and caps are all computed on.

### `upnl_in_quote(size, entry, exit, direction)`

```
uPnL = sign(direction) × size × (exit − entry) / 1e12     — floored toward −∞
```

Worked: long 1 lot, entry 1.08543, exit 1.08643 (+10 pips) → **+$100.00** — the classic
"$10 per pip per lot" falls straight out of the scales.

### `convert_pnl_to_collateral(pnl_quote, conversion, rate)` — correction C-3

The result above is in the **quote currency**. For EUR/USD that is USD and nothing more is
needed. For USD/INR it is *Rupees*:

```
QuotePerUsd:  pnl_usdc = pnl_quote × 1e9 / rate     (rate = quote units per USD)
UsdPerQuote:  pnl_usdc = pnl_quote × rate / 1e9
None:         identity
```

Worked: +₹8,842 on USD/INR with rate 88.42 → 8,842 / 88.42 = **+$100.00**. Skipping this
step books ₹8,842 *as* $8,842 — an 88× error, on every position, on nearly every listable
market (most are `USD/XXX` or `XXX/JPY`). Rounding: **floored as trader value** in the
gain direction, so conversion can never manufacture a profit.

`convert_cost_to_collateral` is the same conversion for unsigned costs (notional, fees),
rounding **up** — a converted charge grows.

`notional_in_collateral` = `notional_in_quote` + `convert_cost_to_collateral` in one call —
what margin checks actually use on non-USD-quoted markets.

### `weighted_entry_price(old_size, old_entry, add_size, fill)`

For `increase_position`:

```
new_entry = (old_size × old_entry + add_size × fill) / (old_size + add_size)
```

Rounded adversely to the trader (up for longs, down for shorts) so averaging in can never
create free PnL. Property-tested to stay within `[min, max]` of the two inputs.

### `proportional_pnl(total_pnl, closed_size, total_size)`

Partial closes realise `total × closed / size`, floored — so closing in slices can never
beat closing whole (also a property test).

---

## 6. `margin.rs` — solvency

### Requirements

```
initial_margin      = notional × imr_bps / 1e4      (ceil)   — to open
maintenance_margin  = notional × mmr_eff / 1e4      (ceil)   — to stay open
mmr_eff             = mmr_bps × mmr_multiplier_bps(notional) / 1e4
```

`mmr_multiplier_bps` is the **size tier** (§ 6.3): <$100k → 1.0x, $100k–1M → 1.5x,
$1M–5M → 2.5x, >$5M → 4.0x. A $5M position is not the same risk as fifty $100k positions —
it exits through the same spread all at once, so it must hold proportionally more margin.

`effective_leverage(notional, collateral)` = `notional/collateral` (ceil — rounding *up*
makes the leverage check strictly harder to pass).

### `equity(collateral, upnl, AccruedCosts { carry, funding, close_fee })`

```
equity = collateral + uPnL − carry − funding − close_fee_est
```

The v0 formula omitted accrued costs; at 50x the day's carry can be the difference between
solvent and not. `AccruedCosts` exists so no call site can forget a term — you construct
the struct or you don't call the function.

### `is_liquidatable(equity, maintenance_margin)` — **strict** `<`

Equity exactly equal to the requirement is *not* liquidatable. A `<=` here is liquidation
griefing (threat T7): an attacker liquidates positions sitting on the boundary and collects
a penalty from a solvent trader. `health_factor_bps` (10,000 = 1.0) exists for display
only; code branches on `is_liquidatable`, never on the ratio, to avoid a divide-by-zero
branch and a rounding disagreement.

### `liquidation_price(entry, collateral, size, mmr_eff, direction)`

The number the trader actually watches. Derived by solving `equity = maintenance` for
price; rounded **conservatively** (the displayed price sits at or before the true trigger,
never after), and clamped at 1 unit for already-doomed positions. Property test: the
displayed price is always reached at-or-before the real liquidation condition.

---

## 7. `pricing.rs` — the anti-toxic-flow layer (§ 6.6)

The most important module in the protocol. If execution pricing is wrong, the vault leaks
continuously and invisibly.

```
conf_bps      = conf × 1e4 / price                       (ceil — a band never rounds to free)
                hard reject if conf_bps > market ceiling  (C-4)
conf_spread   = conf_bps × multiplier / 1e4
skew_impact   = rate × (|skew_after| − |skew_before|) / SCALE    (only when worsening)
total_spread  = base_spread + conf_spread + max(skew_impact, 0)

execution_price:
  Buy  side:  oracle × (1 + total_spread/1e4)     — pay the ask
  Sell side:  oracle × (1 − total_spread/1e4)     — hit the bid
```

What each term buys:

- **`conf_spread`** — the protocol charges more *precisely when it knows less*. During a
  news event Pyth's band blows out; the spread widens automatically instead of the vault
  eating the uncertainty. This is the direct answer to C-4/T2 — latency arbitrage killed
  several GMX forks.
- **`skew_impact`** (`skew_delta` → `skew_impact_bps`) — pushing the book further one-sided
  costs progressively more; trades that *reduce* `|skew|` pay nothing extra. The pool's
  primary inventory-management lever (ADR-001 makes the pool the counterparty, so aggregate
  skew is the pool's directional risk).
- **Adverse-side selection** — removes the free half-spread option.

Guards: `validate_confidence` (the hard reject), `validate_notional` (the $1 floor, T14),
`validate_slippage` (buy: `exec ≤ bound`; sell: `exec ≥ bound` — the trader's protection,
pointing the opposite way from the protocol's), `price_delta_to_pips` (display only).

**Minimum hold** (§ 6.6's second requirement) is enforced in Phase 3's close path via
`Position.opened_at_slot` + `Market.min_hold_slots`: a same-slot round trip must never be
able to scalp a spread narrower than the true market's.

---

## 8. `fees.rs` — revenue and its routing

### `fee_rate_for_volume(thirty_day_volume)` — § 8.2's schedule

<$1M → 1.5 bps · <$10M → 1.2 · <$50M → 1.0 · <$250M → 0.8 · above → 0.6 bps per side.
Returned at `RATE_PRECISION` (1 bp = 100,000) — the reason the finer scale exists.

### `fee_amount(notional, rate)`

```
fee = notional × rate / 1e9      (ceil, minimum 1 unit if notional > 0)
```

The 1-unit floor is the second line of defence against T14: a trade small enough for its
fee to round to zero could be repeated forever at no cost.

### `FeeSplitBps` / `split_fee` — § 8.3

Launch split **LP 55 / treasury 25 / insurance 10 / referral 10** (validated to sum to
exactly 10,000 — fees can neither vanish nor over-allocate). Mechanics: LP, insurance and
referral shares floor; **the treasury takes the remainder**, making the split exactly
conservative *by construction* — the rounding dust accrues to the protocol rather than
leaking. Under-paying LPs is the most common death of pool-backed perps, so the code also
refuses any split where treasury ≥ LP.

### `split_liquidation_penalty` — § 6.8

Liquidator 40 / insurance 40 / treasury 20, same remainder pattern. A cross-module property
test proves the liquidator's cut on any ≥$10k position exceeds transaction cost — *an
unprofitable liquidation is an unliquidated position*.

---

## 9. `funding.rs` — carry and funding (two mechanisms, never conflated)

### The cumulative-index model (why O(1))

You cannot iterate every open position on-chain. Instead the market accumulates an index;
each position snapshots it at entry; the difference is what that position owes:

```
owed = basis × (index_now − index_at_entry) / PRECISION
```

`accrued_carry` uses the position's **entry notional** as basis (carry is interest on
borrowed notional); `accrued_funding` uses **base size** (matching the units the funding
index accrues in).

### Carry — the FX swap (§ 6.7), and the transparency feature

```
carry_cost/hour = (quote_rate − base_rate) / 8760  +  markup/hour     (ceil-signed)
```

Sign convention (worked in FOREX-EXPLAINED § 9): long EUR/USD holds EUR (earning 2%) and
borrows USD (paying 4%) → cost 2%/yr ≈ $5.95/day per lot. The **markup is added to both
directions** — that is the revenue — and is stored *separately* from the two central-bank
rates so the UI can show `differential` and `markup` as independent line items. That split
is the concrete improvement over XM/Exness opacity and one of the product's three pillars.
`advance_carry_index` clamps at zero rather than running backwards, so no side is ever
charged less than the markup.

### Funding — skew correction between traders (invariant I3)

```
skew_ratio  = (oi_long − oi_short) / max(oi_long + oi_short, 1)      ∈ [−1e9, 1e9]
rate/hour   = clamp(skew_ratio × k, −cap, +cap)
```

`funding_index_update` moves the two indices in opposite directions, with the **receiving
side's rate scaled by the OI ratio** — on an imbalanced book the heavy side's payments
exactly fund the light side's receipts, so funding **nets to zero and never touches LP
capital**. That is invariant I3, and it holds by construction rather than by check.
`utilisation_ratio(exposure, aum)` supports the § 7.2 breaker added in Phase 5.

---

## 10. What the property tests pin down

38 properties (grown to 56 by Phase 5), run at 4,096 cases locally and 100,000 in CI's deep
run. The two worth naming here:

- **`round_trip_at_the_same_price_always_loses`** — open then immediately close at an
  unchanged oracle price, across the full space of sizes, prices, spreads, fee tiers and
  both directions: the trader always ends with less. If this ever fails there is free money
  and bots will drain the vault. A twin runs the same property through the C-3 conversion
  path.
- **`pnl_is_antisymmetric`** — a long and short of identical parameters sum to ≤ 0 within
  one rounding unit: the matched pair can never *create* value out of rounding.

Full inventory: [`../test-cases/phase-1-tests.md`](../test-cases/phase-1-tests.md)
