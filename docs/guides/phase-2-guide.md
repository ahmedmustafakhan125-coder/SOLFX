# Phase 2 Guide — Vault, Markets & the Validated Pyth Read Path

**Flowchart:** [`../diagrams/phase-2.png`](../diagrams/phase-2.png)
**Deliverable:** the first on-chain program (`programs/solfx-core`): protocol config, market
listing, collateral custody, and the single oracle choke point (ADR-006).
**Exit criteria:** direct, synthetic, EM and non-USD-quoted feeds all price correctly on
chain; invariant I1 holds. Met — 84 integration tests on the real BPF binary via LiteSVM.

---

## 1. The shape of the phase

Two ideas organise everything here:

**Code is the formula; markets are rows (§ 5.6).** The program knows nothing about EUR/USD.
Each market is a PDA created at runtime by one transaction — no redeploy, no migration, no
downtime. Twelve markets across four price/quote shapes are listed against one unchanged
binary in the test suite; Phase 9 repeats that proof on a live devnet deployment.

**One choke point for prices (ADR-006).** No instruction reads a price except through
`solfx_core::oracle`. One place to audit, one place to fix, and no instruction can
accidentally skip a check.

---

## 2. State accounts — every field is a decision

### `Protocol` (`["protocol"]`, singleton)

Admin (multisig on mainnet) · `pending_admin` (two-step transfer — a typo'd handover would
otherwise orphan the protocol) · `guardian` (pause-only hot key) · `usdc_mint` (fixed
forever; changing it would orphan every deposit) · `paused` · the four fee-split bps ·
**`total_user_collateral`** — the on-chain leg of invariant I1, maintained incrementally so
tests can compare three independent figures (vault token balance, Σ over accounts, this
counter) · `total_deposits` / `total_withdrawals` (I7) · bumps · 128 B reserved padding.

Methods: `fee_split()` / `set_fee_split()` (validates the 10,000-bps sum),
`record_deposit()` / `record_withdrawal()` (the withdrawal underflow gets its own error —
it means accounting has diverged from the token balance, which is worth distinguishing
from a generic overflow).

### `Market` (`["market", index]`, one per instrument — append-only)

The biggest struct, grouped:

- **Price sourcing:** `feed_kind` (`SpotFx | ContinuousIndex | Crypto` — selects the Phase 4
  regime), `price_source` (`Direct | Synthetic{invert_quote}`), three 32-byte feed ids
  (primary, secondary leg, quote-conversion), `quote_conversion_kind` (the C-3 direction —
  an id alone cannot say multiply-or-divide).
- **Session calendar:** `session_{open,close}_{dow,seconds}` in UTC. **Per market**, because
  Phase 0b measured EM pairs on local hours (USD/BRL 14–21 UTC); one global session would
  leave them open against a dead feed — C-1 on exactly the pairs where a devaluation gap is
  likeliest.
- **Weekend overrides:** four derated parameters, dormant until Q1b.
- **Risk parameters:** leverage cap, IMR/MMR bps, liquidation fee, OI caps per side,
  position size bounds — all *data*, retunable without an upgrade.
- **Oracle guards:** `max_staleness_seconds` (≤ 60 by protocol ceiling — every live feed
  publishes at 1 s, so more tolerance is tolerance for a *closed* feed),
  `max_conf_bps` (≤ 3,000), `max_deviation_bps`, and later `liquidation_max_conf_bps`.
- **Pricing:** `base_spread_bps`, `conf_spread_multiplier_bps`, `skew_impact_bps_per_unit`.
- **Fees:** `open/close_fee_rate` at 1e9 (0.8 bps must be expressible).
- **Funding state** (Phase 4 fills it) and **live state** (OI both scales, last/EMA price,
  fee total).

Key methods: `validate_risk_params()` — called on create *and* after every update so a
partial retune can never leave an unchecked configuration; among its rules, IMR must be
≥ `1e4 / max_leverage` (the two state one constraint and must agree), MMR < IMR (a position
must not be liquidatable at birth), liquidation fee < MMR (a liquidation must not
manufacture the debt it exists to prevent). `symbol_str()`, `effective_*()` (WeekendMode
derating accessors), `is_synthetic()`, `needs_quote_conversion()`, `validate_session()`.

`MarketStatus` with `allows_open()` (allow-list: Active, WeekendMode only — a new status is
closed to opens until someone opens it on purpose), `allows_close()`, and
`allows_liquidation()` (everything except `Initialized` — an underwater position must stay
liquidatable through a halt, or a halt converts bad positions into bad debt).

### `UserAccount` (`["user", authority]`)

`free_collateral` (genuinely free — isolated margin moves position collateral *out* of this
balance, so withdrawing it can never breach a position) · `open_positions` ·
**`referrer` — written once at creation, and no instruction anywhere mutates it.** That
immutability is the § 8.5 moat: the one thing every FX introducing broker complains about
is the broker reassigning their clients. `credit()` / `debit()` (debit fails as
`InsufficientCollateral`, not a generic overflow).

### `LpPool` + `InsuranceFund`

Created in `initialize_protocol` even though their logic lands in Phases 4–5, because the
instruction is one-time and the mint/vault addresses must never move.

### The four token vaults

`CollateralVault` (all user money — I1), `FeeVault` (treasury), `InsuranceVault` (I6),
`LpVault` (I2). All owned by PDAs; classic SPL Token only, hard-pinned by type —
Token-2022 transfer-fee/hook extensions would make credited ≠ received and silently break
I1 (threat T9).

---

## 3. The oracle choke point — `solfx_core::oracle`

`load_validated_price(market, primary, secondary?, quote_conv?, clock)` runs six gates:

| # | Gate | Rejects | Why |
|---|---|---|---|
| 1 | Ownership | account not owned by the Pyth receiver | Anchor's `Account<PriceUpdateV2>` enforces it — forged bytes in an attacker-owned account never deserialise |
| 2 | Feed identity | `feed_id` ≠ the market's configured id | a *genuine, fresh* SOL/USD update must not price EUR/USD |
| 3 | Verification | anything below `Full` | `Partial` lowers how many Wormhole guardians must collude |
| 4 | Freshness, **both ends** | `age > max_staleness` **or** future-dated beyond 5 s drift | the SDK's own helper checks only one side — a price stamped an hour ahead passes it for the next hour regardless of the market. C-1 through a different door |
| 5 | Confidence | `conf/price > max_conf_bps` | C-4: the band is worth percent-of-margin at leverage |
| 6 | Deviation | spot vs **Pyth's own EMA** > `max_deviation_bps` | using the message's EMA avoids the stored-reference deadlock: a self-maintained EMA can only refresh through the gate it feeds, so after a big genuine move nothing passes and the market wedges shut |

Synthetic markets validate **both** legs and compose via `compose_synthetic` (confidence
compounds **linearly**, not in quadrature — the legs share a currency by construction, so
independence does not hold and quadrature understates the band by up to 29%). Non-USD-quoted
markets validate a third feed for the conversion rate. Supplying an account the market does
not use is rejected — an ignored account is an unaudited one.

`observe_market_price` is the same read with confidence/deviation *measured but not
enforced* — the crank's variant (a price too uncertain to record could never trip a
breaker). Phase 4 adds the third variant for liquidation. `require_feed_id_set` rejects the
all-zero id — the on-chain half of the Phase 0b dead-feed lesson.

### `crank_market_price` (keeper, permissionless)

Reads through `observe`, records `last_price` / `ema_price`, and **trips a breaker** — the
market goes `Halted` on excessive deviation (and, since Phase 4, on blown-out confidence),
with a `CircuitBreakerTripped` event. Nothing in the crank can ever move a market *back* to
Active: reopening after a halt is a human decision.

---

## 4. The instruction set

**Admin:** `initialize_protocol` (one-time: config + all four vaults + LP mint; rejects a
non-6-decimal collateral mint — every quote figure would silently be off by a power of ten;
rejects any fee split where treasury ≥ LP), `initialize_market` (sequential indices; created
`Initialized` = not tradeable, so a typo'd feed id cannot be traded before review; rejects
`ContinuousIndex` pending Q1b), `update_market_risk_params` / `update_market_fee_params`
(full revalidation after every change), `set_market_status` (cannot hand-set the continuous
states; `Delisted` is terminal), `update_fee_splits`, `transfer_admin` / `accept_admin`
(two-step), `set_guardian` (admin-only — a stolen guardian key must not be able to make
itself permanent).

**Guardian — can restrict, never move funds, never release:** `emergency_pause` (blocks
opens; withdrawals and liquidations stay live), `halt_market`. `unpause` is **admin**-only:
the asymmetry means a stolen hot key's blast radius is "trading stops", which is
recoverable.

**User:** `initialize_user_account(referrer)` (binds the referrer forever; self-referral
rejected), `deposit_collateral`, `withdraw_collateral`. **Both work while paused** — a pause
that could hold user money would falsify the non-custodial claim; and blocking deposits
would push positions toward liquidation during exactly the incident the pause was called
for. No margin check is needed on withdrawal *because margin is isolated* (ADR-004): free
collateral is unencumbered by construction.

---

## 5. Financial logic introduced here

- **Invariant I1** (`CollateralVault == Σ free + Σ position margin`) is checked three ways
  after every instruction in the test harness — vault balance, account sum, and the
  protocol counter. Two figures agreeing can both be wrong the same way; three cannot,
  cheaply.
- **The stack-frame lesson:** 13 accounts on `initialize_protocol` overflowed the 4 KB BPF
  frame ("Access violation in stack frame 5"). Every `Account<'info, T>` is `Box`ed, and a
  guard test pins the account count so growth fails with a message instead of a runtime
  fault.
- **CU baselines:** the full six-gate read costs ~12k CU; each extra feed ~3.6k. That
  headroom is what makes Phase 3's 120k budget comfortable.

**Test cases:** [`../test-cases/phase-2-tests.md`](../test-cases/phase-2-tests.md)
