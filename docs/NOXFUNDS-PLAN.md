# NOXFUNDS — a decentralized prop firm on top of SolFX

## Context

SolFX (**live on devnet** since 2026-09; Phases 1–9, 565 Rust tests, 97.14% SBF coverage of
the on-chain instructions) is a non-custodial
forex broker on Solana. **NOXFUNDS is a separate product** that uses SolFX as its execution
venue: traders prove themselves on a simulated evaluation, investors pick them from an
auditable on-chain track record and fund them, and the program custodies the capital so the
trader can trade it but never take it.

User's constraints for this work:

- **Do not change anything in solfx-core or solfx-referral.** Verified achievable — §2.1.
- Plan first; implementation only after approval.
- SolFX ships to devnet before NOXFUNDS is built.

---

## Corrections — 2026-09-15

This document was written before SolFX reached devnet, and its load-bearing numbers were
**modelled, never measured**. They have now been derived from source and confirmed against the
Solana MCP. Four decisions were also settled with the user. Sections below are amended in
place; this is the index.

| # | What changed | Where |
|---|---|---|
| **C1** | **The Mandate PDA cannot be the SolFX `authority`.** Both CPIs are `payer = authority`, which routes through the System Program, which rejects a `from` that carries data. A second dataless PDA, `mandate_signer`, is required. | §2.1, §2.4, §2.5 |
| **C2** | **`UncheckedAccount` is not cheaper on the stack than `Box<Account<T>>`** — both are 8 bytes. The mitigation is right, the stated reason is wrong, and the real cost (two by-value `CpiContext`s, 1,392 bytes) was never modelled. | §2.3, R1 |
| **C3** | **Rent is 2.73× the estimate:** 0.0046354 SOL per open position, not ~0.0017. The `TriggerOrder` was omitted entirely. | §2.1 |
| **C4** | **`funded_cancel_stop` is missing.** Without it every voluntary close strands 0.0020393 SOL in an orphaned `TriggerOrder`. | §2.5, Part 6 |
| **C5** | **The account count is 22, not ~21**, and the two extra oracle legs must be **deserialized**, not passed through — §2.3 contradicted §2.5 and §2.5 wins. | §2.3, §2.5 |

Decisions settled with the user, 2026-09-15:

- **Product:** the prop firm described here. A mockup circulated with a
  SWAP/LIQUIDITY/STAKE/GOVERNANCE nav is a **style reference only** and is not this product.
- **Location:** same repository, new `programs/noxfunds/`, one branch.
- **Protocol fee:** **flat 5% of gross.** §4.2's tiered 10/8/6/4 column is withdrawn.
- **Treasury:** `7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs`, the program upgrade authority.
  Recorded concern, overridden by the user: one key then holds both protocol revenue and the
  power to replace the program. It is a `NoxConfig` field and admin-updatable.

---

## Part 1 — Does this already exist?

Short answer: **the category exists, one close competitor is being built on Solana, and nobody
has done it for forex.** The specific thing SolFX makes possible has no precedent.

### The four-level framework

HyroTrader publishes a taxonomy the industry has largely adopted:

| Level | On chain | Who is here |
|---|---|---|
| 1 — Centralized | Nothing | **"Effectively all of forex prop trading."** FTMO, FundedNext, etc. |
| 2 — On-chain payouts | Withdrawals only | *Most* firms advertising themselves as "on-chain" |
| 3 — On-chain protocol | Rules, records, payouts, LP vaults | **"Very few firms deliver"** |
| 4 — Fully decentralized | + trade execution itself | Essentially nobody |

### Who is actually building at Level 3

| Firm | Chain | Assets | Capital source | Status |
|---|---|---|---|---|
| **Hyro Protocol** | Solana | Crypto only | **LPs choose which trader to fund** | Announced, phased rollout — not fully live |
| **Carrot Funding** | Arbitrum | Crypto/FX/stocks via gTrade | Firm's own | Live; rules in contracts |
| **Solana Funded** | Solana | Solana memecoins | Firm's own | Live — but Level 2 (payouts only) |
| **blockfirm** | Solana | Solana memecoins | Firm's own | Live — Level 2 |
| **Hyperliquid vaults** | Hyperliquid | Crypto perps | **Depositors fund a leader** | Live, large |

**Hyro Protocol is the nearest thing to NOXFUNDS** and worth watching closely: Solana, USDC
settlement, LPs allocating to verified traders on track record. But crypto-only, and not yet
fully live.

### The three gaps that remain open

**1. Nobody does this for forex.** Level 1 is where *all* forex prop trading sits. Carrot
routes forex through gTrade but funds from its own balance sheet. SolFX already lists
oracle-verified FX markets including EM pairs no venue on any chain carries — inventory a prop
firm renting a venue cannot replicate.

**2. Prop-firm rules and investor-funded vaults exist separately, never together.**
Hyperliquid vaults let depositors back a trader — with **no rules at all**. A leader can run
20× and lose everything; in March 2025 the JELLY incident cost the HLP vault ~$12M and drove
TVL from $540M to ~$150M. Prop firms have rules but fund from their own capital. This design
is the synthesis: **investor-funded *and* rule-bound.**

**3. Nobody owns the venue — the decisive one.**
Every on-chain prop firm funds you to trade *somewhere else*: Hyperliquid, gTrade, a Solana
DEX. Their contract cannot see the order before it fills, so rules are necessarily
**punitive** — the violating trade executes and you get flagged afterwards.

SolFX *is* the venue, so NOXFUNDS can put its check **in front of** the fill:

> A trade that breaks the rules is not detected and punished. **It fails as a transaction. It
> never existed, and the investor never took the loss.**

No firm surveyed can make that claim today. It follows directly from the user having already
built the exchange, and it is what puts this at **Level 4**.

### Honest counterweights

- ~7% of prop-firm entrants ever see a payout. Traders know this; the pitch has to be
  "provably fair rules", not "easy money".
- **No on-chain prop firm holds a financial services licence**; most register as technology
  platforms offering evaluation-based trading. Same posture applies here.
- SolFX is a **B-book** — its LP pool is the counterparty to every trade, so funded traders'
  profits are ultimately paid by SolFX's LPs. A population pre-filtered for skill is adverse
  selection against them. Small at first, and NOXFUNDS volume pays SolFX fees, but it must
  be monitored (§5, risk R3).

**Sources:** [HyroTrader — the 4 levels](https://www.hyrotrader.com/blog/on-chain-prop-trading-firm/) ·
[Hyro Protocol](https://www.hyrotrader.com/blog/hyro-protocol-on-chain-crypto-prop-trading/) ·
[Best on-chain prop firms 2026](https://alexfirdaus.com/best-on-chain-prop-firms/) ·
[Decentralized prop firms compared](https://roya-trading.com/decentralized-prop-firm/) ·
[Solana Funded](https://solanafunded.com/) · [blockfirm](https://myblockfirm.com/) ·
[Hyperliquid vault docs](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/vaults/for-vault-leaders-legacy) ·
[JELLY incident](https://www.halborn.com/blog/post/explained-the-hyperliquid-hack-march-2025) ·
[Crypto prop firms overview](https://www.coingecko.com/learn/best-crypto-prop-firms)

---

## Part 2 — Design

### 2.1 The mechanism, and why SolFX needs no changes

Verified directly in the code:

- On `open_position`, `initialize_user_account`, `deposit_collateral` and
  `withdraw_collateral`, `authority` is a plain `Signer<'info>` with `has_one = authority` and
  **no owner constraint** ([open_position.rs](./programs/solfx-core/src/instructions/trader/open_position.rs), [user.rs](./programs/solfx-core/src/instructions/user.rs)).
- `solfx-core/Cargo.toml` already exposes `cpi = ["no-entrypoint"]` — the referral program
  uses it.

So **a NOXFUNDS PDA can be the authority of a SolFX `UserAccount`** and sign via
`CpiContext::new_with_signer`. The trader signs NOXFUNDS; the mandate PDA signs SolFX.

This is what makes custody safe with no new code in SolFX:

> The trader can open and close positions. The trader can **never** call
> `withdraw_collateral`, because they are not the authority — the mandate PDA is, and it only
> signs a withdrawal through NOXFUNDS's settlement path.

Two consequences to budget for:
- `position` is `init, payer = authority`, so the signing PDA must hold SOL. **Corrected
  (C3):** rent is `(128 + len) × 6,960` lamports — verified against `solana-rent 4.3.0`, and
  invariant to SIMD-0194. `Position` is 245 data bytes → **2,596,080 lamports**;
  `TriggerOrder` is 165 → **2,039,280**. Because the stop-loss is mandatory, an open position
  costs **both**: **4,635,360 lamports (0.0046354 SOL)**, and a mandate at the three-position
  cap must float ≈ **0.0148 SOL**. The original ~0.0017 counted neither correctly.
- **C1: the payer cannot be the `Mandate` account.** `payer = authority` is serviced by the
  System Program, which refuses a `from` that carries data, and `Mandate` is an `#[account]`
  struct. The authority must be a **dataless, system-owned PDA** — `mandate_signer` — which
  also signs both CPIs and holds the rent float above. See §2.4.
- `deposit_collateral` requires `user_token_account.owner == authority`, so the mandate PDA
  must own a USDC token account.

Dependency direction matches the `solfx-referral` precedent exactly: `noxfunds → solfx-core`,
one way, `features = ["cpi"]`.

### 2.2 The transaction-size question — answered

The #1 risk was that wrapping `open_position` in a CPI would not fit the 1232-byte packet. I
modelled it against the Phase 7 measurement (`liquidate_position` = 758 bytes measured; model
reproduces 758 exactly), on the **widest** market shape — synthetic *and* non-USD-quoted,
three oracle legs:

| Transaction | Bytes | Limit |
|---|---:|---:|
| `open_position` (bare) | ~753 | 1232 |
| **`funded_open_position`** (+5 accounts, +1 program) | **~958** | 1232 |
| same, with an Address Lookup Table | ~713 | 1232 |

**It fits, with ~270 bytes of headroom.** An ALT is an optimisation, not a requirement.

### 2.3 The remaining real risk: BPF stack frames

SolFX hit `Access violation in stack frame 5` twice — at 13 accounts, and again at 18. The
wrapper needs ~21. **Mitigation: NOXFUNDS deserializes only what it reads.**

| Deserialize (`Box<Account<T>>`) | Pass through (`UncheckedAccount`) |
|---|---|
| `config`, `mandate`, `trader_profile`, `market` | protocol, user_account, position, all four vaults, lp_pool, insurance_fund, price updates, programs |

An `UncheckedAccount` is a bare `AccountInfo` — no deserialization, and **solfx-core
re-validates every one of them anyway** through its own seeds and constraints. An account
NOXFUNDS never reads is an account it should not deserialize.

> **C2 — the reason above is wrong, though the conclusion is right.** Read from
> `anchor-lang 1.1.2`: `UncheckedAccount` is `(&AccountInfo)` — **8 bytes** — and
> `Box<Account<T>>` is a pointer, also **8 bytes**. Pass-through saves compute and heap, not
> stack. A 22-field accounts struct is ~176 bytes, comparable to `LiquidatePosition`'s 17 × 8
> = 136, which passes in CI today.
>
> **The real stack cost is in the handler, and this document never modelled it.** Anchor's
> generated CPI structs hold `AccountInfo` *by value* (48 bytes each), so
> `CpiContext<OpenPosition>` is 16 × 48 + 72 = **840 bytes** and
> `CpiContext<PlaceTriggerOrder>` is 10 × 48 + 72 = **552**. If both live at once that is
> **1,392 bytes** before anything else. Total peak is ~1,100 (LLVM reuses the slot) to ~2,890
> (it does not), against `STACK_FRAME_SIZE` = **4,096**.
>
> **Even the pessimistic bound clears by ~1,200 bytes**, and the failure mode has a
> pre-committed two-line remedy: `#[inline(never)]` on each CPI call, so the two contexts
> never share a frame and peak drops to ~1 KB. R1 is downgraded accordingly.
>
> **C5:** the table above puts the price updates in the pass-through column. §2.5 step 2 calls
> `load_validated_price`, whose signature takes `&PriceUpdateV2` — so all three oracle legs
> must be **deserialized**. §2.5 wins; it costs 8 bytes each.

**Stage 0 still measures this before anything else is built**, because whether LLVM reuses
that one stack slot is the single thing arithmetic cannot settle.

### 2.4 Accounts

All PDAs of the `noxfunds` program.

| Account | Seeds | Holds |
|---|---|---|
| `NoxConfig` | `["config"]` | admin, guardian, fee params, tier thresholds, SolFX program id, USDC mint |
| `TraderProfile` | `["trader", authority]` | the track record — see §2.6 |
| `Evaluation` | `["eval", trader, stage:u8]` | simulated balance, stage rules, per-stage stats |
| `VirtualPosition` | `["vpos", evaluation, market_index, nonce]` | simulated position (mirrors SolFX `Position` fields) |
| `InvestorAccount` | `["investor", authority]` | tier, total funded, active mandates, lifetime returns |
| **`Mandate`** | `["mandate", investor, trader, seq:u8]` | Rules, capital, split, peak equity, state. **Not** the SolFX authority — see below |
| **`MandateSigner`** | `["signer", mandate]` | **The SolFX authority.** Dataless and system-owned: signs both CPIs, pays position and trigger rent, owns the USDC token account |
| `MandateVault` | `["vault", mandate]` | USDC token account owned by `MandateSigner` |

> **C1 — why two accounts and not one.** The plan originally made `Mandate` itself the SolFX
> authority. It cannot be. `open_position` and `place_trigger_order` are both
> `init, payer = authority`, and Anchor services that through the System Program, which
> rejects a `from` that carries data. `Mandate` is an `#[account]` struct, so it is
> data-bearing and owned by `noxfunds`. The direct-lamport workaround fails too: the runtime
> raises `UnbalancedInstruction` when an account whose lamports were changed by hand is not in
> the CPI's own account list, and `Mandate` is not in `OpenPosition`.
>
> `MandateSigner` is a `SystemAccount` — no data, system-owned, so the System Program will
> debit it. It is the account that must hold ≈ 0.0148 SOL at the three-position cap.
> The custody argument is unchanged: the trader still never holds the authority.

`Mandate` is the centre of the design. Its fields:

```rust
pub struct Mandate {
    investor: Pubkey, trader: Pubkey, seq: u8,
    solfx_user_account: Pubkey,        // the SolFX account this PDA authorises
    principal: u64,                    // what the investor put in
    peak_equity: u64,                  // high-water mark, for drawdown
    // --- the rules, fixed at funding and never mutable ---
    max_trade_notional: u64,           // e.g. $1,000 on a $3,500 mandate
    max_drawdown_bps: u16,             // e.g. 300 = 3%
    max_daily_loss_bps: u16,
    require_stop_loss: bool,
    max_stop_distance_bps: u16,        // an SL at 99% away is not an SL
    allowed_markets: u128,             // bitmap over market_index; 128 markets
    min_hold_slots: u64,               // the no-scalping rule
    // --- economics ---
    trader_split_bps: u16,             // 7000
    // --- lifecycle ---
    state: MandateState, opened_at: i64, bump: u8, _reserved: [u8; 64],
}
pub enum MandateState { Active, Breached, WindingDown, Settled }
```

Rules are **written once at funding and immutable**, mirroring the referral program's
immutable `referrer` binding — the property that makes the ledger trustworthy is that nobody,
including the operator, can retighten terms after the fact.

### 2.5 The instruction that is the whole product

```rust
pub fn funded_open_position(
    ctx: Context<FundedOpenPosition>,
    market_index: u16, nonce: u8, direction: Direction,
    size_base: u64, collateral: u64, price_limit: i64,
    stop_loss_price: i64,        // MANDATORY — not an argument the trader may omit
) -> Result<()>
```

Order of operations:

1. `mandate.state == Active`, signer is `mandate.trader`
2. price the market via `solfx_core::oracle::load_validated_price` (same gates as a real trade)
3. notional ≤ `mandate.max_trade_notional` → else `TradeExceedsMandate`
4. `market_index` in `allowed_markets` → else `MarketNotPermitted`
5. `stop_loss_price` on the correct side, within `max_stop_distance_bps` → else `StopLossRequired`
6. current drawdown < `max_drawdown_bps` → else `MandateBreached`
7. **CPI** `solfx_core::open_position`, signed by the mandate PDA
8. **CPI** `solfx_core::place_trigger_order` (the stop-loss), signed by the mandate PDA
9. record the trade on `TraderProfile`

Steps 7–8 in one instruction is what makes "every trade must carry a stop-loss" **true rather
than monitored** — SolFX has no `Position → TriggerOrder` link, so the only way to guarantee
it is to place both atomically.

Also needed: `funded_close_position`, `funded_reduce_position`, `funded_modify_stop`, and
**`funded_cancel_stop` (C4)**.

> **C4 — why `funded_cancel_stop` is not optional.** Rent on a `TriggerOrder` returns to the
> keeper when a stop *fires*, and to the authority when the order is *cancelled*. There is no
> third path. So every **voluntary** close leaves an orphaned order holding 2,039,280 lamports
> with nothing able to reclaim it, and a mandate bleeds 0.0020393 SOL per closed trade.
> `cancel_trigger_order` takes only the authority and the order account — it consults no
> position state — so the wrapper is small. It belongs in Stage 1, not in a later cleanup.
>
> **C5 — the derived account count is 22, not ~21**, and 23–24 transaction keys. The extra
> slots over bare `open_position` are `trigger_order`, `trader`, `nox_config`, `mandate`,
> `trader_profile`, `mandate_signer` and `solfx_core_program`. Packet at the widest market is
> **959 bytes** against 1232 — the original ~958 estimate was right to one byte, for partly
> coincidental reasons.
>
> Two encoding details Stage 0 must pin, both cheaper to find now than in a devnet log:
> an absent oracle leg is passed to the CPI as `Some(solfx_core_program.to_account_info())`
> and **never** `None`, because `ToAccountMetas` emits a meta that `ToAccountInfos` does not
> back; and v1 transactions reject duplicate addresses, so Anchor's absent-`Option`
> convention (which emits the program ID, already present) makes a USD-quoted market
> unencodable as v1 — carry the two extra legs in `remaining_accounts` instead.

### 2.6 The evaluation engine (simulated, priced by SolFX's own code)

`Evaluation` holds a virtual USDC balance. `VirtualPosition` mirrors SolFX's `Position`. Every
virtual trade is priced by **the same functions a real fill uses**:

| Concern | Reused from |
|---|---|
| Price + all six gates | `solfx_core::oracle::load_validated_price` |
| Spread / execution price | `solfx_math::pricing` |
| Fees | `solfx_math::fees` |
| PnL, notional, conversion | `solfx_math::pnl` |
| Margin, equity, liquidation | `solfx_math::margin` |

So the claim is exact, not marketing: **the demo is not an approximation of a SolFX fill; it is
computed by identical code.** No token ever moves, so SolFX invariants I1, I2, I4–I8 are untouched by
construction — the strongest possible safety argument for the evaluation.

Two stages, each with its own rule set and a **minimum trade count (~10)** so the win rate
means something.

### 2.7 Statistics and tiering

SolFX stores **no** per-account PnL, trade count, win count, or drawdown — those exist only in
events. So NOXFUNDS must compute and store its own:

```rust
pub struct TraderProfile {
    authority: Pubkey, stage: Stage, tier: TraderTier,
    trades: u32, wins: u32, losses: u32,
    gross_profit: u64, gross_loss: u64,      // separate, so profit factor is derivable
    peak_equity: u64, max_drawdown_bps: u16, // updated on EVERY equity observation
    total_hold_slots: u128,                  // ÷ trades = avg hold, the anti-scalping stat
    largest_win: u64, largest_loss: u64,     // catches one-lucky-trade records
    ...
}
```

`peak_equity` and `max_drawdown_bps` update on every trade *and* on a permissionless
`observe_equity` crank, so drawdown cannot be hidden by simply not closing a losing position —
the gaming vector that matters most.

### 2.8 Capital, settlement and invariants

Investor USDC → `MandateVault` → SolFX `collateral_vault` via `deposit_collateral` (mandate PDA
signs). On settlement the mandate PDA calls `withdraw_collateral`, then splits:

- principal back to investor first
- profit above principal split `trader_split_bps` / remainder, minus NOXFUNDS's fee
- a loss simply means the investor gets back less than principal

NOXFUNDS needs its **own** invariant series (SolFX's I7 is a closed sum over exactly four
vaults and must not be disturbed):

- **N1** `Σ MandateVault balances + Σ capital deployed into SolFX == Σ principal − Σ settled`
- **N2** a `Mandate` in `Active` state always has a live SolFX `UserAccount` it authorises
- **N3** no `Evaluation` action ever touches a real token account (asserted structurally)
- **N4** `trader_split + investor_split + protocol_fee == realised profit`, exactly
---

## Part 3 — The rulebook

Every number below is a **proposal to confirm**, not a decision already taken. The industry
baseline is given beside each so you can see where we sit relative to FTMO-style firms.

### 3.1 The distinction that drives the design

A rule is only worth writing if we know *where* it can be enforced. Three places, and they are
not interchangeable:

| Where | What can be checked | Enforcement |
|---|---|---|
| **In the CPI, before the fill** | anything derivable from this trade + current state | **Preventive** — tx fails |
| **At close, before the CPI** | hold time, realised PnL of this trade | **Preventive** |
| **By the observe crank** | drawdown while holding, trading days, consistency | **Detective** — flags a breach |

The third category exists because of one unavoidable fact: **a trader who is losing can simply
stop trading.** No trade-time check ever fires again, so equity has to be sampled
independently. That is what `observe_equity` is for, and it is why R5 matters.

### 3.2 Evaluation — Phase 1 and Phase 2

Both phases run on a simulated $100,000 balance (scales with the tier the trader is attempting).

| Rule | Phase 1 | Phase 2 | Industry norm | Enforced |
|---|---|---|---|---|
| Profit target | **8%** | **5%** | 8% / 5% | crank |
| Max daily loss | **3%** | **3%** | 5% | CPI + crank |
| Max total drawdown (trailing from peak) | **6%** | **6%** | 10% trailing | CPI + crank |
| Minimum trades | **10** | **10** | 4–10 days | crank |
| Minimum distinct trading days | **5** | **5** | 4–10 | crank |
| Min hold per trade (anti-scalp) | **10 min** | **10 min** | varies | close-time |
| Min *average* hold across all trades | **45 min** | **45 min** | rare | crank |
| Consistency: no single day > 50% of target | **yes** | **yes** | common | crank |
| Max risk per trade | **1%** of balance at SL | **1%** | rare, but this is the point | **CPI** |
| Stop-loss mandatory | **yes** | **yes** | rare | **CPI** |
| Time limit | none | none | 30/60 days | — |

Three of these deserve their reasoning stated, because they are where we differ:

- **Max risk per trade is computed from the stop-loss**, not from position size. `size ×
  |entry − stop| ≤ 1% of balance`. This is the rule that actually encodes "risk management",
  and it is only checkable because the stop is mandatory and placed in the same transaction.
  No centralized firm can enforce it *before* the fill.
- **3% daily / 6% total is settled**, and is tighter than the 5%/10% norm. The reasoning: the
  investor's capital is real, and the product's whole claim is that the rules are believable.

  The cost is accepted with open eyes. At 1% risk per trade a 6% total drawdown means **six
  consecutive losses ends a run**, and at a 50% win rate a six-loss streak appears somewhere in
  a twenty-trade run roughly 15–20% of the time. So a meaningful share of *genuinely competent*
  traders will wash out on variance alone. That is a real cost to trader supply, and if
  applications dry up this is the first parameter to revisit — it lives in `NoxConfig` and is
  admin-updatable for exactly that reason.
- **No time limit.** Time limits exist to churn challenge fees. Removing it costs us nothing
  and is a genuine differentiator we can state plainly.

**"Swing not scalping" is encoded as three separate rules** — a per-trade floor, an average
floor, and minimum distinct days. The **distinct-days rule does the real work**: with 5 days
required, an evaluation spans a working week however fast the individual trades are. Hold time
only prevents scalping *within* a trade, which is why 45 minutes is sufficient and 4 hours was
overkill.

### 3.2.1 Minimum hold vs the mandatory stop — a contradiction, and its fix

These two rules fight each other. A stop that fires three minutes after entry breaks a
ten-minute floor **through no action of the trader's** — they were stopped out, which is
exactly what the rules told them to arrange.

> **Minimum hold applies only to voluntary closes.** A stop-out or a liquidation is exempt,
> and is excluded from the average-hold calculation entirely.

There is precedent in SolFX itself: Phase 7 exempted trigger execution from `min_hold_slots`
for this same reason ([trigger.rs](../programs/solfx-core/src/instructions/trader/trigger.rs)).
Without the exemption the rulebook would punish traders for the discipline it requires.

### 3.3 Funded mandate rules

Inherited from the evaluation, plus the investor's own constraints:

| Rule | Value | Enforced |
|---|---|---|
| Max notional per trade | set by investor (e.g. $1,000 on $3,500) | **CPI** |
| Max risk per trade | 1% of mandate equity at the stop | **CPI** |
| Max concurrent positions | **3** | **CPI** |
| Max total open notional | 3× mandate equity | **CPI** |
| Max daily loss | 3% of mandate equity | CPI + crank |
| Max total drawdown | 6% trailing from peak equity | CPI + crank |
| Stop-loss mandatory, max distance | 2% | **CPI** |
| Allowed markets | investor's chosen set | **CPI** |
| Min hold (voluntary closes only) | 10 min | close-time |

Breaching a **CPI** rule fails the transaction and nothing happens. Breaching a **crank** rule
moves the mandate to `Breached`, which stops new trades and begins wind-down.

### 3.4 Drawdown, defined precisely

This is the rule most easily fudged, so it is pinned exactly:

```
peak_equity   = max(peak_equity, equity_now)          // updated at every observation
drawdown_bps  = (peak_equity − equity_now) × 10_000 / peak_equity
```

`equity_now` uses `solfx_core::risk::assess`, marking on the **adverse** side — the same
number a liquidation would use. Two consequences to state openly:

- Drawdown is **observed**, not continuous. A spike between observations is not caught. The
  crank cadence *is* the honesty of the rule, so it is a published parameter, not an
  implementation detail.
- `peak_equity` only ever rises. A trader cannot reset it by withdrawing or by closing out.

---

## Part 4 — Tiers

### 4.1 Trader tiers

Assigned on evaluation completion, recomputed after every funded settlement. Thresholds are
proposals.

| Tier | Requires | Max mandate | Max concurrent mandates |
|---|---|---|---|
| **Bronze** | passed both phases | $10,000 | 1 |
| **Silver** | + 20 funded trades, win rate ≥ 45%, profit factor ≥ 1.2 | $25,000 | 2 |
| **Gold** | + 50 trades, PF ≥ 1.5, max DD ≤ 4% | $75,000 | 3 |
| **Platinum** | + 100 trades, PF ≥ 1.8, 2 settled mandates in profit | $200,000 | 5 |

**Win rate alone never determines a tier.** A 90% win rate with a 0.6 profit factor is a
trader who takes tiny wins and enormous losses — the single most common way a track record
lies. Profit factor and max drawdown are the load-bearing metrics; win rate is displayed
because traders and investors expect it.

### 4.2 Investor tiers

By cumulative capital deployed, not by wallet balance — the tier is earned, not bought.

| Tier | Cumulative deployed | Protocol fee | Perks |
|---|---|---|---|
| **Seed** | $2,500 min | 5% | 1 active mandate |
| **Backer** | $25,000 | 5% | 5 active mandates |
| **Partner** | $100,000 | 5% | 15 mandates, Gold+ traders |
| **Anchor** | $500,000 | 5% | unlimited, Platinum access, custom rulesets |

> **Withdrawn, 2026-09-15.** This column previously read 10 / 8 / 6 / 4 %, which contradicted
> both §5.1's worked example and the Decisions table — those say a **flat 5% of gross**, and
> the user confirmed 5% flat. The tiered figures were a leftover from a pre-5% draft. Tiers
> still differ on *how many mandates* and *which traders*, not on the fee. The fee lives in
> `NoxConfig` and is admin-updatable, so a tiered schedule can be switched on later without a
> program upgrade.

The **$2,500 minimum** is yours. It is worth keeping: below roughly that, a 1%-risk rule
produces position sizes smaller than SolFX's `min_position_size` on several markets, so the
mandate would be unfundable in practice rather than merely unattractive.

---

## Part 5 — Economics

### 5.1 Where money comes from and goes

| Flow | Who pays | Who receives | Decided |
|---|---|---|---|
| Evaluation stake | trader | escrow | **$50**, rising for larger evaluation sizes |
| Stake refund **on pass** | escrow | trader | **100%** |
| Stake **on failure** | escrow | treasury | forfeited |
| Performance fee | gross profit | treasury | **5%** |
| Profit split (of net) | — | trader / investor | **70 / 30** |
| Loss | investor's principal | — | investor bears it |
| SolFX trading fees | mandate equity | SolFX | ordinary market fees |

Worked example on your own numbers — investor funds **$3,500**, trader makes **$1,000** gross:

```
gross profit                          $1,000
− treasury performance fee (5%)       $   50
                                      -------
net profit                            $  950
  → trader   70%                      $  665
  → investor 30%                      $  285
investor receives $3,500 + $285    =  $3,785
```

The 5% comes off the **gross** first, so your 70/30 stays exactly 70/30 on what remains. The
alternative — a 70/25/5 three-way split — would quietly cut the investor to 25%.

**The stake is refunded in full on passing Phase 2.** This is worth stating loudly, because it
inverts the business model everyone assumes: a firm that keeps challenge fees earns most when
traders fail, which is the conflict every prop firm is accused of. Here the protocol earns
from traders *succeeding*, and the chain can prove it.

### 5.2 The treasury and the LP pool

Treasury receives the 5% performance fee and all forfeited stakes. **The treasury may add that
capital to SolFX's LP pool at its discretion**, via the ordinary `add_liquidity` instruction —
no change to SolFX required.

Two things to be clear-eyed about:

- **There is no donation path in SolFX.** `aum` moves only through `add_liquidity` (which mints
  shares), `remove_liquidity`, liquidation, and trade settlement — verified across
  [lp.rs](../programs/solfx-core/src/instructions/lp.rs) and
  [mod.rs](../programs/solfx-core/src/instructions/trader/mod.rs). So the treasury cannot hand
  the pool money without receiving shares back. What it gets is **protocol-owned liquidity**:
  the pool deepens, and NOXFUNDS's own position takes losses alongside every other LP. That
  is real alignment, even though NAV per share is unchanged for existing holders.
- **It is discretionary, so it is a promise rather than a mechanism.** Everything else in this
  design is enforced by code; this one is not. That is a deliberate choice for launch — it
  keeps the treasury flexible while volumes are small — but it is the single place where
  NOXFUNDS asks to be trusted, and it should be said out loud rather than glossed. It can be
  made rule-based later (e.g. "maintain LP ≥ X% of funded notional") **without touching
  SolFX**, because the rule would live in NOXFUNDS.

### 5.3 Is the protocol solvent?

Revenue is forfeited stakes plus 5% of winners' gross. Costs are refunds, rent,
and keeper gas. Nothing here requires NOXFUNDS to hold risk: **the protocol never takes the
other side of a trade.** The investor bears trading loss, SolFX's LP pool is the counterparty,
and NOXFUNDS only ever moves other people's money between escrows. That is what makes N1–N4
sufficient — there is no balance-sheet risk to model.

### 5.4 Integrity: Sybil resistance and copy-trading

The product's only asset is that **a track record means something**. Two attacks devalue it.

#### Sybil — farming records by volume

Wallets are free and the evaluation is simulated, so without a cost an attacker opens a
thousand wallets, trades randomly, and keeps whichever pass by variance. The result is dozens
of "verified, on-chain, auditable" records that are pure noise.

Worth being precise about the damage, because it is narrower than it first appears: **a fake
record cannot be cashed out.** The mandate PDA custodies the investor's capital, so a funded
impostor cannot steal it and earns only from profits they are not skilled enough to make. The
harm is to **signal quality** — investors back noise and lose money, and trust in the
marketplace dies. A trust problem, not a theft problem, and still worth preventing.

- **The $50 stake is the enforceable defence.** It is the only one the *program* can apply, it
  scales linearly with attempts regardless of how many wallets are used, and it costs an
  honest trader nothing because passing refunds it in full.
- **IP heuristics are a frontend speed bump, not a defence.** A Solana transaction carries no
  IP address, so the program cannot see one. Anyone submitting to a public RPC bypasses the
  check entirely, a VPN defeats it for pocket change, and carrier-grade NAT makes it
  false-positive real users who share a mobile IP. Collecting IPs also carries data-protection
  weight. Worth having against lazy spam; **must not carry weight the stake should carry.**

#### Copy-trading — farming records by mirroring

A trader mirrors a genuinely skilled trader's entries and inherits their record.

**This cannot be prevented in the UI.** Every NOXFUNDS trade is a public on-chain event the
moment it lands; anyone can watch the chain and mirror it without ever opening the website.
UI-level blocking would be theatre. Four things that do work, strongest first:

1. **Latency already makes it unprofitable.** The copier is always later — crosses the spread
   later and eats skew impact later. On a fast FX market that is real slippage, and over ten
   trades it shows up as a systematically worse profit factor than the trader being copied.
2. **Detect and label, do not ban.** An off-chain correlation score — *"8 of the last 10
   entries matched trader X within 30s, same pair, same direction"* — published on the
   profile. A ban needs certainty we will not have; a label does not, and it lets investors
   decide.
3. **Delay publication of open positions** in our UI; show them only after close. Stops the
   casual copier, which is most of them. Will not stop someone reading the chain.
4. **Tiers self-correct.** Tiers recompute after every settled mandate, so copying your way to
   funded and then trading badly costs the tier and the future mandates.

The two cases differ in severity: **copying during the evaluation is the real damage**, because
it manufactures a record from nothing. Once funded, a successful copy still earns its investor
money, so the harm is much smaller.

---

## Part 6 — Instruction inventory

```rust
// --- admin -------------------------------------------------------------------
initialize_config(params)                 update_tier_thresholds(params)
update_fee_params(params)                 set_guardian(pubkey)
pause() / unpause()

// --- evaluation (simulated; no token ever moves) ------------------------------
start_evaluation(stage, account_size)     // pays the challenge fee
eval_open_position(market, dir, size, stop_loss)
eval_close_position(market, nonce)
eval_observe_equity()                     // permissionless crank
claim_stage_pass()                        // verifies every rule, advances stage
expire_evaluation()                       // permissionless; breach ⇒ terminal

// --- investor ----------------------------------------------------------------
initialize_investor()
fund_mandate(trader, principal, rules)    // creates Mandate + MandateVault + SolFX account
request_settlement()                      // investor-initiated wind-down
claim_settlement()                        // after positions are flat

// --- funded trading (the CPI wrappers) ---------------------------------------
funded_open_position(market, nonce, dir, size, collateral, price_limit, stop_loss)
funded_reduce_position(market, nonce, size, price_limit)
funded_close_position(market, nonce, price_limit)
funded_modify_stop(market, nonce, new_stop)
funded_add_collateral / funded_remove_collateral

// --- keeper ------------------------------------------------------------------
observe_mandate_equity()                  // permissionless; updates peak, detects breach
flag_breach()                             // permissionless; Active → Breached
wind_down_mandate()                       // permissionless; closes positions via CPI
```

**Everything a mandate's survival depends on is permissionless.** `observe_mandate_equity`,
`flag_breach` and `wind_down_mandate` can all be called by anyone, which is the answer to R2 —
investor capital is never hostage to an absent trader or an absent operator. Each pays its
caller from the rent of the accounts it closes, the same mechanism Phase 7 settled on for
trigger orders.

---

## Part 7 — Mandate lifecycle

```
                    fund_mandate
                         │
                         ▼
                    ┌─────────┐   trader trades within the rules
                    │ Active  │◄──────────────────────────────┐
                    └────┬────┘                               │
             ┌───────────┼───────────┐              funded_open_position
             │           │           │                        │
   flag_breach()  request_settlement()  trader done          ─┘
   (rule broken)   (investor exits)     (voluntary)
             │           │           │
             ▼           ▼           ▼
        ┌──────────┐  ┌──────────────────┐
        │ Breached │─►│   WindingDown    │  positions closed by anyone
        └──────────┘  └────────┬─────────┘
                               │  all flat
                               ▼
                         ┌───────────┐
                         │  Settled  │  principal + split paid out
                         └───────────┘
```

`Breached` is distinct from `WindingDown` on purpose: it records **why** the mandate ended, on
chain, permanently. A trader's profile shows breached mandates, and that is exactly the
information an investor is choosing on. Collapsing the two states would let a rule violation
be indistinguishable from a voluntary exit.

---

## Part 8 — Events and errors

Events (the indexer's only source, matching SolFX's convention):

`EvaluationStarted` · `EvaluationTradeOpened` · `EvaluationTradeClosed` · `EquityObserved`
(equity, peak, drawdown_bps) · `StagePassed` · `EvaluationFailed` (with the rule that broke)
· `MandateFunded` · `FundedTradeOpened` (with the rule margins it passed) · `FundedTradeClosed`
· `MandateBreached` (rule, equity, drawdown) · `MandateSettled` (gross, fee, trader, investor)
· `TierChanged`

Errors — each names the rule, never a generic failure:

`TradeExceedsMandate` · `RiskPerTradeExceeded` · `StopLossRequired` · `StopTooFar` ·
`MarketNotPermitted` · `TooManyOpenPositions` · `DailyLossExceeded` · `DrawdownExceeded` ·
`MinimumHoldNotMet` · `MandateNotActive` · `NotTheTrader` · `InsufficientTraderTier` ·
`EvaluationIncomplete` · `ConsistencyRuleViolated`

A trader whose transaction fails must be able to read *which* rule stopped them. A generic
`RuleViolation` would make the product feel arbitrary, which is the opposite of the pitch.

---

## Part 9 — The marketplace (frontend)

Shares the Phase 8 SolFX frontend stack. Three surfaces:

1. **Trader dashboard** — evaluation progress against every rule as a live gauge, not
   pass/fail at the end. Shows **current tier, the exact thresholds for the next one, and
   which single metric is blocking it** — "profit factor 1.31, need 1.50 for Gold" is
   actionable in a way a badge is not. Plus funded mandate management and the copy-correlation
   score, shown to the trader before it is shown to investors.
2. **Investor marketplace** — the leaderboard. Sortable on profit factor, max drawdown, win
   rate, avg hold, trades, settled mandates. **Every figure links to the on-chain event that
   produced it** — that is the entire differentiator versus a prop firm's marketing page, and
   the UI has to make it obvious rather than merely possible.
3. **Public verification** — a permalink for any trader showing the full derivation of their
   stats from raw events, so a sceptic can recompute them.

---

## Part 10 — Staging

| Stage | Deliverable | Exit criteria |
|---|---|---|
| **0. Feasibility spike** | One throwaway instruction that CPIs `open_position` from a PDA authority, with pass-through `UncheckedAccount`s | **Fits the stack frame and the packet.** Measured, not argued. If this fails the design changes — nothing else is built first. |
| **1. Mandate primitive** | `NoxConfig`, `Mandate`, `MandateVault`, `funded_open_position` + close | A rule-violating trade **fails as a transaction**; a compliant one lands with its stop attached atomically; SolFX I1, I2, I4–I8 still hold |
| **2. The full rulebook** | Every Part 3 rule, split CPI vs crank | Each rule has a test that proves it blocks *and* one that proves it permits; each names its own error |
| **3. Evaluation engine** | `Evaluation`, `VirtualPosition`, simulated trade path | A simulated fill matches a real SolFX fill **to the unit** on the same oracle input; no token moves |
| **4. Track record & tiers** | `TraderProfile`, stats, `observe_equity` | Drawdown cannot be hidden by holding a loser; tier boundaries pinned exactly |
| **5. Investor side** | `InvestorAccount`, funding, settlement, splits, fees | N1–N4 hold under adversarial sequences; investor always recovers principal from a wound-down mandate |
| **6. Keeper** | Breach detection, permissionless wind-down | A breached mandate winds down with **neither** the trader nor the operator cooperating |
| **7. Marketplace** | The three frontend surfaces | Every displayed statistic is re-derivable from events by a third party |

---

## Part 11 — The hard problems, stated plainly

- **R1 — Stack frame. DOWNGRADED, 2026-09-15.** The wrapper needs **22** accounts, and
  SolFX's own failures were at 13 (unboxed `Account<T>`) and 18 (a wider struct plus a vault
  transfer) — while `LiquidatePosition` passes in CI today at **17** fully-boxed accounts. The
  accounts struct is not where the risk is: at 8 bytes per field it is ~176 bytes against a
  4,096-byte frame. The real cost is the two by-value `CpiContext`s in the handler, 1,392
  bytes, giving a peak of ~1,100–2,890. **Even the pessimistic bound clears by ~1,200 bytes.**
  The one genuinely unknowable part is whether LLVM reuses a stack slot between the two
  contexts, and that has a pre-committed two-line remedy — `#[inline(never)]` on each CPI
  call. Still measured in Stage 0; no longer "the one that could force a redesign". See §2.3.
- **R2 — A trader who walks away.** Answered: `observe`, `flag_breach` and `wind_down` are all
  permissionless and rent-paid, so anyone can free the investor's capital.
- **R3 — Adverse selection on SolFX's LPs.** Funded traders are pre-filtered for skill and
  SolFX's LP pool pays their profits. **Partially mitigated** by routing the 5% fee and
  forfeited stakes to the treasury, which maintains an LP position (§5.2). Be honest about the
  magnitude: if funded traders collectively profit $X the pool pays $X and sees at most
  $0.05X returned, so 5% offsets a twentieth of the gross. What actually protects the pool is
  that it also keeps every *losing* funded trader's money and earns spread and fees on all
  their volume — and industry data says most funded traders do not make money. **That is an
  empirical bet, not a proof**, and it stays the design's main open economic question. Monitor
  funded-cohort net PnL against the pool from day one.
- **R4 — Gaming the evaluation.** Both directions at once to farm trade count; holding losers
  to hide drawdown; a lucky single trade. Countered by min *average* hold, distinct trading
  days, the consistency rule, `largest_win` vs `gross_profit`, and `observe_equity`.
- **R5 — Drawdown is only ever observed.** Equity moves between transactions. Mitigated by
  publishing the crank cadence as a rule parameter rather than hiding it.
- **R6 — market bitmap width. RESOLVED:** `allowed_markets` is a `u128`, so 128 markets.
  SolFX lists 29 today and Tiers 4–6 plausibly reach 50–60, which left `u64` uncomfortably
  close to its ceiling. Migrating a bitmap once real investor capital sits in live `Mandate`
  accounts is painful, and the extra 8 bytes cost nothing on a new account type.
- **R7 — Regulatory.** No on-chain prop firm holds a licence; all register as technology
  platforms offering evaluation-based trading. Same posture, and worth a legal opinion before
  real investor money — not before devnet.

---

## Verification

- **Stage 0 is itself the verification** of the central assumption; it produces a measured
  packet size and a passing CPI, or it invalidates the design.
- Every stage: LiteSVM tests loading **both** `solfx_core.so` and `noxfunds.so`, exactly as
  `programs/solfx-core/tests/referral.rs` already does for the two-program case.
- **SolFX's `assert_invariants()` (I1, I2, I4–I8) runs after every NOXFUNDS action.** This is the
  guarantee that NOXFUNDS cannot corrupt SolFX — your constraint, enforced as a test rather
  than a promise.
- Every rule in Part 3 gets a **paired** test: one proving it blocks, one proving it permits.
  A rule with only the first is indistinguishable from a rule that blocks everything.
- Packet size and compute pinned per instruction, in the style of
  `programs/solfx-core/tests/compute_budget.rs`.
- **CI asserts no file under `programs/solfx-core/` or `programs/solfx-referral/` is modified.**

---

## Decisions

### Settled

| | Decision |
|---|---|
| **Min average hold** | 45 min (was 4 h); per-trade floor 10 min; **voluntary closes only** — stop-outs exempt |
| **Trader cost** | $50 refundable stake, rising with evaluation size; **refunded in full on passing Phase 2**; forfeited on failure |
| **Sybil defence** | The stake. IP heuristics are a frontend speed bump only — unenforceable on-chain |
| **Protocol fee** | **5%** of gross to treasury (was 10%), leaving 70/30 on the net |
| **LP mechanism** | Treasury holds the 5% + forfeited stakes and **adds to SolFX's LP at its discretion** via `add_liquidity`. Protocol-owned liquidity; no SolFX change |
| **Trader dashboard** | Tier, next-tier thresholds, and the single blocking metric |
| **Copy-trading** | Cannot be blocked in UI. Latency + correlation label + delayed publication + tier self-correction |
| **Drawdown** | **3% daily / 6% total trailing.** Tighter than the 5/10 industry norm; variance-washout cost accepted and recorded in §3.2 |
| **Market bitmap** | **`u128`** — 128 markets |
| **Product** | The prop firm described here. A mockup with a SWAP/LIQUIDITY/STAKE/GOVERNANCE nav is a **style reference only** |
| **Location** | Same repository, new `programs/noxfunds/`, one branch. `solfx-referral` is the template; CI asserts neither existing program is modified |
| **Treasury** | `7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs` — the program upgrade authority. Concern recorded and overridden: one key then holds both protocol revenue and the power to replace the program. `NoxConfig` field, admin-updatable |
| **Authority split** | `Mandate` holds the rules; a dataless `MandateSigner` PDA is the SolFX authority, signer and rent payer (C1) |
| **Rent float** | **0.0046354 SOL** per open position; ≈ **0.0148 SOL** at the three-position cap (C3) |

### Deferred, not open

**Tier thresholds** (Part 4) are still proposals, but they live in `NoxConfig` and are
admin-updatable, so they are tuned against real data rather than guessed now. Nothing in the
build depends on their final values.

**The plan is complete.** Next: SolFX to devnet, then Stage 0 — the spike that proves a
PDA-authority CPI into `open_position` fits the BPF stack frame.
