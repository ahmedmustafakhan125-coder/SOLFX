# SolFX — independent audit

**Date:** 2026-08-25 · **Scope:** `programs/solfx-core`, `programs/solfx-referral`, `crates/solfx-math`, and the configuration in `crates/solfx-keeper/src/bin/init_protocol.rs`
**Method:** read-only. Documentation pulled live through the Solana MCP server; financial formulas recomputed from first principles and compared against the code.
**Commit state:** audited against the working tree at `1a4f6e8` plus ~12 uncommitted files. The audit itself was read-only; the fixes recorded in the remediation table below were applied afterwards, as separate work.

---

## Remediation status — updated 2026-08-28

| Finding | Status |
|---|---|
| **C-1** insurance-fund subsidy | **fixed & verified** — the top-up is capped at the liquidator share of a *fully payable* penalty on the same position, so the fund never pays more than the position could have paid itself. Proven by reverting the fix: `self_liquidation_is_not_profitable_on_a_small_position` then fails on "a solvent liquidation must not draw down the insurance fund". |
| **C-2** carry stranded on liquidation | **fixed & verified** — `liquidate.rs` distributes on the pre-carry balance. Proven by reverting the fix: I1 breaks by exactly the accrued carry ($5.95). |
| **B-1** carry never charged on close | **fixed & verified** — `close_position.rs` settles carry and routes it as revenue. |
| **B-2** penalty not split 40/40/20 | **fixed & verified** — liquidator and insurance now receive an identical 40% (measured: 21,402,177 each of a 53,505,444 penalty). |
| **B-3 … B-7** | **open** — revenue and configuration questions, deferred to devnet observation. |

Suite after the fixes: **549 passed, 0 failed, 0 ignored**, clippy clean.
Details and the exact diffs in [`docs/audit-fixes.md`](docs/audit-fixes.md).

---

## 0. How this audit was run

### Context sources read

`docs/CONTEXT.md`, `docs/ARCHITECTURE.md` (§5–§7, §13), plus the phase reports and test catalogue. Claims in those documents were treated as scope, not as evidence — every "this is done" was checked against source before being audited.

Two documented claims were **verified true**:

- *"0 runtime `find_program_address` in program source"* — confirmed; all PDA derivation is via Anchor `seeds`/`bump` constraints.
- *"LP inflation attack closed — there is no donation path"* — confirmed. `LpPool.aum` ([`state/lp_pool.rs:23`](programs/solfx-core/src/state/lp_pool.rs#L23)) is a tracked field, never a token-balance read. A direct USDC transfer into `lp_vault` cannot move NAV per share.

One documented claim was **verified false** — see finding **B-1**: `docs/CONTEXT.md` and the test name `carry_is_charged_when_the_position_closes` both assert carry is charged on a voluntary close. It is not.

### MCP tools exposed, and what each was used for

The `solana-mcp` server exposes exactly **five** tools. All five were available; four were used.

| Tool | Used for in this audit |
|---|---|
| `list_sections` | Enumerating the doc corpus and picking source ids — established that `pyth-docs`, `anchor-docs`, `gh-anchor`, `gh-sealevel-attacks`, `solana-docs` and the peer-protocol sources (`gh-kamino-scope`, `gh-marginfi-v2`, `gh-drift-protocol-v2`) were the relevant ones. |
| `Solana_Documentation_Search` | The two substantive doc sweeps: (a) Pyth pull-oracle verification level and staleness practice; (b) Anchor account validation, `has_one`, seeds and canonical bump. Cited throughout §1. |
| `Solana_Expert__Ask_For_Help` | Resolving three specific version questions I could not settle from the code: whether `CpiContext::new_with_signer` takes a `Pubkey` or an `AccountInfo` in Anchor 1.x, whether `has_one` is deprecated, and the current `transfer` vs `transfer_checked` guidance. Cited in §3. |
| `program_autofixer` | Static security lint over the `LiquidatePosition` account-validation surface and its CPI helper. Result: clean — `{"issues":[],"suggestions":[],"require_another_tool_call_after_fixing":false}`. |
| `get_documentation` | **Not used.** The two semantic searches returned the specific passages needed at much lower token cost; pulling whole 50 KB sources would have added nothing to the verdicts below. Stated for completeness rather than omitted. |

### What "verified" means below

A verdict of *correct* appears only where I recomputed the arithmetic independently or matched the code against a cited doc passage. Anything I could not establish is marked **unverified** and left as a question, not a finding.

---

## 1. Critical security

### C-1 — [FIXED] The minimum-liquidator-reward subsidy is an economic drain on the insurance fund
**Severity: High** · [`instructions/keeper/liquidate.rs:233-241`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L233-L241), [`constants.rs:121`](programs/solfx-core/src/constants.rs#L121)

When a liquidated position cannot pay the liquidator, the insurance fund tops the reward up to `MIN_LIQUIDATOR_REWARD` = $1.00:

```rust
let reward_top_up = if liquidator_out < MIN_LIQUIDATOR_REWARD {
    MIN_LIQUIDATOR_REWARD.saturating_sub(liquidator_out)
} else { 0 };
```

The design rationale in `constants.rs` is sound in isolation — an unliquidated position is worse than a dollar of subsidy. The problem is the threshold relative to the configured parameters.

**Recomputed from the shipped configuration.** `liquidation_fee_bps = 50` (0.5%, [`init_protocol.rs:485`](crates/solfx-keeper/src/bin/init_protocol.rs#L485)) and the liquidator takes 40% of the penalty ([`fees.rs:165`](crates/solfx-math/src/fees.rs#L165)). So:

```
liquidator_out ≥ $1.00  ⟺  penalty × 0.40 ≥ $1.00
                        ⟺  penalty ≥ $2.50
                        ⟺  notional ≥ $500
```

**Every liquidation of a position under $500 notional draws a subsidy from the insurance fund.** The floor on position size is `MIN_NOTIONAL_QUOTE` = $1.00 ([`constants.rs:39`](crates/solfx-math/src/constants.rs#L39)); `min_position_base` is "a millionth of a lot" and explicitly does not impose a value floor ([`contracts.rs:63-69`](crates/solfx-keeper/src/contracts.rs#L63-L69)). So the exposed band is $1–$500 of notional, which is ordinary retail size, not an edge case.

The worst case is better for the attacker than the average. `penalty` is capped at positive equity ([`liquidate.rs:193`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L193)), so a position allowed to reach zero or negative equity yields `penalty = 0`, `liquidator_out = 0`, and **the entire $1.00 comes from the insurance fund**.

**Attack shape.** Open a small position, act as your own liquidator, collect the top-up. The trader-side loss goes to the LP pool and can be largely neutralised by opening the opposite side on a second account; the residual cost is spread plus fees on both legs. Per cycle the insurance fund pays out up to $1.00 against a few cents of transaction and spread cost.

**What I verified:** the threshold arithmetic above, the `penalty = 0` path, the $1 notional floor, and that `insurance_draw + reward_from_insurance ≤ insurance_available` so the fund cannot go negative in a single call.
**What I did not verify (unverified):** end-to-end net profitability of the hedged version, which depends on `base_spread_bps` per market and on the attacker's fill costs. I did not model it. The drain on the fund is certain; the attacker's margin is not established.

**Why the test suite does not catch it:** `self_liquidation_is_not_profitable` ([`tests/risk_engine.rs:171`](programs/solfx-core/tests/risk_engine.rs#L171)) uses a position with $250 of *collateral* — notional well above $500 — so `liquidator_out` exceeds $1 and the top-up branch never executes. The threat it names (T8) is tested only in the region where the subsidy is inactive.

**Applied fix.** The top-up ceiling is now the liquidator share of a *fully payable* penalty on the same position, rather than a flat `MIN_LIQUIDATOR_REWARD`:

```rust
let self_funded_reward = fees::split_liquidation_penalty(full_penalty)?.liquidator;
let reward_ceiling = MIN_LIQUIDATOR_REWARD.min(self_funded_reward);
let reward_top_up = reward_ceiling.saturating_sub(liquidator_out);
```

The insurance fund never pays more than the position could have paid itself. Above the ~$500
break-even the ceiling is at least a dollar, so `MIN_LIQUIDATOR_REWARD` still binds and
behaviour is **unchanged** — `self_liquidation_is_not_profitable` passes with and without the
fix. Below it the subsidy scales with the position: at $54 of notional the ceiling is $0.109
rather than $1.00, which is less than the spread and fees of opening the position, so the
self-liquidation cycle cannot profit.

This is the shape every comparable protocol uses — a liquidator premium proportional to the
position and capped, never a flat draw on the reserve. Kamino states the principle directly:
the bonus *"cannot exceed the gap between actual debt and collateral value"*. marginfi pays
2.5% of liquidated collateral; Zeta 30% of the maintenance-margin penalty.

**Test.** `self_liquidation_is_not_profitable_on_a_small_position` is no longer `#[ignore]`d.
It had never actually run — as written it opened a `MINI` (~$10,800) position on $2 of
collateral and failed at `LeverageTooHigh` before reaching the assertion. It now opens
`ONE_LOT / 2_000` (~$54 of notional) on $1.25, carrying the same 43x leverage as the
full-size sibling so it is liquidatable at the same adverse price. Reverting the fix fails it
on *"a solvent liquidation must not draw down the insurance fund"*.

---

### C-2 — [FIXED] Accrued carry is stranded in the collateral vault on liquidation, breaking invariant I1
**Severity: High (accounting/solvency)** · [`risk.rs:236-247`](programs/solfx-core/src/risk.rs#L236-L247), [`liquidate.rs:157`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L157), [`liquidate.rs:211-225`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L211-L225)

`settle_carry` deducts carry from `position.collateral` and returns the amount. It does **not** adjust `protocol.total_user_collateral`, and it moves no tokens — it relies entirely on the caller routing exactly `carry` back out through the fee path. In `liquidate_position` that routing is clamped:

```rust
let given_up   = collateral.checked_sub(credit)?;          // collateral is POST-carry
let routed_fee = carry.checked_add(split.insurance)?
                      .checked_add(split.treasury)?
                      .min(after_liquidator);              // ← the clamp
```

`given_up` is computed from the already-reduced collateral, so the carry is not inside it, yet `routed_fee` tries to pay the carry *out of* it.

**Worked trace.** Position collateral 100, accrued carry 10, uPnL 0, maintenance margin 200 (forced liquidatable), `full_penalty` 5:

| Step | Value |
|---|---|
| `settle_carry` | `position.collateral` 100 → 90, returns `carry = 10` |
| `liquidation_equity` | 90 + 0 = 90 |
| `penalty` | min(5, 90) = 5 → split 2 / 2 / 1 |
| `credit` | 90 − 5 = 85 |
| `given_up` | 90 − 85 = **5** |
| `liquidator_out` | min(2, 5) = 2 → `after_liquidator` = 3 |
| `routed_fee` | min(10 + 2 + 1, 3) = **3** ← clamp binds, 10 dropped |
| `pool_in` | 0 |

Tokens leaving the collateral vault: 2 + 3 + 0 = **5**. `protocol.total_user_collateral` decreases by the same 5. But the trader's claim fell from 100 to 85 — a decrease of **15**.

**Result: 10 units (exactly the carry) remain in the collateral vault attributed to no one.** `total_user_collateral` is left 10 too high, so:

- `vault == total_user_collateral` — still holds (I1, second form).
- `vault == Σ(user free_collateral + position collateral + funding_balance)` — **breaks by `carry`** (I1, first form).

Both forms are asserted in the harness ([`tests/common/mod.rs:2073-2078`](programs/solfx-core/tests/common/mod.rs#L2073-L2078)). The error accumulates across every liquidation with non-zero carry and inflates apparent protocol solvency.

**Why the test suite does not catch it.** No test exercises carry and liquidation together. The carry tests (`carry_accrues_over_time`, `carry_is_charged_when_the_position_closes`) never liquidate; all nine liquidation tests run with `cum_borrow_index` unmoved, so `settle_carry` returns 0 and the defect is invisible. Verified by scanning every `#[test]` in `tests/risk_engine.rs`.

The same pattern is present in [`adl.rs:146`](programs/solfx-core/src/instructions/keeper/adl.rs#L146) and should be checked there too.

---

## 2. Correctness bugs

### B-1 — [FIXED] Accrued carry is never charged on a voluntary close
[`instructions/trader/close_position.rs`](programs/solfx-core/src/instructions/trader/close_position.rs) (whole file), [`risk.rs:236`](programs/solfx-core/src/risk.rs#L236)

`grep -c carry close_position.rs` returns **0**. `settle_carry` has exactly two call sites — `liquidate.rs:157` and `adl.rs:146`. `reduce()` computes released collateral, close fee and realised PnL directly ([`close_position.rs:249-348`](programs/solfx-core/src/instructions/trader/close_position.rs#L249-L348)) and never reads `market.cum_borrow_index` or `position.cum_borrow_entry`.

A trader can therefore hold a position for any length of time, accrue carry, and close it without ever paying. Carry is revenue stream 2 in §8.1 and §6.5 defines equity as including `− accrued_carry`; neither holds on the voluntary-close path. Only liquidated and auto-deleveraged positions pay carry.

**The test that appears to cover this does not.** `carry_is_charged_when_the_position_closes` ([`tests/risk_engine.rs:548`](programs/solfx-core/tests/risk_engine.rs#L548)) asserts:

```rust
assert!(returned < 5_000 * ONE_USDC, "a day of carry at 20% must reduce what comes back");
assert!(env.token_balance(&env.fee_vault) > treasury_before, "carry is revenue and must reach the treasury");
```

Both assertions are satisfied by the **close fee and spread alone**, with carry at exactly zero. The test passes whether or not the feature works. This is the single most misleading item in the suite: it is cited as evidence for behaviour that is absent.

On a partial close the accrued carry on the closed fraction is forgiven outright — `entry_notional` is reduced proportionally ([`close_position.rs:403`](programs/solfx-core/src/instructions/trader/close_position.rs#L403)) while `cum_borrow_entry` is left untouched.

### B-2 — [FIXED] The liquidation penalty is not split 40 / 40 / 20
[`liquidate.rs:194`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L194), [`liquidate.rs:218-222`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L218-L222), [`liquidate.rs:261-265`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L261-L265)

`ARCHITECTURE.md` §6.8 step 6 specifies *"liquidator 40% | insurance 40% | treasury 20%"*, and `split_liquidation_penalty` implements exactly that ([`fees.rs:164-177`](crates/solfx-math/src/fees.rs#L164-L177)). But only `split.liquidator` is used as computed. The other two shares are summed with the carry and passed into `compute_flows` as an ordinary fee, where `split_fee` **re-divides them by the trading-fee schedule** (LP 55 / treasury 25 / insurance 10 / referral 10, [`fees.rs:70-75`](crates/solfx-math/src/fees.rs#L70-L75)).

Effective destination of a penalty, recomputed:

| Destination | Specified | Actual |
|---|---:|---:|
| Liquidator | 40% | 40% |
| Insurance | 40% | 60% × 10% = **6%** |
| Treasury | 20% | 60% × 25% = **15%** |
| LP pool | — | 60% × 55% = **33%** |
| Referral | — | 60% × 10% = **6%** |

The insurance fund receives roughly **one-seventh** of its specified share of every penalty. Since the fund is the first step of the §6.9 bad-debt waterfall and C-1 also draws on it, this compounds: the reserve is both under-fed and over-drawn.

**Why the test does not catch it:** `the_liquidation_penalty_is_split_three_ways` ([`tests/risk_engine.rs:127`](programs/solfx-core/tests/risk_engine.rs#L127)) asserts only that each destination *increased* (`> before`) — never a proportion. The test name promises what the assertions do not check.

### B-3 — `thirty_day_volume` never decays; it is a lifetime counter
[`state/user_account.rs:37-38`](programs/solfx-core/src/state/user_account.rs#L37-L38), [`instructions/user.rs:66-67`](programs/solfx-core/src/instructions/user.rs#L66-L67)

`volume_window_start_ts` is written once at account creation and **never read again anywhere in the workspace** (verified by grep across `programs/` and `crates/`). `thirty_day_volume` is only ever `saturating_add`-ed — at `open_position.rs:273`, `close_position.rs:424` and `increase_position.rs:177`. There is no decay, no window reset, no scheduled sweep.

It feeds the fee tier ([`fees.rs:30`](crates/solfx-math/src/fees.rs#L30)) and the IB tier ([`referral.rs:58`](crates/solfx-math/src/referral.rs#L58)). A trader who cumulatively crosses $250M of notional is permanently pinned at the lowest 0.6 bps tier regardless of subsequent activity — a one-way ratchet on revenue, and a mismatch with the §8.2 schedule, which is explicitly a *30-day* schedule.

Note the field is also incremented on both open and close, so a round trip counts the notional twice.

### B-4 — The utilisation cap is per-market but is documented and reasoned about as global
[`open_position.rs:350-372`](programs/solfx-core/src/instructions/trader/open_position.rs#L350-L372)

`check_utilisation` divides **one market's** open interest by the **whole pool's** AUM:

```rust
let exposure = market.oi_long + market.oi_short;
let utilisation = exposure * 10_000 / aum;
require!(utilisation <= max_utilisation_bps, ...);
```

There is no protocol-level aggregate exposure anywhere in the program — confirmed by grep for `total_oi`, `global_oi`, `total_exposure`, `total_notional`: no such field exists on `Protocol`. With the cap at 80% and five markets listed, aggregate exposure can reach 400% of AUM with every individual check passing.

§7.2 lists *"Vault utilisation > 80%"* as a pool-level breaker and §7.4 specifies *"Global utilisation cap: Σ(max_loss_exposure) / AUM ≤ 0.60"*. Neither is implemented at the pool level. The function's own doc comment claims it "bounds the *pool's*" risk; it bounds one market's.

### B-5 — `skew_impact_bps` is a flat 1 bp step, not the size gradient §6.6 specifies
[`pricing.rs:87-96`](crates/solfx-math/src/pricing.rs#L87-L96), configured at [`init_protocol.rs:551`](crates/solfx-keeper/src/bin/init_protocol.rs#L551)

```rust
let denominator = LOT_SIZE_BASE.checked_mul(RATE_PRECISION)?;   // 1e14 × 1e9 = 1e23
let impact = mul_div_ceil(skew_delta, rate, denominator)?;
```

The shipped configuration is `skew_impact_bps_per_unit: 1`. Recomputed:

| Imbalance increase | True value | Returned (ceil) |
|---|---:|---:|
| 1 lot (1e14) | 1e-9 bps | **1 bps** |
| 100 lots | 1e-7 bps | **1 bps** |
| 10,000 lots | 1e-5 bps | **1 bps** |
| 10⁹ lots (1e23) | 1 bps | 1 bps |

Because the ceiling fires on any positive `skew_delta`, **every** book-worsening trade pays exactly 1 bp and nothing scales with size until 10⁹ lots. §6.6's stated purpose — *"make it progressively expensive to push the book further one-sided"* — is not achieved; the mechanism is a constant.

The rounding direction is protocol-favourable, so this is not a drain. It is a spec/implementation divergence with two consequences: the inventory-management lever described in §6.6 does not function, and every small trade on the heavy side is over-charged by up to 1 bp, which on a market with a 1–2 bp base spread is a 50–100% widening. The documentation in `pricing.rs:79-86` describes behaviour ("0.001 bps per lot: a 100-lot imbalance increase costs 0.1 bps") that the ceiling makes unreachable.

### B-6 — First depositor into an emptied-but-funded LP pool captures the retained balance
[`lp.rs:56-68`](crates/solfx-math/src/lp.rs#L56-L68)

```rust
if supply == 0 || aum == 0 { return Ok(amount); }   // 1:1
```

If `supply == 0` while `aum > 0` — every LP has exited while the pool holds retained exit and performance fees — the next depositor mints 1:1 and immediately owns the entire retained `aum` alongside their own deposit.

The docstring acknowledges the choice but frames it as *"equally arbitrary but at least cannot be gamed by timing the last exit."* That understates it: the last LP to exit knows precisely when `supply` reaches 0 and can deposit 1 unit immediately afterward to take the retained fees. The cooldown cited as the defence ([`lp_pool.rs:32`](programs/solfx-core/src/state/lp_pool.rs#L32)) gates *withdrawals*, not deposits, so it does not delay the re-entry leg.

Low reachability in a pool with many providers; the consequence when reached is total capture of the retained balance.

### B-7 — `health_factor_bps` and `is_liquidatable` disagree at one point
[`margin.rs:110-132`](crates/solfx-math/src/margin.rs#L110-L132)

At `equity == 0, maintenance_margin == 0`: `health_factor_bps` returns 0 via the `equity <= 0` early return, so the UI reads "liquidatable"; `is_liquidatable` evaluates `0 < 0` = false. The agreement test (`health_factor_and_liquidatable_agree`) only ever uses `mm = 1_000 * USD` and so never reaches it.

`maintenance_margin()` rejects `mmr_bps == 0` and rounds up, so `mm == 0` requires `notional == 0`, which the notional floor should prevent. **Unverified** whether the state is reachable through any instruction; recorded because the module's own stated contract is that the two never disagree.

---

## 3. Deprecated patterns

### D-1 — `anchor_spl::token::transfer` is deprecated; `transfer_checked` is current
All **13** CPI call sites: [`trader/mod.rs:65`](programs/solfx-core/src/instructions/trader/mod.rs#L65), [`trader/mod.rs:84`](programs/solfx-core/src/instructions/trader/mod.rs#L84), [`liquidate.rs:405`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L405), [`liquidate.rs:422`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L422), [`liquidate.rs:439`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L439), [`liquidate.rs:488`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L488), [`lp.rs:95`](programs/solfx-core/src/instructions/lp.rs#L95), [`lp.rs:414`](programs/solfx-core/src/instructions/lp.rs#L414), [`lp.rs:431`](programs/solfx-core/src/instructions/lp.rs#L431), [`user.rs:138`](programs/solfx-core/src/instructions/user.rs#L138), [`user.rs:205`](programs/solfx-core/src/instructions/user.rs#L205), [`protocol_admin.rs:164`](programs/solfx-core/src/instructions/admin/protocol_admin.rs#L164), [`protocol_admin.rs:266`](programs/solfx-core/src/instructions/admin/protocol_admin.rs#L266).

Anchor's own token-proxy test program now wraps the unchecked form in `#[allow(deprecated)]` ([`anchor/tests/spl/token-proxy/programs/token-proxy/src/lib.rs`](https://github.com/solana-foundation/anchor/blob/HEAD/tests/spl/token-proxy/programs/token-proxy/src/lib.rs)), and the current documentation states plainly: *"To transfer tokens through an Anchor program, you need to make a cross program invocation (CPI) to the `transfer_checked` instruction"* ([Anchor — Transfer Tokens](https://github.com/solana-foundation/anchor/blob/HEAD/docs/content/docs/tokens/basics/transfer-tokens.mdx)).

Impact here is low: SolFX pins `Program<'info, Token>` (classic Token, not Token-2022) and validates `usdc_mint` on the user-facing token accounts, so the decimals confusion `transfer_checked` guards against is constrained. It will become a real problem if Token-2022 collateral is ever listed. Expect deprecation warnings on any Anchor upgrade.

### D-2 — `has_one` is deprecated in Anchor v2 (forward-looking only)
[`open_position.rs:31`](programs/solfx-core/src/instructions/trader/open_position.rs#L31) and other `Accounts` structs.

The Anchor **v2** constraint reference lists `has_one = field` as *"Deprecated relation check. Prefer `address = parent.field` on the sibling account"* ([anchor docs-v2, `v2/reference/account-constraints.mdx`](https://github.com/solana-foundation/anchor/blob/HEAD/docs-v2/src/content/docs/v2/reference/account-constraints.mdx)).

**This is not a defect today.** SolFX is on Anchor 1.1.2, where `has_one` is the standard idiom and is correctly used with a custom error. Recorded only so a v2 migration plans for it. Note the codebase already prefers `address =` where the relation is to a wallet ([`liquidate.rs:86`](programs/solfx-core/src/instructions/keeper/liquidate.rs#L86)).

### D-3 — Checked and found *not* deprecated
Two patterns that look wrong against pre-1.0 Anchor knowledge and are in fact correct here. Verified via `Solana_Expert__Ask_For_Help` rather than assumed:

- **`CpiContext::new_with_signer(ctx.accounts.token_program.key(), …)`** — passing a `Pubkey`, not an `AccountInfo`, is the **correct** Anchor 1.x form. *"Anchor 1.0.0 removed the redundant AccountInfo copy from CpiContext… Pre-1.0 (broken in 1.0.0): `CpiContext::new(ctx.accounts.token_program.to_account_info(), …)`; Anchor 1.0.0: `CpiContext::new(ctx.accounts.token_program.key(), …)`"* ([Chainstack — Solana: Anchor development](https://docs.chainstack.com/docs/solana-anchor-development)).
- **`space = 8 + Position::INIT_SPACE`** ([`open_position.rs:45`](programs/solfx-core/src/instructions/trader/open_position.rs#L45)) — correct; the 8-byte discriminator must be added explicitly ([Solana Docs — PDA Accounts](https://solana.com/docs/core/pda/pda-accounts)).

---

## 4. Verified correct

Recorded because an audit that lists only defects misrepresents the codebase. Each of these was checked, not assumed.

**Oracle read path** — [`programs/solfx-core/src/oracle.rs`](programs/solfx-core/src/oracle.rs). Six gates in one choke point, no instruction reading a price any other way. The SDK's own header warns that `get_price_unchecked` *"does not check: how recent the price is, whether the price update has been verified… unsafe to use without any extra checks"* ([pyth_solana_receiver_sdk `price_update.rs`](https://github.com/helium/helium-program-library/blob/HEAD/utils/pyth_solana_receiver_sdk/src/price_update.rs)). SolFX supplies both missing checks explicitly — `VerificationLevel::Full` at [`oracle.rs:263-266`](programs/solfx-core/src/oracle.rs#L263-L266) and freshness at [`oracle.rs:276-282`](programs/solfx-core/src/oracle.rs#L276-L282) — and closes the **future-dated** end that `get_price_no_older_than` leaves open. Pyth's own best-practices page identifies staleness as the primary defence against adversarial price selection ([Pyth — Best Practices](https://docs.pyth.network/price-feeds/core/best-practices)); the two-sided window is stricter than the documented baseline and stricter than the Kamino and marginfi implementations returned by the same search. The requirement to validate feed **and** timestamp ([Pyth — Solana pull integration](https://docs.pyth.network/price-feeds/core/use-real-time-data/pull-integration/solana)) is met by `get_price_unchecked(expected_feed_id)`.

**PDA derivation and seeds** — no runtime `find_program_address` in program source; bumps stored at init and reused, matching the documented optimisation (*"By storing the bump value in account data, the program avoids a runtime search"*, [anchor docs-v2, `pdas-and-resolution.mdx`](https://github.com/solana-foundation/anchor/blob/HEAD/docs-v2/src/content/docs/v2/fundamentals/pdas-and-resolution.mdx)). Every variable-length seed is fixed-width (32-byte pubkey, `u16::to_le_bytes`, single `u8` nonce), so the seed-collision hazard — *"`seeds = [b"ab", b"cd"]` and `seeds = [b"abcd"]` produce the same PDA"* ([Chainstack](https://docs.chainstack.com/docs/solana-anchor-development)) — does not arise. Canonical-bump handling matches the secure pattern in [`sealevel-attacks/7-bump-seed-canonicalization`](https://github.com/coral-xyz/sealevel-attacks/blob/HEAD/programs/7-bump-seed-canonicalization/secure/src/lib.rs).

**Account validation** — `program_autofixer` returned zero issues and zero suggestions on the `LiquidatePosition` surface and its CPI helper. Manually: every account carries owner (`Account<T>`), instance (`seeds`/`constraint`/`address`), signer and mutability constraints; the three `UncheckedAccount`s are all address-constrained; `rent_destination` carries `address = user_account.authority`, so a liquidator cannot redirect rent to themselves; all `token_program` fields are `Program<'info, Token>`, closing arbitrary-CPI. No `init_if_needed`, no `realloc` anywhere.

**Fixed-point arithmetic** — [`crates/solfx-math/src/fixed.rs`](crates/solfx-math/src/fixed.rs) is the only module permitted raw `/` and `%`, each guarded and each with a named rounding direction. `mul_div_floor_signed` correctly rounds toward −∞ rather than Rust's toward-zero truncation, which is the difference between growing and shrinking a trader's loss. Workspace lints deny `arithmetic_side_effects`, `integer_division`, the lossy casts, `float_arithmetic`, `indexing_slicing`, `unwrap_used`, `expect_used`, `panic`; `unsafe_code` is forbidden.

**Financial formulas recomputed from first principles.** All of the following were computed independently and matched the code exactly:

| Quantity | Independent recomputation | Code | Match |
|---|---|---|:--:|
| Notional, 1 lot EUR/USD @ 1.08543 | `⌈1e14 × 1.08543e9 / 1e12⌉` = 108,543,000,000 = $108,543.00 | `pnl.rs:22` | ✓ |
| uPnL, 1 lot, +10 pips | `⌊1e14 × 1e6 / 1e12⌋` = 1e8 = $100.00 ($10/pip) | `pnl.rs:46` | ✓ |
| USD/INR PnL conversion | 50,000 INR ÷ 89.00 = $561.797752…→ floor 561,797,752 | `pnl.rs:81` | ✓ |
| USD/JPY PnL conversion | 10,000 JPY ÷ 157.30 = $63.5727908…→ floor 63,572,790 | `pnl.rs:81` | ✓ |
| Initial margin @ 2% | `⌈1.085e11 × 200 / 1e4⌉` = $2,170.00 | `margin.rs:40` | ✓ |
| Maintenance margin, 1.5× tier | `⌈⌈1.085e11 × 100/1e4⌉ × 15000/1e4⌉` = 1,627,500,000 | `margin.rs:52` | ✓ |
| Liquidation price, long | 1.0850 − ($1,085 ÷ 100,000) = 1.07415 (exactly −1.00%) | `margin.rs:141` | ✓ |
| Execution price, 10 bps | 1.08543 ± 0.00108543 → 1,086,515,430 / 1,084,344,570 | `pricing.rs:126` | ✓ |
| Fee, 1 bp on $108,543 | 108,543e6 × 1e5 / 1e9 = $10.8543 | `fees.rs:47` | ✓ |
| Funding conservation (I3) | longs pay 3,000 × 500,000 = shorts receive 1,000 × 1,500,000 | `funding.rs:170` | ✓ |
| Funding conservation, extreme skew | 1,000,000 × 999,983 = 1 × 999,983,000,000 | `funding.rs:170` | ✓ |

**Rounding discipline** — verified adverse to the initiating party throughout: fees and margin ceil, payouts floor, `weighted_entry_price` rounds direction-aware (up for a long, down for a short — both reduce trader profit), `proportional_pnl` floors signed so partial closes never over-realise a gain and never under-realise a loss. `split_fee` gives the treasury the remainder rather than rounding each share, making conservation exact by construction rather than by luck.

**Liquidation boundary** — strict `<` ([`margin.rs:110`](crates/solfx-math/src/margin.rs#L110)); equity equal to maintenance margin is not liquidatable, closing griefing threat T7.

**Non-custodial claim** — holds. Margin is isolated: position collateral moves out of `free_collateral` into the `Position` account, so `withdraw_collateral` needs only the authority's signature and a sufficient free balance ([`user.rs:188-195`](programs/solfx-core/src/instructions/user.rs#L188-L195)). No admin instruction can move a user's collateral.

---

## 5. Stylistic notes

- **S-1** — `docs/CONTEXT.md` reports test counts of 490, 529 and 531 in different places (lines 4, 82, 242 and the phase table). The handoff says 531. Pick one and generate it.
- **S-2** — `MathError` has no `SlippageExceeded` variant; `validate_slippage` returns `InvalidPrice` ([`pricing.rs:165`](crates/solfx-math/src/pricing.rs#L165)) and both call sites remap it (`open_position.rs:134`, `close_position.rs:286`). Correct, but one forgotten remap would surface a slippage rejection as an oracle error. A dedicated variant would make the mapping unnecessary.
- **S-3** — `total_spread_bps` ceils each of three components independently ([`pricing.rs:102`](crates/solfx-math/src/pricing.rs#L102)), so the total can exceed the true figure by up to 2 bps. Protocol-favourable and bounded, but combined with B-5 it means the minimum effective spread on a book-worsening trade is meaningfully wider than `base_spread_bps` suggests.
- **S-4** — `accrued_funding` can in principle overflow `i64` under sustained extreme skew, since `funding_index_update` scales the light side's rate by `payer_oi / receiver_oi` ([`funding.rs:200`](crates/solfx-math/src/funding.rs#L200)). Every operation is checked, so the failure mode is an error rather than a wrap — but an error on `accrued_funding` would make the position **unclosable**, since every close and liquidate path computes it. **Unverified** whether the required OI ratio is reachable given the skew cap; worth a bound proof before mainnet.
- **S-5** — `normalize_price` accepts positive Pyth exponents ([`oracle.rs:103`](crates/solfx-math/src/oracle.rs#L103)); Kamino's equivalent rejects them outright as a corrupt-feed signal. SolFX's handling is safe (bounded by `pow10` overflow checks), just more permissive than the peer implementation.

---

## 6. Verdict on production viability

**Not ready for mainnet. Ready to continue toward devnet.**

The engineering standard here is genuinely high, and I want to be precise about that rather than generous. The oracle read path is stricter than the Pyth SDK's own recommended helper and stricter than the Kamino and marginfi implementations I compared it against. The account-validation surface passed both a manual four-question review and the MCP static analyser with zero findings. The rounding discipline in `solfx-math` is the best part of the codebase: one audited module owns every division, each with a declared direction, and eleven separate financial formulas recomputed from first principles matched the code exactly — including the two quote-conversion paths that are the usual place FX engines go wrong. The LP inflation attack really is closed, for the structural reason claimed.

What stops it being production-ready is not the cryptography or the account model. It is that **three of the money paths do something different from what the specification and the tests say they do**, and the test suite is shaped so that none of them shows up:

1. Carry is not charged on voluntary closes (B-1) — and the test named for that behaviour passes on the close fee alone.
2. The liquidation penalty does not reach the insurance fund in the specified proportion (B-2) — and the test named for that behaviour asserts only that balances went up.
3. Carry is stranded on liquidation and breaks invariant I1 (C-2) — and no test puts carry and liquidation in the same scenario.

Each is individually fixable in hours. The pattern is the concern: **531 green tests are a weaker signal than they appear**, because at least three tests are named for properties their assertions do not check. Before mainnet the suite needs a pass for tests that would still pass if the feature were deleted — B-1 and B-2 were both found that way, and I would not assume they are the only two.

The insurance fund is the sharpest concrete risk. It is under-fed by B-2 (receiving ~6% of penalties instead of 40%), drained by C-1 (up to $1.00 per sub-$500 liquidation, with self-liquidation an open path), and is simultaneously step 1 of the §6.9 bad-debt waterfall. Those three interact, and `docs/CONTEXT.md` §6.9's own warning applies — launching with a depletable insurance fund means the first gap event reaches LPs directly.

Against the roadmap's own gates: no external audit has been done (the roadmap budgets $40–150k for the first), Trident fuzzing is a Phase 9 exit criterion and has not been run, and neither would have found B-1 or B-2 anyway — a fuzzer explores the code that exists, and these are behaviours that are absent. They need a specification-versus-implementation review, which is what this document is a first pass at.

**Recommended sequence before devnet:** fix C-2 and B-2 (both localised, both in `liquidate.rs`), decide whether B-1 is a bug or a deliberate scope cut and make the docs and test names say so either way, then re-run the suite with the four misleading tests strengthened. B-4 and B-5 are configuration and design questions that can be settled during devnet operation. C-1 needs a parameter or a mechanism change before any deployment that holds real value.

**Not audited:** `programs/solfx-referral` (589 lines) beyond its interface with `UserAccount`; the keeper services and the four operator binaries except where they set market parameters; the trigger-order and session state machines; ADL beyond confirming it shares C-2's carry pattern. Absence from this document is not a clean verdict on any of them.
