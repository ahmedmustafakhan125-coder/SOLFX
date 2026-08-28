# Devnet-blocker fixes — C-2, B-2, B-1

Companion to [`AUDIT.md`](../AUDIT.md). These are the three fixes agreed as devnet blockers.

> **Status: applied, compiled, and verified. 2026-08-28.**
>
> - `cargo fmt --all` — clean
> - `cargo clippy --workspace --all-targets` — clean (workspace denies `arithmetic_side_effects`,
>   `unwrap_used`, `float_arithmetic`, the lossy casts, and the rest)
> - `anchor build` — `solfx_core.so` rebuilt, so LiteSVM tests the changed program
> - `cargo test --workspace --no-fail-fast` — **532 passed, 0 failed, 1 ignored**
>   (baseline was 531; +1 new passing test, +1 deliberately ignored C-1 regression test)
> - `program_autofixer` — clean over the changed settlement arithmetic. One medium
>   `unchecked-arithmetic` on the pre-existing `Flows::is_conservative`, dismissed with
>   evidence: the four operands are `i64`/`u64` widened by `i128::from` before any addition,
>   bounding the sum at ~3.7e19 against an i128 domain of ~1.7e38.
>
> **The C-2 fix was verified by reverting it.** With only the two-line C-2 hunk backed out and
> the program rebuilt, `carry_is_settled_when_a_position_is_liquidated` fails inside
> `assert_invariants` with:
>
> ```
> I1 broken: vault holds 49788941220, accounts sum to 49782991626
> ```
>
> A difference of **5,949,594** — $5.95, exactly one day of carry at 20%/yr on that position's
> notional, which is what the trace below predicts. The test has teeth; it is not passing by
> construction.

All three share one root cause: **`risk::settle_carry` reduces `position.collateral` but moves
no tokens and does not touch `Protocol::total_user_collateral`.** It relies entirely on its
caller routing exactly `carry` back out through the fee path. One caller clamps it away (C-2),
one caller does not exist (B-1), and the caller that does route it sends it through the wrong
split (B-2).

---

## C-2 — carry stranded in the collateral vault on liquidation

**File:** `programs/solfx-core/src/instructions/keeper/liquidate.rs`, at line 182.

### Replace

```rust
    let collateral = position.collateral;
    let equity = health.liquidation_equity;
```

### With

```rust
    // `settle_carry` above removed the accrued carry from `position.collateral`, but it moved
    // no tokens — they are still sitting in the collateral vault, and they are still counted
    // in `Protocol::total_user_collateral`. The distribution below must therefore be based on
    // the **pre-carry** balance, or exactly `carry` units are stranded in the vault owned by
    // nobody, and invariant I1 drifts by that amount on every liquidation.
    //
    // `equity` deliberately stays on the post-carry figure: carry is a real cost the position
    // has borne, so it must reduce what the trader gets back. The two are not in tension —
    // the carry leaves trader ownership here, and `routed_fee` below is where it lands.
    let collateral = position
        .collateral
        .checked_add(carry)
        .ok_or(SolfxError::MathOverflow)?;
    let equity = health.liquidation_equity;
```

### Why this balances

Trace from `AUDIT.md` C-2 — collateral 100, carry 10, uPnL 0, `full_penalty` 5:

| | Before fix | After fix |
|---|---:|---:|
| `given_up` = collateral − credit | 90 − 85 = **5** | 100 − 85 = **15** |
| `liquidator_out` | 2 | 2 |
| `routed_fee` (clamped at `after_liquidator`) | min(13, 3) = **3** | min(13, 13) = **13** |
| `pool_in` | 0 | 0 |
| **Total distributed** | 2+3+0 = **5** | 2+13+0 = **15** |
| Trader's claim change | 100 → 85 = **−15** | 100 → 85 = **−15** |
| **Discrepancy** | **10 (= carry)** | **0** |

The clamp also stops binding in the normal case, because `given_up` now genuinely contains the
carry it is being asked to pay out.

---

## B-2 — liquidation penalty re-split through the trading-fee schedule

**Files:** `programs/solfx-core/src/instructions/trader/flows.rs` and
`programs/solfx-core/src/instructions/keeper/liquidate.rs`.

### Step 1 — `flows.rs`: separate allocation from flow construction

Replace the body of `compute_flows` with a thin wrapper and add the new entry point:

```rust
/// Build flows from an allocation that has **already been decided**.
///
/// [`compute_flows`] derives the allocation from the protocol's trading-fee split, which is
/// correct for a trading fee. A liquidation penalty has its own split (§ 6.8: liquidator 40 /
/// insurance 40 / treasury 20) and must not be re-divided by the trading schedule — doing so
/// sends 55% of the insurance fund's share to the LP pool instead. That path decides its own
/// allocation and calls this.
pub fn flows_for_allocation(alloc: FeeAllocation, pnl: i64) -> Result<Flows> {
    let fee = alloc.total().or_program_err()?;

    let lp_delta = i64::try_from(
        i128::from(alloc.lp)
            .checked_sub(i128::from(pnl))
            .ok_or(SolfxError::MathOverflow)?,
    )
    .map_err(|_| SolfxError::MathOverflow)?;

    let fee_vault_in = alloc
        .treasury
        .checked_add(alloc.referral)
        .ok_or(SolfxError::MathOverflow)?;

    let collateral_delta = i64::try_from(
        i128::from(pnl)
            .checked_sub(i128::from(fee))
            .ok_or(SolfxError::MathOverflow)?,
    )
    .map_err(|_| SolfxError::MathOverflow)?;

    let flows = Flows {
        lp_delta,
        insurance_in: alloc.insurance,
        fee_vault_in,
        collateral_delta,
    };

    // Not a debug assertion. If this ever fails, USDC has been created or destroyed, and the
    // correct response is to fail the transaction rather than let invariant I7 find it later.
    require!(flows.is_conservative(), SolfxError::MathOverflow);

    Ok(flows)
}

pub fn compute_flows(fee: u64, pnl: i64, split: FeeSplitBps) -> Result<(FeeAllocation, Flows)> {
    let alloc = split_fee(fee, split).or_program_err()?;
    let flows = flows_for_allocation(alloc, pnl)?;
    Ok((alloc, flows))
}
```

The existing `flows.rs` unit tests are unchanged and still cover `compute_flows`.

### Step 2 — `liquidate.rs`: route the penalty where §6.8 says

Replace the `routed_fee` / `pool_in` block (lines 218–225):

```rust
    // Carry is ordinary revenue and takes the standard four-way split. The penalty's
    // non-liquidator shares are not: § 6.8 fixes them at insurance 40% / treasury 20% of the
    // penalty, and passing them through `split_fee` would re-divide them by the trading
    // schedule — which is how the insurance fund ends up with 6% of a penalty instead of 40%.
    //
    // Carry is paid first when the position cannot cover everything: it is a debt already
    // incurred, where the penalty is a charge levied now.
    let carry_routed = carry.min(after_liquidator);
    let penalty_budget = after_liquidator.saturating_sub(carry_routed);
    let insurance_share = split.insurance.min(penalty_budget);
    let treasury_share = split
        .treasury
        .min(penalty_budget.saturating_sub(insurance_share));

    let routed_fee = carry_routed
        .checked_add(insurance_share)
        .ok_or(SolfxError::MathOverflow)?
        .checked_add(treasury_share)
        .ok_or(SolfxError::MathOverflow)?;

    let pool_in = after_liquidator.saturating_sub(routed_fee);
```

And replace the `compute_flows` call (lines 261–265):

```rust
    let settlement_pnl = -i64::try_from(pool_in).map_err(|_| SolfxError::MathOverflow)?;

    // The carry portion splits the normal way; the penalty portion goes where § 6.8 sends it.
    let carry_alloc =
        fees::split_fee(carry_routed, ctx.accounts.protocol.fee_split()).or_program_err()?;
    let alloc = fees::FeeAllocation {
        lp: carry_alloc.lp,
        treasury: carry_alloc
            .treasury
            .checked_add(treasury_share)
            .ok_or(SolfxError::MathOverflow)?,
        insurance: carry_alloc
            .insurance
            .checked_add(insurance_share)
            .ok_or(SolfxError::MathOverflow)?,
        referral: carry_alloc.referral,
    };
    let flows = flows_for_allocation(alloc, settlement_pnl)?;
```

Import change: `use crate::instructions::trader::flows::flows_for_allocation;` replaces the
`compute_flows` import.

### Conservation check

`alloc.total()` = `carry_alloc.total()` + `treasury_share` + `insurance_share`
= `carry_routed + insurance_share + treasury_share` = `routed_fee`.

So the `fee` computed inside `flows_for_allocation` equals the `routed_fee` passed to
`SettlementInput`, and `is_conservative()` holds by the same argument as before.

### Resulting split of a penalty

| Destination | §6.8 | Before | After |
|---|---:|---:|---:|
| Liquidator | 40% | 40% | 40% |
| Insurance | 40% | 6% | **40%** |
| Treasury | 20% | 15% | **20%** |
| LP pool | — | 33% | 0% |
| Referral | — | 6% | 0% |

---

## B-1 — carry never charged on a voluntary close

**File:** `programs/solfx-core/src/instructions/trader/close_position.rs`, inside `reduce()`.

### Step 1 — settle carry at the top of `reduce()`

After the `min_hold_slots` check and before the execution price is taken:

```rust
    // Carry accrued since entry (§ 6.7). Settled here, not only at liquidation: § 6.5 puts it
    // in the equity formula and § 8.1 makes it revenue stream 2, so a close path that skipped
    // it would let any position avoid carry entirely by never being liquidated.
    //
    // `settle_carry` removes it from `position.collateral` and re-snapshots the index, but
    // moves no tokens. Those tokens are still in the collateral vault, so `carry` is added
    // back into the distribution basis below and charged as part of the fee. The two cancel
    // in what the trader receives, while moving the value from trader ownership to revenue.
    let carry = crate::risk::settle_carry(position, market)?;
```

### Step 2 — split "released" into two quantities

The current single `released` does two jobs that are no longer the same number. Rename the
existing computation to `released_from_position` and derive the distribution basis from it:

```rust
    let released_from_position = if size_delta == position.size_base {
        position.collateral
    } else {
        u64::try_from(
            u128::from(position.collateral)
                .checked_mul(u128::from(size_delta))
                .ok_or(SolfxError::MathOverflow)?
                .checked_div(u128::from(position.size_base))
                .ok_or(SolfxError::DivideByZero)?,
        )
        .map_err(|_| SolfxError::MathOverflow)?
    };

    // Pre-carry basis: what actually leaves the position's claim on the vault.
    let released = released_from_position
        .checked_add(carry)
        .ok_or(SolfxError::MathOverflow)?;
```

### Step 3 — charge carry as part of the fee

The existing `fee` becomes `close_fee`, and the routed total adds carry:

```rust
    let close_fee =
        fees::fee_amount(notional, market.close_fee_rate.min(tier_rate)).or_program_err()?;
    let fee = close_fee.checked_add(carry).ok_or(SolfxError::MathOverflow)?;
```

`available`, `raw_equity`, `credit` and `bad_debt` are unchanged — they already read `released`
and `fee`, and the carry cancels:

```
available = released − fee
          = (released_from_position + carry) − (close_fee + carry)
          = released_from_position − close_fee
```

### Step 4 — subtract only the position's share

At the book-keeping step (currently line ~409), `position.collateral` is already post-carry, so
it must lose only `released_from_position`:

```rust
    position.collateral = position
        .collateral
        .checked_sub(released_from_position)
        .ok_or(SolfxError::MathOverflow)?;
```

### Why this balances

For a full close with pre-carry collateral `C`:

- Position claim: `C → 0`, i.e. `−C`
- User free collateral: `+credit` where `credit = released_from_position − close_fee + pnl`
- Net claim change: `credit − C = −carry − close_fee + pnl = pnl − fee`
- `settle` moves `collateral_delta = pnl − fee`

The two agree, so I1 holds. With `carry == 0` every quantity is identical to today's
behaviour, so no existing zero-carry test changes.

---

## Test changes

Four tests currently pass for the wrong reason and must be strengthened, plus one gap filled.
All in `programs/solfx-core/tests/risk_engine.rs`.

1. **`carry_is_charged_when_the_position_closes`** — currently satisfied by the close fee
   alone. Needs a zero-carry control: run the same scenario twice, once with
   `rate_quote_annual = 0`, and assert the difference in what comes back is the carry, not
   merely that something was deducted.
2. **`the_liquidation_penalty_is_split_three_ways`** — currently asserts only `> before` on
   each destination. Needs the actual proportions: liquidator 40%, insurance 40%, treasury 20%
   of `penalty`, read from the `PositionLiquidated` event.
3. **`self_liquidation_is_not_profitable`** — uses a position far above the $500 notional
   threshold, so the `MIN_LIQUIDATOR_REWARD` top-up never fires. Needs a second case at ~$50
   notional, which is the C-1 exposure band.
4. **New: `carry_is_settled_when_a_position_is_liquidated`** — the gap that hid C-2. Accrue
   carry, liquidate, call `assert_invariants()`. This test fails on today's code and passes
   after the C-2 fix, which is the point of it.

---

## Out of scope for this pass

- **C-1** (insurance-fund subsidy) — does not block devnet; the fund holds no real value
  there. Blocks mainnet.
- **B-3 … B-7** — revenue and configuration questions, better settled against real devnet
  usage.
- **`increase_position.rs` does not settle carry either.** Same shape as B-1. Lower impact,
  since carry keeps accruing on the enlarged position rather than being forgiven, but the
  weighted-entry basis and `cum_borrow_entry` interaction should be checked before mainnet.
- **`adl.rs`** shares C-2's `settle_carry` pattern at line 146 and needs the same treatment;
  it was not traced in detail.
