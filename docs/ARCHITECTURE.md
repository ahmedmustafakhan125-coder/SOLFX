# SolFX Protocol — Technical Architecture & Engineering Specification

**Version:** 1.1
**Date:** 2026-07-27
**Author/Lead:** Ahmed Mustafa Khalid
**Status:** Pre-implementation, Phase 0. Supersedes the sections of `SolFX_Decentralized_Forex_Brokerage_Proposal.pdf` marked as corrected below.
**Scope:** On-chain programs, financial engine, off-chain infrastructure, revenue model, security, and delivery plan for a decentralised leveraged FX and metals trading protocol on Solana.
**Companion:** [`FOREX-EXPLAINED.md`](FOREX-EXPLAINED.md) – the same material in plain English, assuming no FX or DeFi background. Read that first if any of this is unfamiliar.

**Changes in v1.1** (following competitive and oracle research):
- § 1 – GMTrade competitive reality; repositioned on EM FX, weekend metals, and brokerage UX
- § 2 C-1 – 24/7 is achievable for metals via Pyth Indices, still false for FX; replaced with a two-regime design
- § 3.5, § 5.6 – market coverage strategy (synthetic crosses to Exness parity) and market listing without redeploy
- § 7.3 – two per-market regime state machines keyed off `feed_kind`
- § 15 – roadmap restructured around the capstone-first track

---

## Table of Contents

1. [Reality Check & Positioning](#1-reality-check--positioning)
2. [Corrections to the v0 Proposal](#2-corrections-to-the-v0-proposal)
3. [Product Definition](#3-product-definition)
4. [Architecture Decision Records](#4-architecture-decision-records)
5. [On-Chain Program Architecture](#5-on-chain-program-architecture)
6. [Financial Engine – Exact Specification](#6-financial-engine--exact-specification)
7. [Risk Engine & Safety Systems](#7-risk-engine--safety-systems)
8. [Revenue Architecture](#8-revenue-architecture)
9. [Off-Chain Infrastructure](#9-off-chain-infrastructure)
10. [Frontend Architecture](#10-frontend-architecture)
11. [Repository Structure](#11-repository-structure)
12. [Testing Strategy](#12-testing-strategy)
13. [Security & Threat Model](#13-security--threat-model)
14. [Deployment & Operations](#14-deployment--operations)
15. [Delivery Roadmap](#15-delivery-roadmap)
16. [Risk Register](#16-risk-register)
17. [Open Decisions](#17-open-decisions)

---

## 1. Reality Check & Positioning

### 1.1 The competitive landscape, accurately

The v0 proposal states SolFX would be the first brokerage on Solana, and refers to "the Jupiter blockchain." Both are wrong, and the second error conceals a well-capitalised direct competitor.

- **Jupiter is not a blockchain.** Jupiter is a protocol suite *on Solana*. Jupiter Perps is a pool-backed perpetuals exchange (the JLP vault is the counterparty), trading crypto only.
- **A forex venue already exists on Solana, and it leads the chain.** **GMTrade** (formerly GMXSOL – GMX-lineage, deployed to Solana mainnet March 2025, rebranded November 2025) launched **Forex Perpetuals in January 2026** on EUR/USD, GBP/USD, AUD/USD and NZD/USD – precisely the pair set the v0 proposal targets.

**GMTrade, as of mid-2026:**

| Metric | Value |
|---|---|
| Position | **#1 perp DEX on Solana by volume** – ahead of Jupiter, Drift, Pacifica, Flash Trade |
| 24h volume | ~$554M (peak day ~$4.9B) |
| TVL | ~$15M |
| Fees / revenue (30d) | $2.57M / $643k |
| Markets | 86 pairs – crypto, FX, commodities, equities, indices |
| Leverage | up to 500x |
| Oracle | Chainlink Data Streams |
| Architecture | Isolated GM Pools + Global Liquidity Vault |

Drift, Jupiter, Zeta, Flash Trade and Pacifica remain crypto-only. Off-Solana, **Ostium** (Arbitrum) runs the same product shape – $50B+ cumulative volume, 26k traders, ~$56.6M TVL – and **Gains Network (gTrade)** has done so on EVM chains for years.

**Consequence: "first FX on Solana" is dead as a claim.** It is checkable in one search, and asserting it costs credibility on every other statement in the document.

### 1.2 The defensible position

Three gaps survive GMTrade, and together they are a real product:

**1. GMTrade is a perpetuals DEX, not a brokerage.** FX is 4 of 86 pairs, presented in a crypto-native terminal with points farming and 500x leverage. Nobody has built for the actual retail forex trader – lot sizing, pip-denominated P&L, MT5-familiar layout, transparent swap rates.

**2. Emerging-market FX is uncontested on every chain.** Pyth ships feeds for INR, IDR, PHP, KRW, TWD, CLP, COP and PEN (launched April 2025; SGX FX and Euronext FX added as publishers April 2026). **No leveraged venue anywhere lists USD/INR or USD/IDR.** Demand is structural: high crypto adoption, currency depreciation, capital controls, large remittance corridors.

**3. Weekend metals.** – **WITHDRAWN 2026-08-01 – the feed is not reachable. See the C-1 banner in § 2.** ~~**Pyth Indices** (10 June 2026) publishes proprietary **24/7** benchmarks for gold and silver – continuous pricing while the underlying market is shut, purpose-built for perpetuals settlement, already live at Coinbase, Kraken, dYdX and Nado. XAU/USD routinely out-trades EUR/USD at XM and Exness; a venue whose gold book stays open all weekend is a concrete switching reason for an audience whose broker goes dark Friday evening.~~
>
> The Indices product is real but is not distributed through the public Hermes catalogue; access is a commercial question tracked as **Q1b**. Gold remains a listed weekday market – and measured the tightest confidence of any feed in the set (1.43 bps p50) – but it is not a weekend product. **The remaining selling points are unaffected, and #1 and #2 were always the stronger pair.**

**Positioning statement to use everywhere** *(revised 2026-08-01 – the weekend-gold clause is struck; it is not true and must not ship)*:
> ~~SolFX is a non-custodial forex brokerage on Solana built for forex traders – lot-based sizing, pip-denominated P&L, transparent swap rates, and a verifiable partner-rebate ledger – offering gold that trades all weekend and the emerging-market pairs no venue on any chain lists.~~

> SolFX is a non-custodial forex brokerage on Solana built for forex traders – lot-based sizing, pip-denominated P&L, transparent swap rates, and a verifiable partner-rebate ledger – offering the emerging-market pairs no venue on any chain lists, with collateral that never leaves your own account.

**Note the moat is distribution, not code.** This protocol is replicable in months. A network of introducing brokers who trust the rebate ledger because they can audit it is not. See § 8.5.

### 1.3 What you are actually building, without the marketing layer

You are building a **B-book**. This needs to be understood clearly, because the whole risk architecture follows from it.

The v0 proposal says trades execute "against transparent, algorithmic liquidity vaults." That means: when a trader is long EUR/USD and wins, the LP vault pays. The vault is the counterparty to every trade and is structurally short the aggregate trader PnL. That is the definition of a B-book.

The honest and *more persuasive* pitch is not "we removed the counterparty conflict." It is:

> The counterparty is a public vault that anyone can join or exit, priced off an oracle no operator controls, with rules encoded in open-source programs that cannot be changed against you mid-trade, and collateral that never leaves your PDA until the math says it must.

That claim survives scrutiny. "No conflict of interest" does not, and a knowledgeable reviewer will hit it immediately.

---

## 2. Corrections to the v0 Proposal

These are ordered by how much damage they do if left unaddressed.

### C-1 – CRITICAL: "24/7" is true for metals and false for FX – and the difference is load-bearing

The proposal's selling point #4 is "24/7 Global Trading: Solana operates continuously 365 days a year." Solana does. The instruments do not – but the answer differs by asset class, and conflating them is what makes this critical.

**FX: not possible, and the failure mode is severe.** The interbank market closes Friday ~17:00 ET and reopens Sunday ~17:00 ET, plus a holiday calendar. Pyth's spot FX feeds follow the underlying: outside session hours they stop updating and report a non-trading status.

Trading against a frozen Friday price is a free-money machine. A trader who sees a weekend geopolitical event that will gap EUR/USD 150 pips at Sunday open opens maximum size at the stale price with **zero risk**, and the LP vault pays the entire gap. This single hole drains the vault.

> ## 🚨 C-1 REFUTED BY MEASUREMENT – 2026-08-01
>
> **The paragraph immediately below is contradicted by live data and must not be built on.**
>
> Measured against Hermes on Saturday 2026-08-01, 18:13–18:24 UTC: **all 137 `Metal` and
> `Commodities` feeds in Pyth's public catalogue were frozen** at the Friday 21:00 UTC close.
> XAU/USD, XAG/USD, XPT/USD, XPD/USD and both oil spot feeds – none publishing. BTC/USD, ETH/USD
> and SOL/USD sampled in the same requests were live at ~1 s, so this is not an outage on our side.
> The catalogue exposes a `Crypto Index` asset type but **no metals or commodity index of any kind**.
>
> **The product is real; the distribution is the problem.** Pyth Indices did launch 10 June 2026
> (metals added 24 June) – the claim below is not fabricated. But a sweep of all 3,056 catalogue
> feeds for any gold/silver/platinum/palladium symbol or description returns 29, of which exactly
> **4 were live during the closure – all tokenised-gold crypto tokens** (PAXG, XAUT, XAUM, XAGM),
> not a metals index. No Pyth-branded metals index is catalogued under any asset type.
>
> So **a Solana program cannot read it today**: `FeedKind::ContinuousIndex` has no metals feed to
> point at, and `WeekendMode` has nothing to govern. Access is a commercial question – entitlement,
> endpoint, or paid agreement – tracked as **Q1b**.
>
> **Consequences:** metals become session-bound like FX. Selling point #3 (§ 1), the positioning
> statement, and the `Continuous` row of the table below are all withdrawn pending **Q1b** – whether
> Pyth Indices is a separately entitled product. That is a conversation with Pyth, not a measurement.
>
> Keep `FeedKind::ContinuousIndex` and the `WeekendMode` machinery in the design: it costs nothing to
> reserve, `FeedKind::Crypto` still needs the continuous path, and if Q1b resolves favourably the
> feature turns on by changing one config field. **Build no metals-weekend behaviour against it.**
>
> Evidence and method: [`oracle-feasibility.md`](oracle-feasibility.md).

**Metals: genuinely possible, and proven in production.** ~~**Pyth Indices** (10 June 2026) publishes proprietary **24/7** benchmarks for gold and silver, aggregating venues and geographies to produce a continuous reference price where the underlying market is closed – built explicitly for perpetuals settlement. Coinbase (GOLD-PERP / SILVER-PERP, May 2026, 24/7/365, 25x), Kraken, dYdX and Nado already price against them. Binance and Crypto.com run comparable products.~~ **– unverified secondary claim; refuted for the public API. See banner above.**

**Required design – continuity is a per-market property driven by feed type, never a global rule:**

| Class | Feed | Regime |
|---|---|---|
| ~~**Continuous** – XAU/USD, XAG/USD~~ **(no feed exists – withdrawn)** | ~~Pyth 24/7 index~~ | ~~Trades 7 days; `WeekendMode` Sat–Sun~~ |
| **Continuous** – crypto only | Pyth crypto feeds | Natively 24/7; no weekend derating |
| **Session-bound** – FX majors, EM FX, **metals**, crosses | Pyth spot FX / Metal | Trades 24/5; halts outside session |

Implemented as a `feed_kind` discriminant on `Market` (§ 5.3) driving a two-regime state machine (§ 7.3). **If Pyth ships a 24/7 FX index, those pairs flip to continuous by changing one config field** – no code change. Phase 0 checks whether one exists yet.

**Weekend risk engineering is mandatory, not optional.** Weekend metals liquidity is thin and unhedgeable – LPs carry risk they cannot lay off. `WeekendMode` must automatically apply: max leverage halved · OI caps at 40% of weekday · widened `base_spread_bps` · tighter `max_conf_bps` · no new opens in the final 30 minutes before Monday spot reopen (index-to-spot convergence gap).

~~**Market it accurately:** *"Gold and silver trade every day of the week. Currency pairs follow global FX market hours."*~~

**Corrected 2026-08-01 – that sentence is now false and must not ship.** The accurate claim is
*"Gold, silver and currency pairs follow global market hours."* Which is, bluntly, what every
broker offers. **The weekend differentiator is gone unless Q1b rescues it**, and the positioning
falls back to emerging-market coverage, non-custodial collateral, and the auditable rebate ledger.

The lesson generalises beyond this feature: **a vendor's marketing claim is not a measurement.**
The 24/7-metals claim survived into a full protocol specification – a `feed_kind` discriminant, a
two-regime state machine, four `weekend_*` fields on `Market`, and a 7-day on-call requirement – before anyone asked the feed whether it publishes on a Saturday. Every future listing goes through
the Phase 9 checklist with live data first.

### C-2 – CRITICAL: the fee level makes you uncompetitive

The proposal specifies a 0.02% (2 bps) protocol execution fee.

| Venue | Effective round-trip cost on EUR/USD |
|---|---|
| XM / Exness (retail, standard account) | ~0.6–1.0 pip spread – **0.6–0.9 bps** |
| Jupiter Perps / GMX (crypto perps) | 6 bps per side – **12 bps** |
| SolFX as proposed | 2 bps per side – **4 bps** |

You would be **4–6x more expensive than the incumbent you are trying to displace.** Crypto perps get away with 6 bps because BTC moves 3% a day, so 12 bps is 4% of the daily range. EUR/USD moves ~0.5% a day, so 4 bps is 8% of the daily range – sixteen times more punitive in the terms that matter to a trader.

**Required:** target **0.8–1.5 bps per side**, tiered down by volume, plus a dynamic oracle-confidence spread (§ 6.6). See § 8 for how the revenue model still works at that level.

### C-3 – HIGH: the uPnL formula is only valid for USD-quoted pairs

The proposal gives:

```
uPnL_long = Size_lots × ContractSize × (P_oracle – P_entry)
```

For EUR/USD, XAU/USD and XAG/USD this is correct – the result is in USD, which is the collateral currency. For **USD/INR the result is in Rupees**, for USD/JPY in Yen, for EUR/GBP in Pounds. Booking those numbers as USDC silently mis-prices every position by the FX rate – for USD/INR at ~88, by a factor of 88.

**Required:** a `quote_conversion_feed: Option<FeedId>` on `Market`, applied uniformly in `math/pnl.rs`. `None` for USD-quoted markets (the fast path); `Some(feed)` triggers a conversion read. One code path, decided by configuration.

This must be built in Phase 3, not deferred – the EM pairs that constitute the product's differentiation (USD/INR, USD/IDR, USD/PHP, USD/KRW) are *all* non-USD-quoted, as is every synthetic cross (§ 3.5). Restricting v1 to USD-quoted pairs only would eliminate the strategy.

**Launch ordering mitigates the risk without deferring the capability:** Tier 1 markets (XAU/USD, XAG/USD, EUR/USD, GBP/USD) are all USD-quoted, so the conversion path is exercised by tests and a single non-USD market before it carries production weight.

### C-4 – HIGH: oracle confidence is not optional at FX leverage

At 200x leverage on EUR/USD, one pip (0.0001, ≈ 0.92 bps) is **1.84% of the trader's margin**. Pyth's EUR/USD confidence interval routinely sits in the ± 0.5–2 pip range, and widens sharply around news. So the *uncertainty band alone* is worth 1–4% of a maximum-leverage trader's margin.

If you execute at the mid price and ignore confidence, latency arbitrageurs will systematically open on the favourable side of the band and close inside it. This is the single most common way pool-backed perp protocols bleed out. It is not theoretical – it is what killed several GMX forks.

**Required:** every fill prices off the **trader-adverse edge of the confidence band**, plus a dynamic spread that scales with `conf/price`, plus a hard reject when `conf/price` exceeds a per-market ceiling. Specified in § 6.6.

### C-5 – HIGH: Pyth's integration model has changed

The proposal describes Pyth as streaming "directly into Solana memory slots every slot." That was the legacy push model on Pythnet. Current integrations use the **Pyth pull oracle** (`pyth-solana-receiver-sdk`): the client fetches a signed price update from Hermes, posts it as an account in the same transaction, and the program verifies and reads it with `get_price_no_older_than`.

This is not a footnote – it changes the transaction shape, the compute budget, the account list of every instruction, and the keeper design. The liquidator must post its own price update, which means liquidation transactions carry an extra ~40–60k CU and an extra signature.

**Required:** design every price-touching instruction around pull-oracle account plumbing from day one. Retrofitting this is a rewrite.

### C-6 – MEDIUM: components missing from the spec entirely

The v0 architecture table lists four components. A production perp protocol needs these, none of which appear:

| Missing | Why it is not optional |
|---|---|
| Funding / skew rate | Without it, the vault ends up structurally long or short the whole market with no mechanism to rebalance |
| Borrow / carry fee | This is the mechanism that charges for holding leveraged notional (the on-chain equivalent of an FX swap) |
| Insurance fund | Bad debt from gap events has to be absorbed somewhere or LPs eat it directly |
| Auto-deleveraging (ADL) | Last-resort backstop when the insurance fund is exhausted |
| Open-interest caps | Per-market and per-side limits, otherwise one whale sizes the vault out of existence |
| Trigger orders (TP/SL) on-chain | The UI section mentions stop-loss, but there is no on-chain order type to hold it |
| LP entry/exit cooldown | Without it, LPs JIT-deposit ahead of a known trader loss and JIT-withdraw ahead of a gain |
| Keeper infrastructure | Liquidations do not happen by themselves; someone has to run the bots |

All are specified below.

### C-7 – MEDIUM: the roadmap omits ~60% of the work

Phases 1–4 cover the happy path (contracts – margin – UI – devnet). Not in the plan: audits, fuzzing and invariant testing, keeper and indexer infrastructure, insurance fund seeding, LP bootstrapping, incident runbooks, key management, or a mainnet cutover. Revised plan in § 15.

---

## 3. Product Definition

### 3.1 One-sentence definition

SolFX is a non-custodial, oracle-priced, vault-backed perpetual swap protocol on Solana that offers leveraged exposure to foreign exchange rates, settled entirely in USDC.

### 3.2 What a trader does

1. Connects a Solana wallet (Phantom / Solflare / Backpack).
2. Deposits USDC into their own `UserAccount` PDA. **They retain withdrawal authority at all times, subject only to margin requirements.**
3. Selects a market (e.g. EUR/USD), a direction, a size in lots, and a leverage.
4. Submits `open_position`. The program prices off Pyth, checks margin, charges the open fee, and writes a `Position` PDA.
5. Optionally attaches take-profit / stop-loss trigger orders.
6. Closes manually, or a keeper closes them on a trigger or a liquidation.
7. Realised PnL settles to their `UserAccount`; they withdraw whenever margin allows.

### 3.3 What an LP does

1. Deposits USDC into the liquidity vault, receives `slpUSD` LP tokens representing a pro-rata claim.
2. Earns a configured share of every fee stream (§ 8.3).
3. Is the counterparty to aggregate trader PnL – earns when traders lose, pays when traders win.
4. Withdraws after a cooldown period, minus an exit fee.

### 3.4 v1 scope (explicit non-goals)

**In scope for v1:**
- USDC collateral only
- Isolated margin per position
- Two market regimes: continuous (metals) and session-bound (FX) – § 7.3
- Direct and synthetic price sources; quote-currency conversion – § 3.5
- Max 50x leverage on Tier 1, tiered down by asset class – § 6.3
- Market orders with slippage bounds; TP/SL trigger orders
- On-chain IB / referral programme – § 8.5

**Explicitly out of scope for v1:**
- Cross margin
- Indices, equities, energies
- Governance token and DAO
- Limit orders resting on an orderbook
- Mobile app

### 3.5 Market coverage strategy

**Benchmark:** XM lists 55 FX pairs; Exness lists 100+ plus 8 metals pairs (including XAU/EUR, XAU/GBP, XAU/AUD).

> ### ⚠️ Revised by measurement – see [`oracle-feasibility.md`](oracle-feasibility.md) § 4
>
> This section originally assumed Pyth's direct FX coverage was too thin for parity, making synthetic
> composition necessary. **Phase 0b measured the catalogue and that assumption was wrong.**
>
> Pyth carries **290 FX feeds**, ~180 of them crosses served natively – and they measure *tighter*
> than the majors (EUR/JPY 0.59 bps p50 vs EUR/USD 1.28 bps). Composing EUR/JPY from two feeds
> would produce a **worse** price than the one Pyth already publishes, while doubling the oracle
> failure surface.
>
> **v1 lists direct feeds only.** `price_source::Synthetic` stays in the enum – free to reserve, and
> wanted later for pairs Pyth lacks – but it is **off the Phase 2/3 critical path**, and Phase 3's
> exit criteria drop the synthetic-cross case.
>
> `quote_conversion_feed` is **unaffected and still mandatory** (C-3): nearly every listable market
> is `USD/XXX` or an `XXX/JPY` cross, so PnL lands in the quote currency rather than USDC.

**Measured listable set (~23 markets, provisional pending the weekend result):**

| Tier | # | Markets | Phase |
|---|---:|---|---|
| 1 | 2 | XAU/USD, XAG/USD – *conditional on Q1, the weekend measurement* | 4 |
| 1 | 2 | EUR/USD, GBP/USD | 4 |
| 2 | 5 | USD/JPY, USD/CAD, AUD/USD, USD/CHF, NZD/USD | 9 |
| 2 | 8 | EUR/JPY, GBP/JPY, CAD/JPY, CHF/JPY, EUR/GBP, EUR/AUD, EUR/CHF, AUD/JPY | 9 |
| 3 | 7 | EM: USD/MXN, USD/ZAR, USD/PHP, USD/INR, USD/TRY, USD/TWD, USD/KRW | 9 |
| 4 | 2 | XPT/USD, XPD/USD – minimum leverage, conf p95 18–29 bps | Post-devnet |
| 5 | – | BTC/USD, ETH/USD, SOL/USD | Any time – generic engine |

**Excluded by measurement:** USD/IDR (conf p95 30 bps) · USD/BRL, USD/COP, USD/CLP, USD/PEN
(publish ~hourly – cannot support liquidation) · USD/PKR, USD/NGN, USD/EGP, USD/VND, USD/BDT,
USD/LKR, USD/THB, USD/MYR (**listed in the catalogue but have never published**).

> **Listing rule, learned the hard way:** a symbol appearing in Pyth's catalogue is *not* evidence a
> feed exists. Validate against live data before every `initialize_market` call. Add to the Phase 9
> listing checklist.

**Synthetic composition – retained design, deferred.** When a pair is eventually needed that Pyth
does not carry:

```
EUR/GBP = (EUR/USD) ÷ (GBP/USD)
GBP/JPY = (GBP/USD) × (USD/JPY)
```

Costs, which would then need engineering: two oracle reads per price (both legs must pass § 7.1
independently); confidence compounds as ≈ `√(conf² + conf²)`, so such pairs need wider spreads
and lower leverage. Represented as `price_source: Direct | Synthetic { … }` on `Market` (§ 5.3).

**Expansion beyond Tier 1 requires zero new code**, provided the constraints in § 5.6 hold. That is
the payoff of building the engine asset-class-generic.

---

## 4. Architecture Decision Records

### ADR-001 – Counterparty model: single LP vault, not a CLOB

**Decision:** pool-as-counterparty. A single USDC vault backs every market.

**Alternatives:** central limit orderbook (Drift/Zeta style); virtual AMM.

**Rationale:** A CLOB needs market makers before it needs traders, and you have neither. A pool bootstraps liquidity from passive capital and gives instant fills at oracle price from day one – which is exactly the UX a retail FX trader expects (they have never seen an orderbook). gTrade and Ostium both chose this. The cost is that you carry directional risk, mitigated by skew funding + OI caps + insurance fund.

**Consequence:** the entire risk engine in § 7 exists because of this decision. Revisit only if you reach nine-figure OI.

---

### ADR-002 – Pricing: oracle mid, adjusted for confidence and skew

**Decision:** `exec_price = pyth_price ± (conf_spread + base_spread + skew_impact)`, always adverse to the trader.

**Rationale:** see C-4. Without confidence-adjusted pricing you are handing free optionality to anyone with a faster feed than your users.

**Consequence:** your quoted spread is variable and widens on news. This must be shown in the UI *before* order submission, or users will report it as slippage bugs.

---

### ADR-003 – Collateral: USDC only, single global vault

**Decision:** one collateral asset, one vault PDA, all markets share it.

**Rationale:** Multi-collateral means haircuts, collateral oracles, and liquidation of collateral itself – a large surface for a v1. USDC is the natural unit for FX because the majors are USD-quoted. A shared vault gives better capital efficiency than per-market vaults.

**Consequence:** a loss event in one market draws on liquidity that backs the others. Mitigated with per-market OI caps and per-market utilisation ceilings.

---

### ADR-004 – Margin: isolated per position

**Decision:** each `Position` PDA holds its own collateral. Liquidation touches only that position.

**Rationale:** Isolated margin is what retail FX traders already understand, it makes liquidation a single-account operation (cheap, ~<150k CU, no iteration over a portfolio), and it caps blast radius per position. Cross margin requires loading every position of a user in one transaction – expensive and account-limit-constrained on Solana.

**Consequence:** capital-inefficient for sophisticated users. Add cross margin in v2 as an opt-in account type.

---

### ADR-005 – Arithmetic: fixed-point integers, u128 intermediates, checked everywhere

**Decision:** No floats. Ever. Fixed precisions defined in § 6.1. All intermediates in `u128`/`i128`. `overflow-checks = true` in release. Every arithmetic op uses `checked_*` and returns a program error on failure.

**Rationale:** Float non-determinism across validators is a consensus-breaking bug class. Silent overflow in a financial program is a drain.

**Consequence:** rounding direction must be chosen explicitly at every division. **Rule: always round in the protocol's favour** – fees round up, payouts round down, margin requirements round up. Encode this in the math module's function names (`div_ceil_fee`, `div_floor_payout`).

---

### ADR-006 – Oracle: Pyth pull oracle with hard staleness and confidence gates

**Decision:** `pyth-solana-receiver-sdk`. Every price read goes through a single `oracle::load_validated_price()` helper enforcing:
- `publish_time` within `market.max_staleness_seconds`
- `conf / price <= market.max_conf_bps`
- price within `market.max_deviation_bps` of the on-chain EMA
- Pyth trading status == trading

**Rationale:** One choke point means one place to audit, one place to fix, and no instruction can accidentally skip a check.

**Consequence:** during high volatility some trades will be rejected. That is the correct behaviour. Surface it in the UI as "market conditions – spread too wide," not as a generic failure.

---

### ADR-007 – Market hours: two per-market regimes, selected by feed type

**Decision:** each market carries a `feed_kind` that selects one of two `MarketStatus` state machines, both driven by a keeper crank plus oracle trading status (§ 7.3):
- `SpotFx` – session-bound. Outside its own market hours: `Halted`.
- `ContinuousIndex` / `Crypto` – continuous. Metals derate to `WeekendMode` Sat–Sun and block opens in a `PreOpenWindow` before Monday convergence.

**Alternatives:** one global session rule (breaks weekend metals); trading everything 24/7 against last price (the C-1 vault-draining exploit).

**Rationale:** C-1. The two products have genuinely different feed availability, so one rule cannot serve both without either losing the differentiator or opening the exploit.

**Consequence:** every market must declare its own calendar – EM pairs follow *local* market hours, not the interbank week. And a market's regime is data, not code: if Pyth ships a 24/7 FX index, those pairs flip to continuous with no upgrade (§ 5.6).

---

### ADR-008 – Upgrade authority: Squads multisig, then timelock

**Decision:**
- Devnet: single dev keypair.
- Mainnet launch: 3-of-5 Squads V4 multisig holding upgrade authority and admin authority.
- Post-audit maturity: 48-hour timelock on parameter changes, 7-day on program upgrades. Emergency pause remains immediate via a separate `guardian` key that can *only* halt, never move funds.

**Rationale:** An upgradeable program with a single hot key means users are trusting you exactly as much as they trust XM. The non-custodial claim is void without this. The asymmetry – instant pause, delayed upgrade – is the standard pattern and is correct.

---

## 5. On-Chain Program Architecture

### 5.1 Programs

| Program | Purpose |
|---|---|
| `solfx_core` | Markets, vault, positions, margin, liquidation, fees. The whole financial engine. |
| `solfx_referral` | On-chain IB/affiliate rebate accounting. Separate so it can be upgraded and audited independently. |

Keeping referral separate matters: it is the growth engine (§ 8.5) and will iterate far faster than the financial core, which you want frozen after audit.

### 5.2 Account map

All PDAs derive from `solfx_core`.

| Account | Seeds | Approx. size | Notes |
|---|---|---|---|
| `Protocol` | `["protocol"]` | 400 B | Global config, authorities, pause flags, fee splits |
| `Market` | `["market", market_index: u16]` | 700 B | Per-pair config, risk params, OI, funding & borrow indices |
| `LpPool` | `["lp_pool"]` | 300 B | AUM accounting, LP mint, cooldown config |
| `LpVault` | `["lp_vault"]` | Token acct | USDC ATA owned by `LpPool` PDA |
| `LpMint` | `["lp_mint"]` | Mint | `slpUSD`, mint authority = `LpPool` PDA |
| `InsuranceFund` | `["insurance_fund"]` | 200 B | Balance accounting |
| `InsuranceVault` | `["insurance_vault"]` | Token acct | USDC ATA |
| `FeeVault` | `["fee_vault"]` | Token acct | Protocol treasury USDC ATA |
| `UserAccount` | `["user", authority]` | 400 B | Free collateral, position count, referral link, volume tier |
| `CollateralVault` | `["collateral_vault"]` | Token acct | Holds all user + position collateral |
| `Position` | `["position", user_account, market_index: u16, nonce: u8]` | 350 B | Isolated position |
| `TriggerOrder` | `["order", position, order_id: u8]` | 200 B | TP / SL |
| `LpWithdrawRequest` | `["lp_withdraw", authority]` | 150 B | Cooldown enforcement |

**Critical invariant:** `CollateralVault.amount == Σ(UserAccount.free_collateral) + Σ(Position.collateral)`. Assert this in tests after every instruction (§ 12.3).

### 5.3 Core state definitions

```rust
// state/protocol.rs
#[account]
pub struct Protocol {
    pub admin: Pubkey,                  // Squads multisig
    pub pending_admin: Pubkey,          // two-step transfer
    pub guardian: Pubkey,               // pause-only key
    pub usdc_mint: Pubkey,
    pub num_markets: u16,
    pub paused: bool,                   // global kill switch
    pub fee_split_lp_bps: u16,          // e.g. 5500
    pub fee_split_treasury_bps: u16,    // e.g. 2500
    pub fee_split_insurance_bps: u16,   // e.g. 1000
    pub fee_split_referral_bps: u16,    // e.g. 1000  (must sum to 10_000)
    pub bump: u8,
    pub _reserved: [u8; 128],           // forward-compat padding
}

// state/market.rs
#[account]
pub struct Market {
    pub market_index: u16,
    pub symbol: [u8; 16],               // b"EURUSD\0..."
    pub status: MarketStatus,

    // --- price sourcing (§ 3.5) ---
    pub feed_kind: FeedKind,            // drives the regime state machine (§ 7.3)
    pub price_source: PriceSource,      // Direct | Synthetic { base, quote }
    pub pyth_feed_id: [u8; 32],         // primary feed (base leg if synthetic)
    pub secondary_feed_id: [u8; 32],    // quote leg if synthetic, else zeroed
    pub quote_conversion_feed: [u8; 32],// non-USD-quoted markets (C-3), else zeroed

    // --- session config (session-bound markets only) ---
    pub session_open_dow: u8,           // UTC day-of-week
    pub session_open_seconds: u32,      // UTC seconds past midnight
    pub session_close_dow: u8,
    pub session_close_seconds: u32,

    // --- weekend overrides (continuous markets only, § 7.3) ---
    pub weekend_max_leverage: u16,
    pub weekend_oi_cap_bps: u16,        // % of weekday cap, e.g. 4000 = 40%
    pub weekend_spread_bps: u16,
    pub weekend_max_conf_bps: u16,

    // --- risk parameters (all admin-settable) ---
    pub max_leverage: u16,              // 50 => 50x
    pub imr_bps: u16,                   // initial margin ratio, 200 = 2%
    pub mmr_bps: u16,                   // maintenance margin ratio, 100 = 1%
    pub liquidation_fee_bps: u16,       // penalty on liquidation
    pub max_oi_long: u64,               // quote units
    pub max_oi_short: u64,
    pub max_position_size: u64,
    pub min_position_size: u64,

    // --- oracle guards ---
    pub max_staleness_seconds: u32,
    pub max_conf_bps: u16,              // reject if conf/price exceeds
    pub max_deviation_bps: u16,         // reject if price deviates from EMA

    // --- pricing ---
    pub base_spread_bps: u16,
    pub conf_spread_multiplier_bps: u16,
    pub skew_impact_bps_per_unit: u32,

    // --- fees ---
    pub open_fee_bps: u16,              // ~10 => 1.0 bps (uses 1e5 scale, see note)
    pub close_fee_bps: u16,

    // --- funding & carry (cumulative index model) ---
    pub cum_funding_long: i128,         // per BASE_PRECISION of size
    pub cum_funding_short: i128,
    pub cum_borrow_index: u128,
    pub last_funding_update_ts: i64,
    pub funding_rate_cap_per_hour: i64,
    pub carry_rate_per_hour: i64,       // interest-rate differential + markup

    // --- live state ---
    pub oi_long: u64,                   // quote units
    pub oi_short: u64,
    pub base_oi_long: i128,             // base units, for skew
    pub base_oi_short: i128,
    pub ema_price: i64,
    pub last_price: i64,
    pub total_fees_collected: u64,

    pub bump: u8,
    pub _reserved: [u8; 128],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum MarketStatus {
    Initialized,     // created, not tradeable
    Active,          // normal
    ReduceOnly,      // closes and liquidations only (pre-session-close, or winding down)
    Halted,          // nothing but liquidations of already-underwater positions
    GapWindow,       // session-bound: just reopened – closes + liquidations, no new opens
    WeekendMode,     // continuous: trading with reduced-risk weekend parameters
    PreOpenWindow,   // continuous: 30m before spot reopen – no new opens
    Delisted,        // settlement only
}

/// Determines which regime state machine the market follows (§ 7.3).
/// A new asset class is a new variant plus its regime rules – nothing else.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    SpotFx,           // session-bound: halts outside interbank/local market hours
    ContinuousIndex,  // 24/7 Pyth index (metals) – WeekendMode applies
    Crypto,           // natively 24/7, no weekend derating
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    Direct,
    /// price = base_leg (op) quote_leg. Confidence compounds – see § 3.5.
    Synthetic { invert_quote: bool },
}

// state/position.rs
#[account]
pub struct Position {
    pub user_account: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub direction: Direction,           // Long | Short

    pub size_base: u64,                 // BASE_PRECISION units of base currency
    pub entry_price: i64,               // PRICE_PRECISION
    pub collateral: u64,                // QUOTE_PRECISION (USDC)

    pub cum_funding_entry: i128,        // snapshot at open/last-settle
    pub cum_borrow_entry: u128,
    pub realized_funding: i64,          // accrued, settled into collateral on modify

    pub opened_at: i64,
    pub last_updated_at: i64,
    pub open_fee_paid: u64,
    pub referrer: Pubkey,               // Pubkey::default() if none

    pub bump: u8,
    pub _reserved: [u8; 64],
}
```

> **Note on fee precision:** 1.0 bps cannot be expressed in whole basis points at the granularity we need. Use a **`FEE_PRECISION` of 1e6 (i.e. units of 0.0001 bps)** for fee rates, or store fees in tenths of a bp. Do not store `open_fee_bps: u16` as literal bps – you cannot express 0.8 bps. **Decision: store all rates in `RATE_PRECISION = 1e9` fractional units** and name the fields `open_fee_rate` / `close_fee_rate`. The struct above uses `_bps` naming for readability; the implementation must use the finer scale.

### 5.4 Instruction set

#### Admin (signer = `Protocol.admin`)

| Instruction | Purpose |
|---|---|
| `initialize_protocol` | One-time. Creates `Protocol`, vaults, LP mint. |
| `initialize_market` | Adds a market with its Pyth feed and risk params. |
| `update_market_risk_params` | Leverage, IMR/MMR, OI caps, oracle guards. |
| `update_market_fee_params` | Fee rates, spreads. |
| `update_fee_splits` | LP / treasury / insurance / referral distribution. |
| `set_market_status` | Manual override of the session state machine. |
| `transfer_admin` / `accept_admin` | Two-step authority transfer. |
| `withdraw_treasury_fees` | Move `FeeVault` balance to a destination. |
| `deposit_insurance_fund` | Seed or top up the insurance fund. |

#### Guardian (signer = `Protocol.guardian`) – can only restrict, never move funds

| Instruction | Purpose |
|---|---|
| `emergency_pause` | Sets `Protocol.paused = true`. Blocks opens; liquidations and withdrawals of free collateral remain live. |
| `halt_market` | Single market to `Halted`. |

#### LP

| Instruction | Purpose |
|---|---|
| `add_liquidity(amount, min_lp_out)` | Mint `slpUSD` against current AUM/supply ratio. |
| `request_remove_liquidity(lp_amount)` | Starts the cooldown clock. Creates `LpWithdrawRequest`. |
| `remove_liquidity(min_usdc_out)` | After cooldown. Burns LP, pays USDC minus exit fee. |
| `cancel_remove_liquidity` | Aborts the request. |

#### Trader

| Instruction | Purpose |
|---|---|
| `initialize_user_account(referrer)` | Creates `UserAccount`, binds referrer permanently. |
| `deposit_collateral(amount)` | USDC → `CollateralVault`, credits free collateral. |
| `withdraw_collateral(amount)` | Free collateral → wallet. Blocked if it would breach margin. |
| `open_position(market_index, direction, size_base, collateral, max_price, referrer)` | Full open with slippage bound. |
| `increase_position(size_delta, collateral_delta, max_price)` | Adds to an existing position; recomputes weighted entry price. |
| `decrease_position(size_delta, min_price)` | Partial close, realises proportional PnL. |
| `close_position(min_price)` | Full close. |
| `add_collateral(amount)` / `remove_collateral(amount)` | Adjust position margin without changing size. |
| `place_trigger_order(kind, trigger_price, size)` | TP or SL. |
| `cancel_trigger_order(order_id)` | |

#### Keeper (permissionless)

| Instruction | Purpose |
|---|---|
| `execute_trigger_order(order)` | Fires TP/SL when the oracle crosses the trigger. Pays a keeper tip. |
| `liquidate_position(position)` | Closes a position with HF < 1. Pays liquidator a share of the penalty. |
| `crank_funding(market)` | Updates `cum_funding_*` and `cum_borrow_index`. |
| `crank_market_session(market)` | Advances `MarketStatus` based on oracle trading status + session calendar. |
| `auto_deleverage(position)` | Force-closes a profitable position when the insurance fund is exhausted. |
| `settle_bad_debt(position)` | Draws from insurance fund to cover a negative-equity close. |

### 5.5 Transaction shape

Every price-touching instruction carries a Pyth pull-oracle update. A typical `open_position` transaction:

```
[0] ComputeBudgetProgram::SetComputeUnitLimit(300_000)
[1] ComputeBudgetProgram::SetComputeUnitPrice(<dynamic priority fee>)
[2] pyth_solana_receiver::post_update      // posts signed price for EUR/USD
[3] solfx_core::open_position               // reads the posted update
[4] pyth_solana_receiver::close_update      // reclaim rent
```

Budget roughly: post_update ~45k CU, open_position ~120k CU (oracle verify + margin math + 3 account writes), close_update ~10k CU. Comfortably inside a single transaction. **Verify this empirically in Phase 2 and record actuals in `docs/compute-budget.md`** – CU regressions are how instructions silently start failing at scale.

**Synthetic markets cost roughly double on the oracle side** – two `post_update` calls, two verifications, plus a third if `quote_conversion_feed` is set. Worst case (synthetic + non-USD-quoted) is ~3 price updates in one transaction. Measure this in Phase 2; if it exceeds the transaction limit, the fallback is posting updates in a preceding transaction and reading cached accounts, which is why `max_staleness_seconds` exists.

### 5.6 Listing markets without redeploying – and the constraints that keep it that way

A common assumption is that deployed blockchain code is frozen, so every new trading pair means a redeployment. **On Solana neither half is true**, and the architecture depends on understanding why.

**Programs are upgradeable.** Solana programs deploy under `BPFLoaderUpgradeable`, which assigns an **upgrade authority**. That key can publish new bytecode to the **same program ID** – no address change, no user migration. (Contrast Ethereum, where contracts genuinely are immutable and upgradeability requires proxy patterns.) Authority policy is governed by ADR-008. Revoking it to achieve immutability is available as a later trust signal, not a starting position.

**But adding a market requires no upgrade at all.** Code and data are separate concerns:

- The **program** is a generic engine. It holds no knowledge that EUR/USD exists.
- Each **market** is a `Market` PDA created at runtime by the `initialize_market` instruction.

Listing EUR/JPY is one transaction – roughly 1 second, ~0.005 SOL of rent, no downtime, no migration, existing positions untouched:

```
initialize_market(
    market_index:           12,
    symbol:                 "EURJPY",
    feed_kind:              FeedKind::SpotFx,
    price_source:           PriceSource::Synthetic { invert_quote: false },
    pyth_feed_id:           <EUR/USD>,
    secondary_feed_id:      <USD/JPY>,
    quote_conversion_feed:  <USD/JPY>,
    max_leverage:           20,
    ...risk params
)
```

> The program is the *formula*; markets are *rows in a table*. Adding a row does not mean rewriting the formula.

**What genuinely cannot be undone: account layout.** Solana deserialises by byte offset, so shrinking, reordering, or retyping a field in an existing account struct corrupts every account of that type already on chain. Two mitigations, both already in § 5.3:

1. Every struct carries `_reserved: [u8; 128]`. New fields consume reserved bytes; nothing breaks.
2. **Append fields only, never reorder.** Enforced in code review – this is the one discipline with no automated safety net.

**Generic-engine constraints (Phase 2, non-negotiable).** Tiers 4–6 of § 3.5 are config-only *if and only if* these hold:

| Constraint | Rationale |
|---|---|
| No FX-specific assumption in `math/` or `state/` | A `Crypto` variant must need only regime rules, not new maths |
| All asset-class behaviour keyed off `feed_kind` | One dispatch point, not scattered conditionals |
| All price sourcing keyed off `price_source` | Synthetic support cannot be a special case bolted on later |
| All quote handling keyed off `quote_conversion_feed` | One PnL path for USD-quoted and non-USD-quoted alike |
| Risk parameters are per-market data, never constants | Leverage/MMR differ per asset class and must be tunable without an upgrade |

Cost of honouring these from the start: near zero. Cost of retrofitting: a rewrite. **Phase 9 proves the property empirically** by listing a new market on a running devnet deployment and trading it with no redeploy (§ 12.5).

---

## 6. Financial Engine – Exact Specification

### 6.1 Fixed-point precision constants

```rust
// math/constants.rs
pub const QUOTE_PRECISION: u128 = 1_000_000;          // 1e6  – USDC native decimals
pub const BASE_PRECISION:  u128 = 1_000_000_000;      // 1e9  – base currency units
pub const PRICE_PRECISION: u128 = 1_000_000_000;      // 1e9  – quote per 1 base
pub const RATE_PRECISION:  u128 = 1_000_000_000;      // 1e9  – fees, funding, carry
pub const BPS_PRECISION:   u128 = 10_000;

// Derived: converts (base × price) into quote units.
pub const NOTIONAL_DIVISOR: u128 =
    BASE_PRECISION * PRICE_PRECISION / QUOTE_PRECISION; // = 1e12

pub const LOT_SIZE_BASE: u128 = 100_000 * BASE_PRECISION; // 1 standard lot = 100,000 base
```

**Why 1e9 for price:** EUR/USD at 1.08543 stores as `1_085_430_000`. One pip (0.0001) = `100_000` units, one pipette = `10_000` units. That gives 10,000 ticks of sub-pip resolution – enough that rounding never dominates a 1 bp fee. `i64::MAX` is 9.2e18, so even USD/JPY at 157 (`157_200_000_000`) has eleven orders of magnitude of headroom.

**Why base size in base-currency units, not lots:** lots are a presentation concept. Storing base units lets the engine handle micro-lots and arbitrary sizes uniformly; the UI multiplies by `LOT_SIZE_BASE` for display.

### 6.2 Notional value

```
notional_quote = size_base × price / NOTIONAL_DIVISOR
```

```rust
pub fn notional(size_base: u64, price: i64) -> Result<u64> {
    let n = (size_base as u128)
        .checked_mul(price as u128).ok_or(SolfxError::MathOverflow)?
        .checked_div(NOTIONAL_DIVISOR).ok_or(SolfxError::MathOverflow)?;
    u64::try_from(n).map_err(|_| SolfxError::MathOverflow.into())
}
```

**Worked check:** 1 standard lot of EUR/USD at 1.08543.
`size_base = 100_000 × 1e9 = 1e14`. `price = 1_085_430_000`.
`1e14 × 1.08543e9 / 1e12 = 1.08543e11` = `108_543_000_000` quote units = **$108,543.00**. ✓

### 6.3 Margin

```
initial_margin_required  = notional × imr_bps / 10_000      (round UP)
maintenance_margin       = notional × mmr_bps / 10_000      (round UP)
max_leverage_check       = notional / collateral <= market.max_leverage
```

Recommended launch parameters, **tiered by asset class**. Final values are set from Phase 0 confidence measurements, not from this table – these are the starting envelope.

| Class | Markets | Max lev | IMR | MMR | Liq. penalty | Rationale |
|---|---|---|---|---|---|---|
| **Metals (weekday)** | XAU/USD, XAG/USD | 50x | 2.00% | 1.00% | 0.50% | Deep liquidity, ~1–1.5%/day range |
| **Metals (weekend)** | same, `WeekendMode` | **25x** | 4.00% | 2.00% | 0.75% | Thin, unhedgeable, no spot market to lay off risk |
| **FX majors** | EUR/USD, GBP/USD | 50x | 2.00% | 1.00% | 0.50% | Tightest spreads, deepest feeds |
| **FX majors (2nd tier)** | USD/JPY, USD/CHF, USD/CAD, AUD/USD, NZD/USD | 30x | 3.33% | 1.67% | 0.60% | Slightly wider confidence |
| **Synthetic crosses** | EUR/GBP, EUR/JPY, GBP/JPY… | **25x** | 4.00% | 2.00% | 0.75% | Two oracle legs; confidence compounds (§ 3.5) |
| **EM FX** | USD/INR, USD/IDR, USD/PHP, USD/KRW… | **10–20x** | 5.00–10.00% | 2.50–5.00% | 1.00% | Managed floats; central-bank devaluation gaps |

**Why EM leverage is a fraction of majors.** These are managed or heavily-intervened currencies. A policy change produces a discontinuous jump, not a slide – the risk shape is the 2015 CHF depeg, not a normal trading range. No liquidation engine outruns a gap; the only defence is requiring enough margin that the gap does not exhaust it. Treat any pressure to raise EM leverage for competitive reasons as a request to accept bad debt.

**Size-tiered MMR** (mandatory – a $50k position and a $5M position are not the same risk):

| Notional | MMR multiplier |
|---|---|
| < $100k | 1.0x |
| $100k – $1M | 1.5x |
| $1M – $5M | 2.5x |
| > $5M | 4.0x |

Start conservative. Every leverage increase is a one-way door in terms of user expectation – raising 50x → 100x is easy PR, lowering 100x → 50x looks like distress.

### 6.4 Unrealised PnL

Corrected and unified for both directions (see C-3 – valid only when quote currency == USDC):

```
sign = +1 for Long, −1 for Short

uPnL_quote = sign × size_base × (price_exit – price_entry) / NOTIONAL_DIVISOR
```

```rust
pub fn upnl(pos: &Position, exit_price: i64) -> Result<i64> {
    let delta = (exit_price as i128)
        .checked_sub(pos.entry_price as i128).ok_or(SolfxError::MathOverflow)?;
    let signed = match pos.direction {
        Direction::Long  => delta,
        Direction::Short => -delta,
    };
    let pnl = (pos.size_base as i128)
        .checked_mul(signed).ok_or(SolfxError::MathOverflow)?
        .checked_div(NOTIONAL_DIVISOR as i128).ok_or(SolfxError::MathOverflow)?;
    i64::try_from(pnl).map_err(|_| SolfxError::MathOverflow.into())
}
```

**Worked check:** long 1 lot EUR/USD, entry 1.08543, now 1.08643 (+10 pips).
`1e14 × 1e6 / 1e12 = 1e8` = `100_000_000` quote units = **+$100.00**. Correct – 1 standard lot, 10 pips, $10/pip. ✓

### 6.5 Account equity and health factor

The v0 formula `HF = (Collateral + uPnL) / (MMR × NV)` is structurally right but omits accrued costs, which at high leverage are the difference between solvent and not.

```
accrued_carry   = size_base × (market.cum_borrow_index – pos.cum_borrow_entry) / RATE_PRECISION
accrued_funding = size_base × (market.cum_funding_side – pos.cum_funding_entry) / BASE_PRECISION
close_fee_est   = notional_now × close_fee_rate / RATE_PRECISION

equity = collateral + uPnL – accrued_carry – accrued_funding – close_fee_est

maintenance_margin = notional_now × mmr_effective / BPS_PRECISION

HF = equity × BPS_PRECISION / maintenance_margin      // scaled: HF < 10_000 means < 1.0
```

Liquidatable when `equity < maintenance_margin`. Compute it as that comparison directly rather than as a divided ratio – it avoids a division and a divide-by-zero branch.

**Cumulative index model – why:** you cannot iterate every open position on-chain to charge funding. Instead the market holds a monotonically accumulating index; each position stores its snapshot at open; the difference is what it owes. This is the standard approach (Drift, Perpetual Protocol, Aave all use it) and it makes funding O(1) per position.

### 6.6 Execution price (the anti-toxic-flow layer)

This is the most important function in the protocol. Get it wrong and the vault leaks continuously.

```
conf_ratio      = pyth_conf × BPS_PRECISION / pyth_price

// hard reject
require(conf_ratio <= market.max_conf_bps)

conf_spread     = conf_ratio × market.conf_spread_multiplier_bps / BPS_PRECISION
skew_before     = base_oi_long – base_oi_short
skew_after      = skew_before + signed_size_delta
skew_impact     = market.skew_impact_bps_per_unit × (|skew_after| – |skew_before|) / OI_SCALE

total_spread_bps = market.base_spread_bps + conf_spread + max(skew_impact, 0)

// always adverse to the trader
exec_price = if (opening Long) or (closing Short):
                 pyth_price × (1 + total_spread_bps / 10_000)   // pay the ask
             else:
                 pyth_price × (1 – total_spread_bps / 10_000)   // hit the bid
```

Three things this buys you:

1. **`conf_spread`** makes the protocol charge more precisely when it knows less. During an NFP release when Pyth's confidence blows out to 5 pips, the spread widens automatically instead of the vault eating the uncertainty.
2. **`skew_impact`** makes it progressively expensive to push the book further one-sided, and free (or rebated) to trade against the skew. This is your primary inventory-management tool given ADR-001.
3. **Adverse-side selection** removes the free half-spread option that latency arbitrageurs harvest.

**Additionally required – minimum hold time or asymmetric fee.** A trader who opens and closes within one slot pays two fees but takes zero risk, and if your spread is ever narrower than the true market spread they profit risklessly. Enforce `min_hold_slots` (suggest 2–5 slots – 1–2s) or make the close fee waive only after N slots. This is cheap to implement and closes a real hole.

### 6.7 Funding and carry

Two distinct mechanisms – do not conflate them:

**Carry (the FX swap equivalent).** Real FX charges/pays overnight interest based on the rate differential between the two currencies. Long EUR/USD when EUR rates < USD rates means you pay. Legacy brokers apply an opaque markup here – this is one of the four vulnerabilities the proposal correctly identifies.

```
carry_rate_per_hour = (rate_base – rate_quote) / (365 × 24) + protocol_markup_per_hour
cum_borrow_index   += carry_rate_per_hour × elapsed_hours
```

Store `rate_base` and `rate_quote` as admin-settable per-market parameters, updated when central banks move (a handful of times a year). Publish them on-chain and in the UI. **This alone is a real, concrete improvement over XM/Exness** – a trader can verify the exact swap rate and markup instead of discovering it after the rollover.

**Funding (skew rebalancing).** Pool-as-counterparty means aggregate skew is the vault's directional risk. Funding pays traders to correct it.

```
skew_ratio   = (base_oi_long – base_oi_short) / max(base_oi_long + base_oi_short, 1)
funding_rate = clamp(skew_ratio × k, −cap, +cap)     // per hour

// positive rate: longs pay shorts
cum_funding_long  += funding_rate × elapsed_hours
cum_funding_short -= funding_rate × elapsed_hours
```

Funding is a pure transfer between traders – it never touches LP capital and must net to zero across the market. Assert that as a test invariant.

### 6.8 Liquidation

```
1. Load position, validate oracle.
2. equity = collateral + uPnL – accrued_carry – accrued_funding      (excl. close fee)
3. require(equity < maintenance_margin)                              else NotLiquidatable
4. exit_price = execution_price(adverse side, includes spread)
5. penalty = min(equity_positive, notional × liquidation_fee_bps / 10_000)
6. Split penalty:  liquidator 40% | insurance 40% | treasury 20%
7. remaining = equity – penalty
   if remaining > 0:  credit to UserAccount.free_collateral
   if remaining < 0:  BAD DEBT – call settle_bad_debt, draw from insurance fund
8. Update market OI, close Position account, refund rent to user.
9. Emit LiquidationEvent.
```

**Partial vs full:** full close for positions under $250k notional (simpler, cheaper, and at 50x the position is small in absolute terms). Above that, partial-close down to `HF = 1.3` to reduce market impact on the vault. Implement full-close in v1, partial in v1.1.

**Liquidator incentive sizing.** Compute-unit cost of a liquidation transaction is ~200k CU plus priority fee – call it $0.002 at normal congestion. A 40% share of a 0.5% penalty on a $10,000 position is $20. Enormous margin, which is what you want: liquidations must be profitable even during the congestion spikes that accompany the volatility that causes them. Do **not** scale the reward down to look cheap; an unprofitable liquidation is an unliquidated position, and unliquidated positions are how vaults die.

### 6.9 Bad debt waterfall

When a position closes with negative equity (a gap through the liquidation price):

```
1. Insurance fund covers the shortfall.
2. If insurance fund is exhausted – ADL: force-close the most profitable
   opposing positions, ranked by (uPnL % of collateral) descending,
   at a price that socialises the shortfall.
3. If ADL is insufficient – LP vault NAV absorbs the remainder pro-rata.
```

Traders must be told ADL exists, in plain language, before they open a position. Every venue that hid it and then used it got destroyed on social media.

**Insurance fund seeding:** target 2% of maximum OI cap at launch, funded from your own capital, then grown from the 10% insurance share of fees. Do not launch with an empty insurance fund – the first gap event will hit LPs directly and you will lose them permanently.

---

## 7. Risk Engine & Safety Systems

### 7.1 Oracle guards – the single choke point

```rust
// math/oracle.rs – ALL price reads go through here. No exceptions.
pub fn load_validated_price(
    update: &PriceUpdateV2,
    market: &Market,
    clock: &Clock,
) -> Result<ValidatedPrice> {
    let feed_id = market.pyth_feed_id;
    let p = update.get_price_no_older_than(
        clock, market.max_staleness_seconds as u64, &feed_id
    )?;

    let price = normalize_to_price_precision(p.price, p.exponent)?;
    let conf  = normalize_to_price_precision(p.conf as i64, p.exponent)?;

    require!(price > 0, SolfxError::InvalidOraclePrice);

    let conf_bps = (conf as u128 * BPS_PRECISION / price as u128) as u16;
    require!(conf_bps <= market.max_conf_bps, SolfxError::OracleConfidenceTooWide);

    if market.ema_price > 0 {
        let dev = ((price - market.ema_price).abs() as u128 * BPS_PRECISION
                   / market.ema_price as u128) as u16;
        require!(dev <= market.max_deviation_bps, SolfxError::OracleDeviationTooLarge);
    }

    Ok(ValidatedPrice { price, conf, conf_bps, publish_time: p.publish_time })
}
```

Suggested starting values: `max_staleness_seconds = 10`, `max_conf_bps = 15` (EUR/USD), `max_deviation_bps = 300`.

### 7.2 Circuit breakers

| Trigger | Action |
|---|---|
| `conf_bps > max_conf_bps` | Reject the individual trade |
| Price deviation > `max_deviation_bps` vs EMA | Market → `Halted`, guardian alert |
| Oracle stale > `max_staleness_seconds` | Reject; if > 60s, market → `Halted` |
| Vault utilisation > 80% | New opens on the crowded side rejected |
| Insurance fund < 25% of target | Market → `ReduceOnly` |
| Single-block realised LP loss > 3% of AUM | Global pause, guardian alert |
| OI cap reached | Reject opens on that side only |
| Continuous-market index stale in `WeekendMode` | Market → `Halted`. 24/7 is a property of the feed, not a promise we make on its behalf |
| Synthetic market – **either** leg fails validation | Reject; halt if the failure persists |
| Monday convergence gap > `max_deviation_bps` | Extend `PreOpenWindow`, guardian alert |

Implement each of these as an explicit named error so the frontend can render a real message.

### 7.3 Market regime state machines (resolves C-1)

**Two regimes, selected by `market.feed_kind`.** One code path, one cranker, different transition rules. This is what makes the weekend-metals feature possible without opening the FX stale-price hole.

#### Regime A – Session-bound (`FeedKind::SpotFx`)

```
                  ├─────────────┐
                  │Initialized  │
                  ├──────┬──────┘
                         │ admin activates
                         ▼
   ├──────────┬ ├─────────────────────┐
   │          │ │      Active         │  full trading
   │          │ ├──────────┬──────────┤
   │          │           │ T−15min to session close
   │          │           ▼
   │          │ ├─────────────────────┐
   │          │ │   ReduceOnly        │  closes + liquidations only
   │          │ ├──────────┬──────────┤
   │          │           │ session close OR oracle non-trading
   │          │           ▼
   │          │ ├─────────────────────┐
   │          │ │      Halted         │  nothing; positions frozen
   │          │ ├──────────┬──────────┤
   │          │           │ session open AND oracle trading AND conf OK
   │          │           ▼
   │          │ ├─────────────────────┐
   │          │ │    GapWindow        │  closes + liquidations, NO new opens,
   │          │ │   (~5 minutes)      │  widened spread, gap-risk repricing
   │          │ ├──────────┬──────────┤
   └──────────┴─┴──────────┴──────────┘ conf normalises
```

**Each market carries its own calendar – they are not one shared session.** FX majors follow the interbank week (Sun ~17:00 ET – Fri ~17:00 ET). **EM pairs follow their local markets**: USD/INR on Indian hours, USD/KRW on Korean, USD/IDR on Indonesian, USD/PHP on Philippine. Assuming a single global session would leave EM markets open against a dead feed – the exact C-1 exploit, on the pairs where a devaluation gap is most likely.

Synthetic crosses take the **intersection** of both legs' sessions, and halt if *either* leg is closed.

#### Regime B – Continuous (`FeedKind::ContinuousIndex`, `FeedKind::Crypto`)

```
   ├──────────┬ ├─────────────────────┐
   │          │ │      Active         │  full trading, weekday parameters
   │          │ ├──────────┬──────────┤
   │          │           │ Fri spot close (metals only; Crypto never leaves Active)
   │          │           ▼
   │          │ ├─────────────────────┐
   │          │ │   WeekendMode       │  full trading, DERATED parameters:
   │          │ │                     │    • max leverage halved
   │          │ │                     │    • OI caps → 40% of weekday
   │          │ │                     │    • widened base spread
   │          │ │                     │    • tighter max_conf_bps
   │          │ ├──────────┬──────────┤
   │          │           │ T−30min to Monday spot reopen
   │          │           ▼
   │          │ ├─────────────────────┐
   │          │ │  PreOpenWindow      │  closes + liquidations, NO new opens
   │          │ │   (~30 minutes)     │  (index-to-spot convergence gap)
   │          │ ├──────────┬──────────┤
   └──────────┴─┴──────────┴──────────┘ spot reopens, conf normalises
```

`FeedKind::Crypto` markets stay in `Active` permanently – there is no underlying session to converge to, so neither `WeekendMode` nor `PreOpenWindow` applies.

**Why `WeekendMode` derates rather than simply staying open.** Over the weekend there is no spot market, so LPs hold risk they cannot hedge at any price, on thinner participation, with the certainty of a convergence move on Monday. Same instrument, materially worse risk – the parameters must say so. Shipping weekend trading on weekday parameters is how you turn a differentiator into a bad-debt event.

#### Determination logic

In `crank_market_session`, belt-and-braces – two independent signals:

1. **Primary:** Pyth feed trading status and `publish_time` freshness. If the feed is not publishing, the market is closed. Handles holidays automatically with no calendar to maintain.
2. **Secondary:** the on-chain per-market session calendar (`session_open_dow` / `session_open_seconds` / `session_close_dow` / `session_close_seconds`, UTC) as a fallback when oracle status is ambiguous.

**Trading restricts if *either* signal says closed. Fail closed, always.** For continuous markets the primary signal governs `WeekendMode` entry, and a stale index feed still forces `Halted` – 24/7 pricing is a property of the feed, not a promise the protocol makes on its behalf.

**Risk disclosure (product requirement, not optional):**
- Before a Friday FX trade: the position will be frozen over the weekend and may gap on reopen.
- Before a weekend metals trade: derated parameters are in force, and the Monday convergence carries gap risk.

Both risks exist at legacy brokers too. The difference is that we state them up front rather than after the fact.

### 7.4 Open interest management

- **Per-market absolute caps** on each side, sized against vault AUM: `max_oi_per_side <= 3 × sqrt(AUM)` as a starting heuristic, hard-capped at 15% of AUM.
- **Global utilisation cap:** `Σ(max_loss_exposure) / AUM <= 0.60`.
- **Per-user concentration cap:** no single user > 10% of a market's OI on one side.
- **Skew cap:** if `|oi_long – oi_short| / (oi_long + oi_short) > 0.6`, block further opens on the heavy side entirely (funding alone is too slow a lever at extremes).

---

## 8. Revenue Architecture

This is the section that answers *"how do I earn from this?"*, and it needs one insight up front.

### 8.1 The key insight: FX revenue is volume-driven, not carry-driven

Crypto perp protocols make most of their money from borrow fees on open interest, because crypto positions are held for days and the fee is ~0.01%/hour. **FX is the opposite.** Retail FX is high-turnover – traders cycle notional many times per day at high leverage. Exness reports monthly volumes in the trillions against a far smaller client asset base. XM's revenue is fundamentally *spread × volume*.

So the model is:

| Stream | Share of revenue (est.) | Mechanism |
|---|---|---|
| **1. Execution fee / spread** | **~85%** | 0.8–1.5 bps per side on notional, tiered by 30-day volume |
| 2. Carry markup | ~7% | Protocol markup over the true interest-rate differential |
| 3. Liquidation penalties | ~4% | 20% of the 0.5% penalty |
| 4. LP performance fee | ~3% | 10% of net LP profit above high-water mark |
| 5. LP exit fee | ~1% | 0.05% on withdrawal, anti-JIT |

Optimising for carry (the crypto playbook) would be the wrong instinct here.

### 8.2 Fee schedule

| 30-day volume | Fee per side |
|---|---|
| < $1M | 1.5 bps |
| $1M – $10M | 1.2 bps |
| $10M – $50M | 1.0 bps |
| $50M – $250M | 0.8 bps |
| > $250M | 0.6 bps |

Round-trip 1.2–3.0 bps, versus 0.6–0.9 bps at XM and 12 bps at Jupiter Perps. You sit between them – more expensive than a legacy broker on pure spread, dramatically cheaper than crypto perps, and the difference is what a trader pays for not handing over custody, plus 24/5 instant settlement, plus verifiable swap rates.

**Be honest in your own marketing about this.** "Cheaper than XM" is a claim you cannot defend on EUR/USD spread and it will be checked. "Non-custodial, transparent, and competitive" is defensible.

### 8.3 Fee routing

Every fee splits atomically at collection:

```
LP vault      55%   – must be the largest share or you get no liquidity
Treasury      25%   – your revenue
Insurance     10%
Referral pool 10%   – unclaimed portion sweeps to treasury
```

Do not start with a larger treasury share. Under-paying LPs is the single most common cause of death for pool-backed perp protocols – no liquidity means no depth, means no traders, means no fees. Raise the treasury share later from a position of strength.

### 8.4 Revenue projections

Assumptions: fee blended at 1.0 bps/side; volume counts both opens and closes; carry markup 0.3 bps/day on OI; 25% treasury share.

| Scenario | Daily volume | Avg OI | Gross daily fees | **Your treasury (25%)** | Annualised |
|---|---|---|---|---|---|
| **A. Devnet/beta** (mo. 6–9) | $2M | $200k | ~$210 | ~$53/day | ~$19k |
| **B. Early traction** (yr 1–2) | $50M | $5M | ~$5,300 | ~$1,325/day | **~$484k** |
| **C. Established** (yr 3) | $500M | $40M | ~$52,000 | ~$13,000/day | **~$4.7M** |
| **D. gTrade-scale** | $2B | $150M | ~$205,000 | ~$51,000/day | ~$18.7M |

**Read Scenario A honestly:** in the first year you will run at a loss. Infrastructure alone – dedicated RPC ($500–2,000/mo), keeper hosting, indexer database, monitoring – runs $1,500–3,000/month, before an audit ($40k–150k) and before liquidity incentives. Budget for 12–18 months of negative cash flow. That is normal, but it must be planned for rather than discovered.

The variable that decides which scenario you land in is **distribution, not code.** Which is why:

### 8.5 The growth engine: an on-chain IB programme

This is the highest-leverage product decision in the document, and it is absent from the v0 proposal. **It is core scope – Phase 6 – implemented as a separate program (`solfx-referral`) so it can iterate independently of the audited financial core.**

XM and Exness did not win on spreads or platform quality. They won on **introducing broker (IB) networks** – armies of affiliates paid a per-lot rebate to bring in traders. It is the entire retail FX growth model.

Every IB in that industry has the same complaint: **the broker controls the ledger.** Rebates get miscounted, clients get reassigned, payouts get delayed or clawed back, and the IB has no way to verify any of it.

An on-chain IB programme fixes that structurally:

- Referral link is written into the `UserAccount` at creation and is **immutable**.
- Rebate accrues per trade in the same instruction that charges the fee.
- IB claims from a PDA whenever they want. No approval, no payout schedule, no counterparty.
- Every rebate is a public, verifiable event.

Suggested tiers, paid from the 10% referral pool:

| IB tier | Referred 30-day volume | IB share of fees |
|---|---|---|
| Bronze | < $5M | 8% |
| Silver | $5M – $25M | 10% |
| Gold | $25M – $100M | 13% |
| Diamond | > $100M | 16% |

Plus a 2-level structure (sub-IBs earn 20% of their parent's rate) – because that is what the existing FX affiliate networks expect, and matching their mental model lowers switching cost.

**This is your actual moat.** The smart contract is replicable in a few months by anyone. A network of IBs who trust your ledger because they can audit it is not.

---

## 9. Off-Chain Infrastructure

Non-negotiable. A perp protocol without keepers is a protocol that does not liquidate.

### 9.1 Services

| Service | Language | Responsibility | SLA |
|---|---|---|---|
| **Liquidator** | Rust | Scan positions, compute HF off-chain, submit liquidations | < 2s from HF < 1 to tx landed |
| **Trigger executor** | Rust | Fire TP/SL when the oracle crosses | < 2s |
| **Funding cranker** | Rust | `crank_funding` per market hourly | Hourly ± 60s |
| **Session cranker** | Rust | `crank_market_session` | Every 60s |
| **Price relayer** | TypeScript | Hermes SSE – cache latest Pyth VAAs for the frontend | < 200ms staleness |
| **Indexer** | TypeScript | Geyser/Helius webhooks – Postgres – REST/WS API | < 1s lag |
| **Monitoring** | – | Prometheus + Grafana + PagerDuty | – |

### 9.2 Liquidator design

The critical path. Design notes:

- **State from Geyser, not polling.** A Yellowstone gRPC subscription on the program's accounts gives you position updates the moment they land. Polling `getProgramAccounts` on 100k positions will rate-limit you into failure exactly when it matters.
- **Prices from Hermes SSE**, held in memory. Recompute HF for affected positions on every price tick.
- **Maintain a sorted "danger list"** – positions within 20% of liquidation – so the hot loop is over hundreds of accounts, not all of them.
- **Pre-sign and pre-build** transaction templates; on trigger, only the oracle update and blockhash need filling in.
- **Aggressive priority fees.** Liquidations compete for blockspace during exactly the congestion spikes that cause them. Budget dynamically from `getRecentPrioritizationFees`, and be willing to pay $0.50 to capture a $20 reward.
- **Run at least three independent instances** in different regions from day one. Assume yours fails; the protocol must not depend on it. Publish the liquidator source and encourage third parties – a permissionless liquidation market is the resilient design.

**Weekend operation is not optional.** Continuous metals markets trade Saturday and Sunday, so liquidations happen at 3am on a weekend with nobody watching. Keepers, alerting, and on-call must cover seven days from the moment the first metals market goes live. **A weekend product with weekday-only operations is worse than having no weekend product** – it accumulates unliquidated positions precisely when liquidity is thinnest and the insurance fund is least able to absorb the result. If 7-day on-call is not realistically staffable, delay the weekend feature rather than shipping it uncovered.

### 9.3 RPC strategy

Public RPC will not work. Budget for Helius or Triton with dedicated nodes plus Geyser. This is a hard dependency, not a nice-to-have – and it is the main reason Scenario A runs at a loss.

---

## 10. Frontend Architecture

**Stack:** Next.js 15 (App Router), TypeScript strict, Tailwind, `@solana/wallet-adapter`, `@tanstack/react-query`, Zustand, TradingView Lightweight Charts, and a generated `@solfx/sdk` typed client from the Anchor IDL.

### 10.1 Structure

```
app/
├── (marketing)/            # landing, docs, IB programme
├── trade/[market]/         # main terminal
├── portfolio/              # positions, history, PnL
├── pool/                   # LP deposit/withdraw, vault stats
├── referral/               # IB dashboard, claim rebates
└── api/                    # BFF routes – indexer (never expose indexer directly)
```

### 10.2 Build a broker terminal, not a perp DEX terminal

This is the product differentiation (§ 1.2), so it is a requirement rather than polish. GMTrade already has a good perps UI; competing there is pointless. The audience is retail FX traders, and they have twenty years of muscle memory from MT4/MT5.

- **Size in lots, show P&L in pips and dollars.** Not percentages, not "base units." A trader who cannot see "+32 pips / +$320" cannot evaluate you against their existing broker.
- **Show the true all-in cost before submit** – spread + fee + estimated carry, in pips *and* dollars.
- **Show the liquidation price prominently**, recomputed live. This is the number the trader actually watches.
- **Swap-rate table with the markup broken out** – the true interest-rate differential and the SolFX markup as separate line items (§ 6.7). This is a concrete, verifiable improvement on XM/Exness opacity, and it is cheap to build.
- **Per-market regime clock.** Session-bound markets show next open/close and a banner in `ReduceOnly`. Continuous markets show a "trades all weekend" badge, and in `WeekendMode` state the derated parameters plainly – halved leverage, wider spread – rather than letting users discover them at order time. *(2026-08-01: no market currently qualifies as continuous – the badge and `WeekendMode` display are blocked on **Q1b**. Do not build them yet.)*
- **Show the confidence-driven spread live.** When it widens, the user must see it widen – otherwise every news event generates a "you scammed me on slippage" ticket.
- **A market browser that scales to 100+ instruments** (§ 3.5) – search, categories (Metals / Majors / Minors / Exotics / EM), favourites. Four hardcoded tabs will not survive Tier 4.
- **Optimistic UI with reconciliation.** 400ms finality feels instant if you render optimistically and reconcile against the confirmed transaction.
- **A real risk disclosure on first connect**, covering leverage, liquidation, ADL, weekend/session gaps, and smart-contract risk. Required both ethically and for any future licensing conversation.

---

## 11. Repository Structure

```
SOLFX/
├── Anchor.toml
├── Cargo.toml                       # Rust workspace
├── package.json                     # pnpm workspace
├── pnpm-workspace.yaml
├── rust-toolchain.toml              # pin the toolchain
│
├── scratch/
│   └── feed-probe/                  # Phase 0b – throwaway Pyth measurement script
│
├── programs/
│   ├── solfx-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs               # declare_id!, #[program] mod
│   │       ├── constants.rs
│   │       ├── errors.rs            # every failure has a named error
│   │       ├── events.rs            # every state change emits an event
│   │       ├── state/
│   │       │   ├── mod.rs
│   │       │   ├── protocol.rs
│   │       │   ├── market.rs
│   │       │   ├── position.rs
│   │       │   ├── user_account.rs
│   │       │   ├── lp_pool.rs
│   │       │   ├── insurance_fund.rs
│   │       │   └── trigger_order.rs
│   │       ├── math/
│   │       │   ├── mod.rs
│   │       │   ├── fixed.rs         # checked helpers, explicit rounding
│   │       │   ├── oracle.rs        # load_validated_price – THE choke point
│   │       │   ├── pricing.rs       # execution price, spread, skew
│   │       │   ├── margin.rs
│   │       │   ├── pnl.rs
│   │       │   ├── funding.rs
│   │       │   └── liquidation.rs
│   │       └── instructions/
│   │           ├── mod.rs
│   │           ├── admin/
│   │           ├── lp/
│   │           ├── trader/
│   │           └── keeper/
│   └── solfx-referral/
│
├── tests/                           # Anchor TS integration tests
│   ├── unit/
│   ├── integration/
│   ├── invariants/
│   └── scenarios/                   # historical replay: CHF 2015, GBP 2016, Mar 2020
│
├── fuzz/                            # Trident fuzz targets
│
├── keepers/
│   ├── liquidator/                  # Rust
│   ├── trigger-executor/            # Rust
│   ├── crankers/                    # Rust
│   └── shared/
│
├── services/
│   ├── indexer/                     # TS: Geyser – Postgres
│   ├── price-relayer/               # TS: Hermes SSE cache
│   └── api/                         # TS: REST/WS for the frontend
│
├── sdk/                             # @solfx/sdk – generated + hand-written helpers
├── app/                             # Next.js frontend
│
├── infra/
│   ├── docker/
│   ├── terraform/
│   ├── grafana/
│   └── runbooks/
│
└── docs/
    ├── ARCHITECTURE.md              # this file
    ├── FOREX-EXPLAINED.md           # plain-English companion
    ├── oracle-feasibility.md        # Phase 0b output – gates every risk parameter
    ├── adr/
    ├── compute-budget.md
    ├── risk-parameters.md
    ├── audit/
    └── runbooks/
```

---

## 12. Testing Strategy

For a protocol holding user funds, test code should exceed program code in volume. Target **>90% line coverage on `programs/`, 100% on `math/`.**

### 12.1 Layers

| Layer | Tool | Covers |
|---|---|---|
| Unit | `cargo test` + `proptest` | Every math function, including boundaries |
| Integration | Anchor + `solana-program-test` | Full instruction flows against a local validator |
| Invariant | Custom harness | Global properties after every instruction |
| Fuzz | Trident | Random instruction sequences hunting for panics and invariant breaks |
| Scenario | Historical replay | Real crisis price data |
| Load | Devnet | 1,000 concurrent positions, mass-liquidation event |

### 12.2 Property tests (mandatory)

```rust
proptest! {
    #[test]
    fn pnl_is_antisymmetric(size in 1u64..1e15 as u64,
                            entry in 1e8i64..1e10, exit in 1e8i64..1e10) {
        let l = upnl(&long(size, entry), exit).unwrap();
        let s = upnl(&short(size, entry), exit).unwrap();
        prop_assert!((l + s).abs() <= 1);   // equal and opposite, ±1 rounding unit
    }

    #[test]
    fn open_then_immediately_close_never_profits(..) {
        // Round-tripping at the same oracle price must ALWAYS lose
        // (spread + fees). If this ever passes, you have a free-money bug.
        prop_assert!(final_collateral < initial_collateral);
    }

    #[test]
    fn fees_always_round_toward_protocol(..) { .. }

    #[test]
    fn liquidation_never_increases_user_equity(..) { .. }
}
```

The second one is the single most valuable test in the suite. Run it across the full parameter space.

### 12.3 Global invariants – assert after every instruction in every integration test

```
I1  CollateralVault.amount == Σ(UserAccount.free) + Σ(Position.collateral)
I2  LpVault.amount == LpPool.aum
I3  Σ(cum_funding_long × oi_long) + Σ(cum_funding_short × oi_short) ≈ 0
I4  market.oi_long == Σ(long positions' notional)   (same for short)
I5  No position exists with collateral == 0 and size > 0
I6  InsuranceVault.amount == InsuranceFund.balance
I7  Σ(all USDC in all program vaults) == Σ(deposits) − Σ(withdrawals)
I8  LP token supply > 0 ⟺ LpPool.aum > 0
I9  No instruction path lets equity go negative without touching insurance
```

### 12.4 Scenario tests – real crisis data

Replay actual tick data through the engine on a local validator:

| Event | What it tests |
|---|---|
| **CHF depeg, 15 Jan 2015** (EUR/CHF → −30% in minutes) | Total liquidation-engine failure mode; bad debt waterfall; the event that bankrupted real brokers (Alpari UK – cited in your own proposal) |
| **GBP flash crash, 7 Oct 2016** (−6% in 2 min, then recovery) | Liquidation during a wick; oracle deviation breaker |
| **March 2020 COVID** | Sustained volatility; confidence-interval blowout; sustained wide spreads |
| **EM devaluation** (IDR 1998, INR 2013 taper tantrum) | EM risk parameters; whether 10–20x survives a managed-float break |
| **Any Sunday FX gap open** | Session-bound regime; `GapWindow` behaviour |
| **Weekend metals gap** (Sunday shock, index-to-spot convergence) | Continuous regime; `WeekendMode` derating; `PreOpenWindow` |

If the engine survives a synthetic CHF-depeg replay with the insurance fund intact, you have something. Until then you have a demo. **Make this a hard gate before mainnet.**

### 12.5 Extensibility test (Phase 9)

The claim in § 5.6 – that new markets need no redeploy – must be **demonstrated, not asserted**. On a running devnet deployment carrying live positions:

1. Call `initialize_market` for a market that did not exist at deploy time.
2. Confirm the program binary hash is **unchanged**.
3. Trade the new market end to end.
4. Confirm every pre-existing position is untouched – same collateral, same entry, same accrued funding.
5. Repeat for each `price_source` and `feed_kind` variant: a direct USD-quoted market, a synthetic cross, a non-USD-quoted EM market, and a continuous metals market.

Step 5 is the one that matters. It proves the engine is genuinely asset-class-generic rather than incidentally working for the cases built first.

---

## 13. Security & Threat Model

### 13.1 Threat table

| # | Threat | Vector | Mitigation |
|---|---|---|---|
| T1 | Oracle manipulation | Compromised/lagged feed | Confidence gate, staleness gate, EMA deviation breaker, single validated read path |
| T2 | **Latency arbitrage / toxic flow** | Faster feed than the protocol | Confidence-scaled spread, adverse-side pricing, min hold time, skew impact |
| T3 | Weekend stale-price exploit | Trading on a frozen Friday price | Session state machine, fail-closed (§ 7.3) |
| T4 | Account substitution | Passing a wrong PDA | Strict `seeds`/`bump`/`has_one` on every account; never trust an input account |
| T5 | Arithmetic overflow / precision drain | Rounding in the user's favour | Checked math, u128 intermediates, explicit rounding rules, property tests |
| T6 | LP JIT attack | Deposit before a known trader loss | Withdrawal cooldown (24h), entry/exit fee, NAV snapshotting |
| T7 | Liquidation griefing | Spam-liquidating near the boundary | Require strict `equity < mm`, no partial-liq below a threshold |
| T8 | Self-liquidation for profit | Liquidating your own position for the reward | Penalty always exceeds reward; net loss to the position owner |
| T9 | Malicious token program CPI | Fake token program in accounts | Hard-pin SPL Token program ID; reject Token-2022 transfer-hook/fee extensions |
| T10 | Admin key compromise | Stolen upgrade authority | Squads 3-of-5, timelock, guardian can only pause |
| T11 | Compute exhaustion | Liquidation tx exceeds CU limit | Budget cap per instruction, regression test on CU, keep liquidation < 200k |
| T12 | Bad debt cascade | Gap through liquidation prices | Insurance fund, ADL, OI caps, conservative leverage |
| T13 | Frontend compromise | DNS/CDN hijack, malicious bundle | IPFS mirror, SRI, wallet tx simulation, signed releases |
| T14 | Rounding-loop drain | Repeated micro-trades exploiting rounding | Minimum position size, protocol-favourable rounding, fuzzing |

**T2 and T3 are the two that kill you.** Everything else is recoverable.

### 13.2 Anchor hygiene checklist

- `#[account(mut, has_one = authority)]` on every user-owned account
- Explicit `seeds` + `bump` constraints – never a raw `AccountInfo` for anything you write
- `#[account(address = <known program>)]` for every program account passed in
- `require!` with a named error for every precondition – no `unwrap()`, no `panic!`, no array indexing without a bounds check
- `overflow-checks = true` in the release profile
- `#[event]` emitted for every state mutation (this *is* your indexer's data source)
- Reserved padding in every account struct for forward-compatible upgrades

### 13.3 Audit plan

| Stage | When | Est. cost |
|---|---|---|
| Internal review + full fuzz campaign | Pre-audit | – |
| Audit #1 (core financial engine) | Before devnet public | $40k–80k |
| Audit #2 (different firm, full scope) | Before mainnet | $60k–150k |
| Immunefi bug bounty | At mainnet | 10% of TVL cap, min $50k |
| Guarded mainnet launch | Weeks 1–8 | TVL cap $250k → $1M → $5M |

**Two audits from different firms is not overkill for a leveraged financial protocol.** Firms have blind spots and they differ. If budget forces one, spend it on the second (full scope, pre-mainnet) and substitute a public bounty on devnet for the first.

---

## 14. Deployment & Operations

### 14.1 Environments

| Env | Cluster | Purpose |
|---|---|---|
| Local | `solana-test-validator` | Dev + CI |
| Devnet | Devnet | Integration, public beta, stress tests |
| Staging | Mainnet-beta, capped | Real USDC, TVL cap, invite-only |
| Production | Mainnet-beta | Public |

### 14.2 Key management

| Key | Storage | Holder |
|---|---|---|
| Program upgrade authority | Squads V4 3-of-5 | Founding team + 2 external |
| Protocol admin | Squads V4 3-of-5 | Same |
| Guardian (pause-only) | Hardware wallet, hot | Lead engineer |
| Keeper operational | Encrypted env, low balance | Automated, rotated monthly |
| Treasury | Squads V4 2-of-3 | Founders |

Keeper keys hold operational SOL only and can never move protocol funds. Verify this by construction, not by policy.

### 14.3 CI/CD

```yaml
# .github/workflows/ci.yml – gates on every PR
- cargo fmt --check
- cargo clippy -- -D warnings
- cargo test --workspace                    # unit + property
- anchor build --verifiable                 # reproducible build
- anchor test                               # integration
- pnpm test                                 # SDK + frontend
- compute-budget regression check           # fail if any ix grows >10%
- invariant suite
- trident fuzz (nightly, 4h)
```

Verifiable builds are mandatory before mainnet – users must be able to confirm the deployed bytecode matches the public source. Without it the open-source claim in the proposal is unverifiable.

### 14.4 Monitoring

Alert on: liquidator lag > 5s, insurance fund below threshold, oracle staleness, LP NAV drop > 2%/hour, OI cap approach, keeper wallet balance low, failed transaction rate > 1%, RPC error rate.

Runbooks in `docs/runbooks/`: oracle failure, mass liquidation, bad debt event, keeper outage, RPC outage, exploit response (with a pre-decided pause decision tree), LP bank run.

---

## 15. Delivery Roadmap

The v0 four-phase plan covers roughly the first 40% of the work. Revised, with **exit criteria** – a phase is not done because time passed, it is done when the criteria pass.

**Track decision:** capstone first. Phases 0–9 deliver a complete, tested, publicly deployed system on devnet. The commercial track (audits, mainnet, legal) follows, gated on traction or grant funding – not started speculatively.

| Phase | Deliverable | Exit criteria | Est. |
|---|---|---|---|
| **0a. Explainer** | `docs/FOREX-EXPLAINED.md` – plain-English guide to FX, the protocol, and every decision | Reader with no FX background can evaluate the trade-offs | 3 d |
| **0b. Feed feasibility** | Pyth probe over 7 days incl. a weekend: enumerate feeds, measure publishers / update rate / confidence p50-p95-p99 / session boundaries | `docs/oracle-feasibility.md` published; listed-market set decided; **go/no-go on the EM and weekend theses** | 1 wk |
| **1. Foundations** | Toolchain, repo scaffold, CI, `math/` module with property tests | `cargo test` green; CI on PR; 100% coverage on `math/`; **round-trip-never-profits passes** | 2 wks |
| **2. Vault, markets, oracle** | Protocol/Market/UserAccount, deposits, withdrawals, Pyth pull oracle, **generic-engine constraints (§ 5.6)** | Direct, synthetic, EM and metals-index feeds all price correctly; I1 holds | 3 wks |
| **3. Position engine** | Open/increase/decrease/close, margin, PnL, quote conversion, execution pricing | Lifecycle tested on a direct pair, a synthetic cross **and** a non-USD-quoted pair; I1–I5 hold | 4 wks |
| **4. Risk engine + regimes** | Funding, carry, liquidation, insurance, ADL, **both regime state machines (§ 7.3)** | CHF-depeg replay survives; a real weekend, an EM holiday, and a full metals `WeekendMode → PreOpenWindow` cycle all transition correctly | 4–5 wks |
| **5. LP vault** | Add/remove liquidity, LP token, cooldown, fee distribution | LP accounting exact under adversarial sequences; I2, I8 hold | 2 wks |
| **6. IB programme** | `solfx-referral` – immutable binding, per-trade accrual, permissionless claim | Rebates accrue and claim correctly; every rebate emits a verifiable event; tier boundaries tested | 2 wks |
| **7. Keepers** | Liquidator, trigger executor, crankers, monitoring – **7-day operation** | Sub-2s liquidations under load; survives an instance being killed; verified firing during a live weekend | 3 wks |
| **8. Frontend + SDK** | Broker terminal (§ 10.2), TypeScript SDK, indexer, price relayer | End-to-end browser trade on devnet **including a weekend gold trade** | 5 wks |
| **9. Expansion + hardening** | List Tiers 2–3 (~15 markets, config only), fuzzing, scenario replays | Trident 24h clean; all scenarios pass; >90% coverage; ≥20 live markets; **§ 12.5 extensibility test passes**; 30 days zero fund-loss bugs | 4 wks |

**→ Capstone complete at Phase 9: ~31 weeks (~7.5 months full-time).** Part-time alongside study: 14–18 months.

**Commercial track (gated on traction or funding):** Tiers 4–6 market expansion (config only) · Audit #1 ($40–80k) · Audit #2, different firm ($60–150k) · Squads 3-of-5 + timelock · insurance fund seeding · legal opinion ($5–20k) + geofencing · guarded mainnet with staged TVL caps ($250k → $1M → $5M) · public launch.

**Apply for funding during Phase 4, not at the end.** Solana Foundation grants, Pyth grants, and Colosseum hackathon prizes all exist for this. Pyth in particular has direct interest – SolFX would be a flagship consumer of both their EM FX feeds and their new 24/7 indices. A working devnet demo at Phase 4 is a far stronger application than a finished protocol asking for audit money at Phase 9.

---

## 16. Risk Register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Toxic flow drains the LP vault | High | Critical | § 6.6 – this is the #1 technical risk |
| R2 | FX session gap causes bad debt | Medium | High | § 7.3 session-bound regime + insurance fund |
| R3 | **Weekend metals gap causes bad debt** | Medium | High | `WeekendMode` derating + `PreOpenWindow` + tighter OI caps (§ 7.3) |
| R4 | Cannot bootstrap LP liquidity | High | Critical | Seed the vault yourself; guarantee early LP yield from treasury |
| R5 | Cannot bootstrap traders | High | Critical | IB programme (§ 8.5); start recruiting IBs during devnet beta |
| R6 | **Pyth EM/index feeds prove unusable** | Medium | Critical | **Phase 0b measures this before any code is written.** Fallback: majors + broker UX + IB network |
| R7 | Audit costs exceed budget | Medium | High | Grants: Solana Foundation, Pyth, Colosseum. Apply during Phase 4, not Phase 9 |
| R8 | Smart contract exploit | Medium | Critical | Two audits, fuzzing, bounty, guarded launch, timelock |
| R9 | **Regulatory action** | Medium | Critical | See below |
| R10 | Pyth feed degradation/delisting | Low | High | Abstract the oracle interface; keep a secondary source (Switchboard/Chainlink) behind the same trait |
| R11 | Solana network outage during volatility | Low | High | Positions frozen; disclose it; MMR buffer sized for it |
| R12 | **GMTrade competes directly on EM/weekend** | Medium | High | They have volume and capital; we have focus and an IB network. Differentiation is audience and distribution, not features they cannot copy |
| R13 | Key person / solo founder risk | High | High | Documentation discipline; recruit a second engineer before Phase 7 |

### R9 – Regulatory, stated plainly

Retail leveraged foreign exchange is among the most heavily regulated retail financial products in the world. In the US it is effectively closed to unregistered operators (CFTC/NFA registration; offering leveraged retail FX without it is a federal enforcement matter). In the EU, ESMA caps retail FX leverage at 30:1. The UK FCA and Australia's ASIC have comparable regimes.

The moment you charge a fee, this stops being an academic project and becomes a financial services business. That it is decentralised is a defence regulators have repeatedly declined to accept – particularly where an identifiable operator collects revenue and runs the frontend.

The EM strategy raises this risk rather than lowering it, and that is the honest trade-off of the positioning. The emerging-market niche is empty partly *because* it is legally difficult:

- **India** – FEMA and RBI rules restrict INR derivatives for residents; offshore INR NDF trading by residents has been a specific enforcement focus.
- **Indonesia** – restrictions on offshore IDR derivatives.
- **Korea, Philippines, Taiwan** – each has its own capital-account and derivatives regime requiring separate analysis.

These are not reasons to abandon the strategy. They are reasons the analysis must be jurisdiction-by-jurisdiction and must happen **before** the fee switch, not after.

Concrete items for a digital-assets lawyer, **before mainnet – not before devnet**:

- Entity structure and jurisdiction
- Geofencing at the frontend (US, likely UK/EU, **and each EM jurisdiction whose currency is listed**), with an honest accompanying note that the on-chain program is permissionless
- Whether the frontend operator and the protocol are legally separable
- Terms of service, risk disclosures, and marketing-claims review
- Whether the leverage tiers offered are defensible in the chosen jurisdiction
- **Whether listing a currency pair constitutes offering a product into that currency's home jurisdiction**

**None of this blocks Phases 0–9.** A devnet deployment with test tokens and no fees is a research project with zero regulatory exposure. Get legal input during Phase 8–9, before real money and before the fee switch. Budget $5k–20k for an initial opinion, and expect the EM scope to push toward the upper end.

---

## 17. Decisions

### Settled

| # | Decision | Chosen | Rationale |
|---|---|---|---|
| D1 | Primary goal | **Capstone first, business optional** | Phases 0–9 to public devnet; commercial track gated on traction or grants |
| D2 | Positioning | **EM-FX + broker UX + auditable IB rebate ledger** | GMTrade owns majors; compete on audience and markets, not on being first (§ 1.2) |
| D3 | Launch leverage | **50x Tier 1, tiered down by class** | Every increase is easy PR; every decrease reads as distress (§ 6.3) |
| D4 | LP vault design | **Single global** | Capital efficiency matters far more at this size (ADR-003) |
| D5 | Token | **LP token only** | A governance token adds legal surface and distribution work, and zero engineering value pre-launch |
| D6 | Weekend trading | **Metals yes, FX no** | Feed-driven, per-market; flips via config if Pyth ships a 24/7 FX index (§ 7.3) |
| D7 | Market coverage | **Direct + synthetic crosses to Exness parity** | ~25 feeds – ~300 constructible pairs (§ 3.5) |
| D8 | Funding path | **Grants first** | Solana Foundation, Pyth, Colosseum. Apply Phase 4 with a working demo |
| D9 | Open-source timing | **At audit** | Public before audit invites forks carrying your unaudited bugs |

### Still open

| # | Decision | Options | Note |
|---|---|---|---|
| D10 | Frontend hosting | Vercel · self-host · IPFS + gateway | Vercel + IPFS mirror recommended; decide at Phase 8 |
| D11 | Solo or team | Solo · recruit | **Recruit before Phase 7.** R13 is the highest-likelihood risk in the register, and 7-day keeper ops makes it more pressing |
| D12 | Which EM pairs to list | Depends entirely on Phase 0b measurements | Do not pre-commit. The data decides |

---

## Appendix A – Immediate Next Steps

1. ✅ Write `docs/FOREX-EXPLAINED.md` (Phase 0a).
2. Install the missing toolchain: Solana CLI (Agave), Anchor via AVM, pnpm.
3. **Build and run the Pyth feed probe for 7 days including a weekend (Phase 0b).** Everything downstream – leverage tiers, spreads, session calendars, and whether the weekend feature exists at all – derives from its output. Do not start protocol code before `docs/oracle-feasibility.md` exists.
4. Scaffold the workspace per § 11.
5. Implement `math/` with property tests **first** – before any instruction. It is the layer everything depends on and where bugs are most expensive.
6. Wire up Pyth pull-oracle reads and confirm a direct feed, a synthetic cross, and a 24/7 index all deserialise correctly, including exponent normalisation.
7. Record baseline compute-unit costs.

## Appendix B – Terminology Correction Sheet

For use in the academic submission and all external material. Each left-hand claim is checkable in one search; asserting them costs credibility on everything else in the document.

| Do not say | Say instead |
|---|---|
| "the Jupiter blockchain" | "Jupiter, a protocol on Solana" |
| "first brokerage on Solana" | "the first FX-native **brokerage** on Solana – GMTrade trades FX pairs, but as 4 of 86 markets in a crypto perps terminal" |
| "first forex protocol on Solana" | **Do not use.** GMTrade launched FX perpetuals in January 2026 |
| "no counterparty conflict of interest" | "the counterparty is a public, permissionless vault priced by an oracle we do not control, with rules that cannot be changed against you mid-trade" |
| "24/7 trading" | "gold and silver trade 7 days a week; currency pairs follow global FX market hours" |
| "0.02% fee is cheaper than brokers" | "competitive all-in cost with full fee transparency and no custody risk" – do **not** claim to undercut XM on EUR/USD spread |
| "the first to offer USD/INR" | Accurate – but verify against Phase 0b before publishing, and pair it with the jurisdictional caveat in R9 |

**On the B-book point specifically:** the vault *is* the counterparty and *does* profit when traders lose. Say so. The defensible claim is transparency and non-custody, not the absence of a conflict – and a reviewer who catches the overclaim will discount the rest of the document.
