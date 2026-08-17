# What You Need to Know About SolFX

Everything here is something you should be able to explain from memory — to an examiner, an
investor, an auditor, or a trader who asks a hard question. Each item has **what it is**,
**why it matters**, and **a worked example with real numbers**.

Read [`FOREX-EXPLAINED.md`](FOREX-EXPLAINED.md) first if any of the FX vocabulary is new.
This document assumes it and goes to the things that are specific to *your* protocol.

---

## 1. What you have actually built, in one sentence

> A non-custodial, oracle-priced, vault-backed perpetual swap protocol on Solana that gives
> leveraged exposure to foreign exchange rates, settled entirely in USDC.

Unpack each word, because each one is a decision:

| Word | What it means | Why it was chosen |
|---|---|---|
| **non-custodial** | Collateral sits in a PDA the *user* controls; no key anywhere can move it | The whole product claim. Enforced by the *absence* of a code path, not by policy |
| **oracle-priced** | Every price comes from Pyth; the protocol cannot invent one | Removes the "broker controls your price" conflict — the #1 retail FX complaint |
| **vault-backed** | A public LP pool is the counterparty to every trade | A CLOB needs market makers before it needs traders, and you have neither (ADR-001) |
| **perpetual swap** | A contract tracking a price with no expiry | Behaves like a retail FX position; no rollover to explain |
| **settled in USDC** | One collateral asset | Multi-collateral means haircuts, collateral oracles, and liquidating collateral itself |

**Example.** A trader deposits 1,000 USDC. It moves from their wallet into
`CollateralVault`, and their `UserAccount.free_collateral` reads 1,000,000,000 (USDC has
6 decimals). No admin instruction exists that can move it back out to anyone but them.

---

## 2. You are a B-book. Say so.

**What it is.** When a trader wins, the LP vault pays. The vault is the counterparty to every
trade and is structurally short the aggregate trader PnL. That is the definition of a B-book
— the same model XM and Exness run.

**Why it matters.** The v0 proposal claimed SolFX "removes the counterparty conflict of
interest." That claim does not survive one knowledgeable question, and asserting it discounts
everything else you say.

**What to say instead** — this version is true *and* stronger:

> The counterparty is a public vault anyone can join or exit, priced off an oracle no
> operator controls, with rules in open-source programs that cannot be changed against you
> mid-trade, and collateral that never leaves your own PDA until the maths says it must.

| XM's B-book | Yours |
|---|---|
| One company, secret balance sheet | Public pool, anyone can join or exit |
| They control your price | Price comes from Pyth; you cannot touch it |
| Rules change silently | Rules are code; changes are on-chain and visible |
| They can freeze withdrawals | Withdrawals work even while the protocol is paused |
| You cannot audit anything | You can audit everything |

---

## 3. The four numbers every FX trader thinks in

You must be able to convert between these instantly, because your UI has to speak them.

| Term | Definition | Example |
|---|---|---|
| **Pip** | Smallest normal move — 4th decimal (2nd for JPY pairs) | 1.0850 → 1.0851 is 1 pip |
| **Lot** | Standard size = 100,000 units of base currency | 1 lot of EUR/USD = €100,000 |
| **Leverage** | Notional ÷ your margin | $108,500 position on $2,170 = 50x |
| **Margin** | The money backing the position | Initial (to open) and maintenance (to survive) |

**The identity you must know cold:** on a USD-quoted pair, **1 standard lot × 1 pip = $10.**

```
100,000 units × 0.0001 = $10
```

So 1 lot with a 50-pip gain = $500. This falls straight out of the fixed-point scales in
`constants.rs` rather than being special-cased, and
`one_lot_of_a_usd_quoted_pair_is_ten_dollars_per_pip` proves it.

---

## 4. Correction C-3 — the single most consequential piece of arithmetic

**What it is.** `PnL = size × (exit − entry)` produces a number in the **quote currency**, not
in dollars. For EUR/USD that happens to be USD, so nothing more is needed. For anything else
it must be converted.

**Why it matters.** Nearly every market you plan to list needs it: 106 of Pyth's 110 direct FX
feeds are `USD/XXX`, and every `XXX/JPY` cross is quoted in Yen. **The EM pairs that are your
whole differentiation are all like this.**

**Worked example — straight from a real MT5 terminal:**

```
GBP/CHF, buy 60.00 lots, 1.09659 → 1.09850

size      = 60 × 100,000              = 6,000,000 GBP
move      = 1.09850 − 1.09659         = 0.00191   (19.1 pips)
P&L quote = 6,000,000 × 0.00191       = 11,460.00 CHF   ← Swiss Francs
P&L USD   = 11,460 ÷ 0.810370         = 14,141.69 USD   ← what the trader sees
```

Skip the conversion and the position reads **$11,460** — a 23% error that looks entirely
plausible. On USD/INR the same mistake is **8,700%** (₹10,000 booked as $10,000 instead of
$113).

`QuoteConversion` makes it impossible to get the *direction* wrong:

| Variant | Feed convention | Operation | Example |
|---|---|---|---|
| `None` | quote is USD | identity | EUR/USD |
| `QuotePerUsd` | `USD/XXX` — quote units per dollar | **divide** | USD/JPY 157, USD/INR 88 |
| `UsdPerQuote` | `XXX/USD` — dollars per quote unit | **multiply** | EUR-quoted markets |

Nine tests in `broker_parity.rs` assert our figures match a real terminal **to the cent**.

---

## 5. The oracle is the security model

**What it is.** One function — `solfx_core::oracle` — is the only path from a Pyth account to
a price the engine will trade on. Six gates, in order:

| # | Gate | Stops |
|---|---|---|
| 1 | Ownership | forged bytes in an attacker-owned account |
| 2 | Feed identity | a real SOL/USD update being used to price EUR/USD |
| 3 | Verification = `Full` | a price backed by too few Wormhole guardians |
| 4 | **Freshness, both ends** | the stale-price exploit (C-1) *and* future-dated prices |
| 5 | Confidence | trading against a band you cannot price (C-4) |
| 6 | Deviation | a dislocated tick being traded through |

**Why it matters.** Two threats kill a pool-backed venue and everything else is recoverable:

- **T2 latency arbitrage** — someone with a faster feed buys inside your spread, repeatedly,
  with bots. This is what killed several GMX forks.
- **T3 stale price** — trading against a frozen price with zero risk.

**Worked example of T3, the one you must be able to explain:**

> It's Saturday. FX is closed, so Pyth's EUR/USD stopped updating at Friday 21:00 UTC. A
> trader reads news that will gap EUR/USD 150 pips at Sunday's open. They post Friday's
> genuine, fully-verified Pyth price and open maximum size. **Zero risk.** On Monday the LP
> vault pays the entire gap.

Three independent defences stop it, and each is a test:
1. `ReduceOnly` at T−15min refuses the pre-close open
2. `Halted` (from the session cranker) refuses the weekend open
3. The **staleness gate** refuses it even on a market nobody cranked

---

## 6. Where the money goes — the five flows

Every trade splits its fee four ways, atomically:

```
fee ──► LP vault      55%   ← must be largest or liquidity leaves
    ──► treasury      25%   ← your revenue (+ the rounding dust)
    ──► insurance     10%   ← the bad-debt reserve grows on every trade
    ──► referral pool 10%   ← accrues to the IB, or sweeps to treasury if none
```

**Worked example.** 0.1 lot EUR/USD at 1.08543, 1 bp fee:

```
notional = $10,857.58
fee      = $1.09
  → LP        $0.599
  → treasury  $0.273  (+ the leftover unit)
  → insurance $0.109
  → referral  $0.109
```

**Who can earn without trading — three roles, all live:**

| Role | Earns | Capital needed |
|---|---|---|
| **LP** | 55% of fees + trader losses | Yes — you fund the vault |
| **Liquidator** | 40% of each liquidation penalty, floored at $1 | Just gas |
| **Introducing broker** | 8–16% of fees from traders you refer | **None** |

The IB one is unusual and is your growth engine (§8.5).

---

## 7. Liquidation — and why it must be *profitable*

**What it is.** When equity falls below the maintenance margin, anyone may close the position
and take a share of the penalty.

**Why it matters.** §6.8's exact words: *an unprofitable liquidation is an unliquidated
position, and unliquidated positions are how vaults die.*

**Worked example.** Long 1 lot EUR/USD at 1.0850, 50x:

```
notional            $108,500
your margin         $2,170     (108,500 ÷ 50)
maintenance (1%)    $1,085
liquidation price   1.07415    ← a 1% move. EUR/USD does that in ~2 normal days.
```

At liquidation on a $10,000 position with a 0.5% penalty: liquidator $20, insurance $20,
treasury $10 — against a transaction costing fractions of a cent.

**The subtlety the CHF replay found:** in a severe gap the equity is *entirely wiped*, so 40%
of the penalty is 40% of nothing. Nobody shows up on the day it matters most. Hence
`MIN_LIQUIDATOR_REWARD = $1.00`, backstopped by the insurance fund.

**The waterfall when a position closes owing more than it holds:**

```
1. insurance fund pays        (LPs made whole)
2. ADL — force-close the most profitable OPPOSING position, withholding
   PROFIT ONLY, never principal
3. LP vault NAV absorbs the remainder
```

Traders must be told ADL exists **before** they open. Every venue that hid it and then used it
was destroyed for it.

---

## 8. The eight invariants

These are checked after **every single instruction** in the test harness. If you learn one
list from this document, learn this one — it is what "the accounting is correct" actually
means.

| | Statement | What breaking it would mean |
|---|---|---|
| **I1** | `CollateralVault == Σ free + Σ position margin + funding_balance` | Someone's collateral has gone missing |
| **I2** | `LpVault.amount == LpPool.aum` | LP accounting has drifted from reality |
| **I3** | Funding nets to zero, never touches LP capital | Traders are paying LPs, or vice versa |
| **I4** | Market OI == Σ over live positions | Risk limits are policing a fiction |
| **I5** | No position with size > 0 and collateral == 0 | An unbacked position exists |
| **I6** | `InsuranceVault == InsuranceFund.balance` | The reserve is not what it claims |
| **I7** | Every USDC in every vault is accounted for | Money was created or destroyed |
| **I8** | LP `supply > 0 ⟺ aum > 0` | Shares with nothing behind them, or unclaimable USDC |

**I1 is checked three ways** — the vault's token balance, the sum over accounts, and the
protocol's own counter. Two figures can be wrong in the same direction; three cannot, cheaply.

**Example of I8 earning its keep.** On its very first run it caught the last LP redeeming and
leaving the exit fee behind in a pool with zero shares — USDC nobody could ever claim.

---

## 9. Three bug patterns that recur — learn to spot them

Every defect found in this build fits one of these. They will recur in Phases 7–9.

### (a) The boundary deadlock
*A control that is correct in steady state and impossible at the boundary.*

- **Deviation gate (Phase 2):** a self-maintained EMA can only refresh through the gate it
  feeds → after a large genuine move nothing passes and the market wedges shut.
- **Skew cap (Phase 5):** §7.4 blocks the heavy side past 0.6, but the *first* position in a
  market is 100% one-sided by definition → no book could ever form.

**Spotting it:** ask "what does this rule do when the quantity it measures is zero or new?"

### (b) The unclamped claim
*Taking from a shared pot more than the individual owed.*

- **Liquidation (Phase 4):** the pool's claim was computed as `collateral − equity`; when
  equity went negative that exceeded the collateral, and the difference came out of **other
  traders' money** in the same vault. Invariant I1 caught it.

**Spotting it:** ask "is this transfer bounded by what *this* account actually holds?"

### (c) The unfunded incentive
*A permissionless action nobody is paid to perform.*

- **Liquidator reward (Phase 4):** zero equity means zero penalty means no liquidator.
- **Still open:** `crank_market_price`, `crank_funding`, `crank_market_session` are
  permissionless but pay **nothing**. Today only you have a reason to run them — which
  quietly makes the protocol depend on you being online.

**Spotting it:** ask "who runs this, and what do they get?"

---

## 10. Why every phase has *testable* exit criteria

**What it is.** A phase is not done because time passed. It is done when its criteria pass.

**Why it matters.** It is the difference between "I built a forex protocol" and "I built one
that survives a 30% currency depeg with the insurance fund intact and every unit of USDC
accounted for." Only one of those is a claim.

**Example.** Phase 4's criterion was the CHF-depeg replay. Writing it found **four real
defects** — none of which any unit test would have caught, because each only appears when a
30% gap, a blown-out confidence band, and an emptied position coincide.

---

## 11. What is *not* true — the claims you must never make

You will be checked on these in one search.

| Do not say | Say instead |
|---|---|
| "First forex protocol on Solana" | GMTrade launched FX perps in January 2026. You are the first FX-native **brokerage** |
| "The Jupiter blockchain" | Jupiter is a protocol *on* Solana |
| "No counterparty conflict of interest" | The counterparty is a public vault priced by an oracle we don't control |
| "Gold trades all weekend" | **Withdrawn 2026-08-01.** All 137 Pyth metals feeds freeze at the Friday close |
| "Cheaper than XM" | You are ~2x XM on spread. Compete on custody, transparency, and markets they don't list |
| "24/7 trading" | Solana is 24/7. The instruments follow global market hours |

**The weekend-gold story is worth being able to tell**, because it is the best evidence of
your engineering judgement: a vendor's launch announcement had reached a full protocol
specification — a state machine, four risk fields, a 7-day on-call requirement — before
anyone asked the feed whether it publishes on a Saturday. Phase 0b asked. It didn't. The
feature was cut before a line of protocol code was written.

> **A vendor's marketing claim is not a measurement.**

---

## 12. The regulatory position — state it plainly

Retail leveraged FX is among the most heavily regulated retail products in the world. The US
is effectively closed to unregistered operators; the EU caps retail leverage at 30:1.

**And your EM strategy *raises* this risk rather than lowering it.** The niche is empty partly
*because* it is legally difficult — India's FEMA rules, Indonesia's offshore-IDR
restrictions, Korea/Philippines/Taiwan each with their own regime.

**What protects you now:** a devnet deployment with test tokens and **no fees** is a research
project with zero regulatory exposure. Legal input is needed before mainnet and before the fee
switch — not before Phase 9.

---

## 13. The numbers to have on hand

| | |
|---|---|
| Tests | **463** (203 math, 260 program) |
| Programs | `solfx-core` ~905 KB · `solfx-referral` ~257 KB |
| Instructions | 26 |
| Listable markets (measured) | **29** |
| `open_position` | 59,705 CU against a 120,000 budget |
| `liquidate_position` | 59,223 CU against §6.8's hard 200,000 ceiling |
| Referral ledger's cost per trade | **+180 CU (+0.3%)** |
| Phases complete | 0–6 of 10 |
| Time to capstone | ~12 weeks remaining |

---

## 14. The five questions you will be asked

**"How is this different from GMTrade?"**
They're a crypto perps DEX with FX as 4 of 86 markets. You're a *broker*: lots and pips, EM
pairs nobody on any chain lists, transparent swap rates, an auditable partner ledger.

**"Isn't this just a casino?"**
It's the same instrument XM and Exness sell to millions. The difference is that the price is
Pyth's, the rules are code, the collateral is yours, and the counterparty is a pool you can
join.

**"What stops you stealing the money?"**
There is no instruction that lets any key move collateral, LP, or insurance funds. The admin
can reach the fee vault and nothing else. That is enforced by the absence of a code path.

**"What happens in a crash?"**
Replayed: CHF depeg 2015 (−30% in minutes), GBP flash crash 2016, weekend gaps. Insurance
absorbs the shortfall, ADL socialises what's left, LPs are last. Every unit stays accounted
for.

**"What's the biggest risk?"**
Not the code — **distribution**. GMTrade already has the volume. That's why the IB programme
is a core phase and not a nice-to-have, and why R13 (solo-founder risk) is the highest-
likelihood item in the register.

---

## Where to go deeper

| For | Read |
|---|---|
| No FX background | [`FOREX-EXPLAINED.md`](FOREX-EXPLAINED.md) |
| The full spec | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| What each phase built | [`guides/`](guides/README.md) — every function, formula and rule |
| Visual overview | [`diagrams/`](diagrams/) — a flowchart per phase |
| What is tested | [`test-cases/`](test-cases/README.md) — all 452 catalogued |
| Oracle measurements | [`oracle-feasibility.md`](oracle-feasibility.md) |
| Performance | [`compute-budget.md`](compute-budget.md) |
