# NOXFUNDS — How It Works

**A prop firm where the rules are code, and a rule-breaking trade cannot happen.**

**Who this is for:** anyone. No trading knowledge assumed, no Solana knowledge assumed.
**How to read it:** Parts 1–4 are the idea in plain English. Parts 5–7 are the specification.
Part 8 is what is live right now and how to check it yourself. Part 9 is what is *not* built.

Every number in this document comes from the source file that defines it, or from an account
you can read on Solana devnet yourself. Where something is unproven, it says so.

---

## Table of contents

1. [What a prop firm is, and what is wrong with them](#1-what-a-prop-firm-is-and-what-is-wrong-with-them)
2. [The idea in one sentence](#2-the-idea-in-one-sentence)
3. [How it works, start to finish](#3-how-it-works-start-to-finish)
4. [Why the trader cannot run off with the money](#4-why-the-trader-cannot-run-off-with-the-money)
5. [The rulebook](#5-the-rulebook)
6. [The money](#6-the-money)
7. [The track record, and why you can trust it](#7-the-track-record-and-why-you-can-trust-it)
8. [Specification](#8-specification)
9. [What is live, and how to verify it yourself](#9-what-is-live-and-how-to-verify-it-yourself)
10. [What is not built](#10-what-is-not-built)

---

## 1. What a prop firm is, and what is wrong with them

A **proprietary trading firm** — "prop firm" — gives money to a trader who has skill but no
capital. The trader trades the firm's money under a set of rules, and they split the profit.
A typical deal: you pass an evaluation, the firm funds you with $100,000, you keep 70–80% of
what you make, and you must not lose more than 5% in a day or 10% overall.

It is a real industry with real demand. It is also built entirely on trust in one direction.

**You have to trust the firm, and the firm has to trust nothing.**

| What happens | Why it is possible |
|---|---|
| You pass the evaluation, then the firm refuses the payout | Your account exists only in their database |
| The firm changes the rules after you have taken a position | The rules are a PDF, not a mechanism |
| Your "funded account" was a demo account the whole time | You cannot see whether real capital ever existed |
| The firm goes under holding your profit | Their balance sheet is private |
| A trader's advertised track record is selected from many accounts | The losing accounts are simply not shown |

Every one of these is a **record-keeping** problem. The firm keeps the record, so the firm
decides what the record says.

And there is a deeper problem that even an *honest* firm cannot solve. A prop firm's rules are
enforced **after the fact**. Their risk desk sees your trade once the broker has already filled
it. If you break a rule, they close you down and keep the loss. The rule did not stop the
trade — it punished you for it, and the capital was really lost.

---

## 2. The idea in one sentence

> NOXFUNDS owns the venue. A trade that breaks the rules is not detected and punished —
> **it fails as a transaction. It never existed, and the investor never took the loss.**

That is the whole product. Everything below is how that sentence is made true.

It works because of an unusual arrangement: NOXFUNDS sits **on top of** SolFX, a forex and
crypto exchange in the same repository. NOXFUNDS is not a firm watching an exchange from
outside. It is a program that holds the account *at* the exchange and forwards the trader's
orders — checking every rule first, in the same transaction, before anything is filled.

On Solana, a transaction is all-or-nothing. If any part of it fails, the entire thing is
discarded as though it never ran. So "check the rule, then place the trade, in one
transaction" means a trade that fails the rule check **leaves no trace and costs nothing**.

```
  A normal prop firm                  NOXFUNDS
  ─────────────────────               ─────────────────────
  trader sends order                  trader sends order
       ↓                                   ↓
  broker fills it          ✅        rules checked          ❌ → whole transaction
       ↓                                   ↓                      reverts, no fill,
  risk desk sees it later             venue fills it              no loss
       ↓
  "you broke rule 4"       ❌
  capital is really gone
```

---

## 3. How it works, start to finish

There are three parties.

| | Who | What they bring | What they get |
|---|---|---|---|
| **Investor** | anyone with USDC | the capital | their money back, plus 30% of the net profit |
| **Trader** | anyone with a wallet | the skill | 70% of the net profit. Never touches the capital |
| **Protocol** | NOXFUNDS itself | the rules and the venue | 5% of gross profit |

The unit of the whole system is a **mandate**: one investor funding one trader with one pot of
money under one rule set.

### Before any of this — a trader earns a record

A trader can prove themselves before anyone risks money on them. They put down a **$50 stake**
and trade a **simulated account** of $10,000 to $200,000 through two phases:

| Rule | Phase 1 | Phase 2 |
|---|---|---|
| Profit target | 8% | 5% |
| Loss in one UTC day | at most 3% | at most 3% |
| Loss from the peak | at most 6% | at most 6% |
| Risk at the stop, per trade | at most 1% | at most 1% |
| Stop-loss | mandatory | mandatory |
| Trades, over distinct days | 10, over 5 | 10, over 5 |
| Hold per voluntary close / on average | 10 min / 45 min | 10 min / 45 min |
| Any single day | at most half the target | at most half the target |
| Time limit | none | none |

Pass both and the **stake comes back in full** — the protocol earns from traders succeeding,
not failing. Break a loss limit, or walk away, and it goes to the treasury.

The simulated trades are not an approximation. Each one is priced by **the exchange's own code**
— the same functions a real trade on SolFX runs — so on the same price and market state, a
simulated fill and a real one agree to the last unit. The tests open both and compare. No real
money moves in the simulation; only the stake is ever held.

Two things differ from a live account, stated here rather than left for someone to find: a
simulated trade does not move the market for anyone else, and the whole simulated balance counts
as margin, so there is no simulated liquidation — the 6% rule ends an evaluation long before one
could happen.

### Step 0 — the two sides find each other, and agree

There is no chat, on purpose. Anything written on a public chain is public forever and
unencrypted, every message would be an account someone pays rent for, and an open message box
is a spam surface. So every "message" is a **typed object tied to a real step**, with a short
note (180 bytes at most) riding on it:

| Direction | What is posted | The answer |
|---|---|---|
| Investor → trader | an **offer**: the capital, already in escrow, plus the full rule set | the trader **accepts**, or **declines with a reason** |
| Trader → investor | a **funding request** — only to an investor with an open listing | the investor **posts an offer**, or dismisses it |
| Either side, publicly | a **listing**: what you are looking for | the other side browses and starts |

That is a complete negotiation: offer → decline with a reason → revised offer → accept. The
investor's terms *are* the message, the escrowed capital is the proof it is serious, and the
trader's acceptance is the signature on the contract. When they accept, the mandate, its vault
and the transfer of the capital happen **in one transaction** — there is no moment where the
terms are agreed but the money has not moved.

A few guarantees that fall out of the design:

- **The investor can always get the capital back** until a trader signs — expired, declined
  or not. Declining never moves money.
- **A request can only reach an investor who has listed.** No wallet can be messaged merely for
  existing, and closing the listing shuts the door.
- **One open request per trader–investor pair**, so an inbox cannot be flooded with copies.
- **The rules the trader accepts are byte for byte the rules the investor published** — the
  program copies them from the offer; nobody retypes them.

In the browser this is `/nox/market` for discovery, `/nox/investor` and `/nox/trader` for each
side's own dashboard. A trader does everything from theirs: take an evaluation, trade a funded
mandate, set a target, close. Nothing on either side needs the CLI or a checkout of this repo —
`nox` exists so the same flows can be run and recorded reproducibly, not because the page
cannot do them.

### Step 1 — the investor funds a mandate

The investor picks a trader, an amount, and a rule set, and signs one transaction. The USDC
moves out of their wallet into a **vault that belongs to the mandate** — not to the trader, not
to NOXFUNDS' operators, not to the investor any more.

The rules are written into the mandate at this moment and **can never be changed** — not by the
trader, not by the investor, not by the NOXFUNDS admin. There is no instruction in the program
that edits them. If the investor wants different rules, they fund a different mandate.

> This is the single biggest difference from a traditional firm. There, the rules are a document
> one party controls. Here they are fields on an account, fixed at creation, with no code path
> that mutates them.

### Step 2 — the trader trades

The trader opens positions by calling NOXFUNDS, not the exchange. NOXFUNDS checks every rule
(Part 5), and only if all of them pass does it forward the order to SolFX.

Two things happen in that same single transaction:

1. the position is opened, **and**
2. a stop-loss is placed.

They cannot be separated. There is no way to open a funded position without a stop-loss,
because both are built into the one instruction. This is what makes the *risk-per-trade* rule
enforceable **before** the fill: the program knows exactly how much can be lost, because it
knows where the stop is.

No centralised prop firm can do this. They can tell you to use a stop. They cannot make the
order fail without one.

### Step 3 — the mandate is watched

Anyone can call a **crank** — a small public instruction that marks the mandate's current value
to the live oracle price and records it. If equity has fallen below the drawdown limit, the
crank flips the mandate to `Breached`.

"Anyone" is deliberate. The trader cannot hide a losing position by refusing to close it,
because a stranger can mark it. The high-water mark (`peak_equity`) only ever rises, so a
trader cannot reset their drawdown by closing out and starting again.

### Step 4 — settlement

When the mandate ends — the investor asks for it back, or it breached — the positions are
closed, the money is counted, and it is divided (Part 6). The investor's share and the trader's
share and the protocol's fee are computed by the program, from the vault's actual balance.
Nobody approves the payout. There is nobody to appeal to, and nobody to be refused by.

### The same four steps, as it actually ran on devnet

This is a real mandate, `Bf7aVemJQwTq5trPgqo6t7vjmHy4M3FcxjjHSp1BCyrc`, and you can read it:

```
1  investor funds 200.000000 USDC into the mandate vault
2  trader opens BTC/USD long, $100 notional, $50 margin,
   stop 100 bps below entry — position and stop in ONE transaction
3  crank marks equity at 199.797874 against a peak of 200.000000
4  trader closes; investor requests settlement; investor receives 199.797874
   trader receives 0 (there was no profit)   protocol receives 0 (there was no profit)
```

The trade lost money, so the trader earned nothing and the protocol charged nothing. That is
the design: **the protocol never earns from a losing mandate, and the trader is never charged
for one.** The investor bears the trading loss, which is what putting up the capital means.

---

## 4. Why the trader cannot run off with the money

This is the question every investor asks, and it has a precise answer.

The mandate's money lives in a token account controlled by an address called a **Program
Derived Address**, or PDA. A PDA is an address on Solana that is deliberately constructed so
that **no private key for it exists or can exist**. Nobody holds the key, because there is no
key.

So how does anything ever move? From the Solana SDK's own documentation:

> There is no cryptographic signing involved — PDA signing is a runtime construct that allows
> the calling program to control accounts as if it could cryptographically sign for them.
> During invocation, the runtime will re-derive the PDA from the seeds and the **calling
> program's ID**, and if it matches one of the accounts, will consider that account "signed".

Read that middle clause again, because it is the entire security argument: the address is
re-derived from **the calling program's ID**. NOXFUNDS' mandate signer is derived from the
NOXFUNDS program. Therefore:

- **Only the NOXFUNDS program can authorise that account.** Not the trader. Not the investor.
  Not the NOXFUNDS admin. Not the person who deployed it. Not anyone with any private key
  anywhere, because the relevant key does not exist.
- The trader signs a transaction to NOXFUNDS. NOXFUNDS then decides whether to sign to SolFX.
  The trader's signature never reaches the exchange.
- The only instructions that release money are settlement instructions, and they pay to
  addresses recorded on the mandate at funding.

There is no "withdraw" instruction a trader can call. Not one that is guarded — one that does
not exist.

**What the trader can do:** open positions, close positions, cancel their own stop-loss.
**What the trader cannot do:** withdraw, change the rules, change who gets paid, or trade a
market the mandate does not permit.

### Where trust is still required — stated plainly

It would be dishonest to stop there. Three things still require trust:

1. **The program can be upgraded.** Whoever holds the upgrade authority can replace the code
   with different code. This is true of almost every Solana program. The mitigation is that
   upgrades are public and on-chain, and the standard endgame is to hand the authority to a
   multisig or revoke it entirely. Neither has been done here.
2. **The admin can pause.** A paused protocol blocks new activity. The admin **cannot** move
   funds — that is a deliberate split, the same one SolFX uses for its guardian.
3. **It has never been audited.** See Part 10. Not once, by anyone.

---

## 5. The rulebook

Every rule below is a field on the mandate, fixed at funding, checked before the exchange is
ever called. Each one has its own named error, so a rejected trade tells you exactly which rule
stopped it — these are the real error strings from
[`errors.rs`](../programs/noxfunds/src/errors.rs).

| Rule | What it stops | Error when it fires |
|---|---|---|
| **Max trade notional** | one oversized bet | `Trade notional exceeds the mandate's per-trade ceiling` |
| **Max total notional** | many bets adding up to one oversized bet | `Trade would exceed the mandate's total open notional` |
| **Max concurrent positions** | attention spread too thin | `Mandate already holds its maximum number of open positions` |
| **Stop-loss required** | trading without a floor | `Every funded trade must carry a stop-loss` |
| **Max risk per trade** | a stop so far away it is not a stop | `Risk at the stop exceeds the mandate's per-trade risk limit` |
| **Max stop distance** | the same thing, measured in price | `Stop-loss is further from entry than the mandate allows` |
| **Stop on the correct side** | a "stop" that is actually a target | `Stop-loss is on the wrong side of the entry price` |
| **Allowed markets** | trading instruments outside the remit | `This market is not in the mandate's permitted set` |
| **Max drawdown** | trading on after the capital is impaired | `Mandate has breached its maximum drawdown` |
| **Minimum hold time** | scalping the statistics | `Position has not been held for the mandate's minimum` |
| **Trader identity** | anyone else trading the mandate | `Signer is not this mandate's trader` |

Two of these deserve a note.

**Risk per trade is the rule nobody else can enforce.** It is checked *before the fill*, and it
is only checkable because the stop-loss is mandatory and placed in the same transaction. A
centralised firm can only measure your risk once you already have the position.

**The minimum hold time exempts stop-outs.** It applies to *voluntary* closes only. Forcing a
trader to sit in a losing position to satisfy an anti-scalping rule would be a rule that causes
the loss it is meant to prevent.

### The rules used on devnet

The three live mandates were funded with these, readable from the accounts themselves:

| Field | Value |
|---|---|
| `max_trade_notional` | $100.00 |
| `max_total_notional` | $100.00 |
| `max_drawdown_bps` | 1000 (10%) |
| `max_risk_per_trade_bps` | 500 (5% of equity) |
| `max_stop_distance_bps` | 300 (3% from entry) |
| `max_concurrent_positions` | 1 |
| `min_hold_slots` | 0 |
| `trader_split_bps` | 7000 (70%) |

These are test values for a $200 mandate, not a recommendation.

---

## 6. The money

### The split

The protocol's 5% comes off the **gross** profit first. The 70/30 split then applies to what
remains, so it stays exactly 70/30. The alternative — a three-way 70/25/5 — would quietly cut
the investor to 25%, and the comment in
[`settlement.rs`](../programs/noxfunds/src/settlement.rs) says so.

Worked example, principal $3,500, final equity $4,500:

```
gross profit                      $1,000
− protocol fee, 5% of gross       $    50
────────────────────────────────────────
net                               $  950
  → trader, 70% of net            $  665
  → investor, principal + 30%     $3,785
```

On a loss there is no profit to share and no fee to take. The investor receives everything that
remains.

### Rounding

Every division rounds **against** the party it is computed for:

- the **protocol fee rounds up** — a charge rounds toward whoever is charging it;
- the **trader's share rounds down** — a payout rounds down;
- the **investor receives the remainder**, computed by subtraction rather than a third division.

So the rounding dust goes to the party whose capital was at risk, and the three shares add up
to the whole **by construction** rather than by luck. This is tested as a property across
thousands of generated inputs, not three hand-picked examples.

### What it costs to run

Prop firms usually charge an up-front evaluation fee that you lose if you fail. The design here
uses a **$50 refundable stake** instead, and a forfeited stake goes to the treasury. The
protocol takes nothing from a mandate that loses money.

---

## 7. The track record, and why you can trust it

Every trader has one `TraderProfile` account. It accumulates **every** trade from **every**
mandate they have ever held. There is no second account, no way to start fresh, and no way to
show one profile while hiding another — the address is derived from the trader's wallet, so
there is exactly one and anybody can find it.

What is recorded: trades, wins, losses, gross profit, gross loss, largest win, largest loss,
total hold time, worst drawdown ever observed, mandates funded, mandates settled in profit.

### Why "win rate" is not the headline number

A 90% win rate next to a 0.6 profit factor is a trader taking tiny wins and enormous losses. It
is the single most common way a track record lies. So in the tier rules below, **win rate never
determines a tier on its own** — profit factor and maximum drawdown do the work, and win rate
only ever adds a condition. `largest_win` is stored beside `gross_profit` for the same reason:
a large profit factor built from one lucky trade reads very differently when you can see it.

### Tiers

Tiers are earned from the profile, and requirements are cumulative — Gold requires everything
Silver requires, and more. These thresholds are **constants in the program**, not
admin-settable fields, and the reason is written into the source:

> A threshold an admin can move is a threshold an admin can move *after* seeing who it would
> promote, and the whole claim of this programme is that the track record is not curated.

Changing one requires a program upgrade, which is public and versioned.

| Tier | Trades | Win rate | Profit factor | Max drawdown | Settled in profit | Max mandate | Concurrent mandates |
|---|---:|---:|---:|---:|---:|---:|---:|
| Bronze | — | — | — | — | — | $10,000 | 1 |
| Silver | 20 | 45% | 1.20× | — | — | $25,000 | 2 |
| Gold | 50 | — | 1.50× | ≤ 4% | — | $75,000 | 3 |
| Platinum | 100 | — | 1.80× | ≤ 4% | 2 | $200,000 | 5 |

Source: [`constants.rs`](../programs/noxfunds/src/constants.rs), [`state.rs`](../programs/noxfunds/src/state.rs).

Note what Platinum requires that the others do not: **two mandates settled in profit**. A live
result with real investor money returned, not a statistic computed from trades inside a mandate
that is still open.

---

## 8. Specification

### Program

| | |
|---|---|
| Program ID | `9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx` |
| Cluster | Solana devnet |
| Framework | Anchor 1.1.2 |
| Venue it trades on | SolFX, `2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi` |
| Settlement asset | USDC, 6 decimals |

The SolFX program is **not modified** by NOXFUNDS. The dependency points one way: NOXFUNDS
depends on SolFX, SolFX knows nothing about NOXFUNDS. This is enforced in CI.

### Accounts

| Account | Address derived from | Holds |
|---|---|---|
| `NoxConfig` | `["config"]` | admin, guardian, treasury, the SolFX program address, USDC mint, fee, pause flag |
| `Mandate` | `["mandate", investor, trader, seq]` | the capital, the rules, the state, the open positions |
| mandate signer | `["signer", mandate]` | nothing — it is dataless. It is the SolFX account authority |
| mandate vault | `["vault", mandate]` | the USDC |
| `TraderProfile` | `["trader", wallet]` | the whole track record |
| `TraderListing` | `["listing", trader]` | what a trader is looking for — advertising, no money |
| `InvestorListing` | `["inv_listing", investor]` | what an investor is offering — advertising, no money |
| `MandateOffer` | `["offer", investor, trader, seq]` | an escrowed proposal: terms, note, expiry, the trader's reply |
| offer vault | `["offer_vault", offer]` | the escrowed USDC until acceptance or revocation |
| `FundingRequest` | `["request", trader, investor]` | a trader's ask, with a note; the trader's rent, always returned |
| `Evaluation` | `["eval", trader, seq]` | a simulated balance, its stage, its record, its loss limits |
| evaluation vault | `["eval_vault", evaluation]` | the $50 stake, until it is refunded or forfeited |
| `VirtualPosition` | `["vpos", evaluation, market_index (LE), nonce]` | one simulated position, priced as a real one |

The `seq` on a mandate lets one investor fund the same trader more than once. The signer is
derived from the *mandate*, not the trader, so a trader holding several mandates from different
investors custodies each pot separately.

### Two structural facts, both found by measurement

These are recorded because they are non-obvious and cost real time to discover:

1. **The `Mandate` account cannot itself be the exchange authority.** Both CPI targets are
   `init, payer = authority`, which Anchor routes through the System Program, which refuses a
   payer that carries data. Hence the separate, dataless signer PDA.
2. **Each CPI is issued from its own `#[inline(never)]` function.** The two `CpiContext` values
   are 840 and 552 bytes held by value; against Solana's 4,096-byte stack frame they cannot be
   allowed to share a frame.

### Instructions — 35

**Admin:** `initialize_config`, `set_paused`
**Trader:** `initialize_trader_profile`, `recompute_tier`, `create_solfx_account`,
`funded_open_position`, `funded_close_position`, `funded_place_take_profit`,
`funded_cancel_stop`
**Investor:** `fund_mandate`, `fund_solfx_collateral`, `request_settlement`, `claim_settlement`
**Public / keeper:** `observe_mandate_equity`, `wind_down_position`, `wind_down_cancel_stop`,
`reconcile_position`
**Marketplace:** `post_listing`, `update_listing`, `post_investor_listing`,
`update_investor_listing`, `post_offer`, `revoke_offer`, `accept_offer`, `decline_offer`,
`post_request`, `close_request`
**Evaluation:** `start_evaluation`, `eval_open_position`, `eval_close_position`,
`eval_trigger_stop`, `eval_observe_equity`, `claim_stage_pass`, `forfeit_stake`,
`abandon_evaluation`

`initialize_config` can only be called by the program's **upgrade authority** — enforced on
chain by checking the program's own `ProgramData` account, not by a stored address that a
first caller could claim. A first-caller-wins initialiser is a standard way to lose a protocol
on deployment day.

### Events — 33

`ConfigInitialized`, `MandateFunded`, `FundedTradeOpened`, `FundedTradeClosed`,
`TakeProfitPlaced`, `StopCancelled`, `EquityObserved`, `MandateBreached`, `SettlementRequested`, `MandateSettled`,
`PositionWoundDown`, `PositionReconciled`, `TraderProfileCreated`, `TradeRecorded`,
`TierChanged`, `ListingPosted`, `ListingClosed`, `InvestorListingPosted`,
`InvestorListingClosed`, `OfferPosted`, `OfferRevoked`, `OfferAccepted`, `OfferDeclined`,
`RequestPosted`, `RequestClosed`, `EvaluationStarted`, `EvaluationTradeOpened`,
`EvaluationTradeClosed`, `EvaluationEquityObserved`, `StagePassed`, `EvaluationFailed`,
`StakeRefunded`, `StakeForfeited`.

Events are the point, not decoration. **Every statistic NOXFUNDS displays must be
re-derivable from these events by a third party** who trusts none of our infrastructure.

### Mandate lifecycle

```
            fund_mandate
                 │
                 ▼
            ┌─────────┐   drawdown breached   ┌──────────┐
            │ Active  │──────────────────────▶│ Breached │
            └─────────┘   (anyone may crank)  └──────────┘
                 │                                  │
   request_settlement (investor)                    │  anyone may
                 │                                  │  wind down
                 ▼                                  │
          ┌─────────────┐                           │
          │ WindingDown │◀──────────────────────────┘
          └─────────────┘
                 │  positions closed, then claim_settlement
                 ▼
            ┌─────────┐
            │ Settled │
            └─────────┘
```

A breached or winding-down mandate can be closed out **by anyone**, with neither the trader nor
any operator cooperating. If the trader disappears mid-position, the investor is not stuck.

---

## 9. What is live, and how to verify it yourself

Four complete mandates have run end to end on Solana devnet — funded, traded, marked, closed
and settled. The fourth was reached through the marketplace rather than funded directly. They
are public accounts. Nothing below requires trusting this document.

| Mandate | Reached by | Principal | Returned to investor | State |
|---|---|---:|---:|---|
| `Bf7aVemJQwTq5trPgqo6t7vjmHy4M3FcxjjHSp1BCyrc` | direct funding | 200.000000 | 199.797874 | Settled |
| `5mhW6R4pZ5vjspS7kPGaojAf3NMaqkRrvgofSHuRBhd7` | direct funding | 200.000000 | 199.770001 | Settled |
| `5qyCdLD2NXCK5jJuHryVwkwvjkxtc2CjeP34DgUohwEs` | direct funding | 200.000000 | 199.790000 | Settled |
| `ECji8hWgCxXqqsnSeT6CPJo9hMnvEX3dvRae7wvCLWbZ` | **the marketplace** | 200.000000 | 199.794167 | Settled |

Trader profile: `85FqmY8snMoFvtKyrLYhXFJW6jCPxBoKfKSzzpEP8F8g` — 4 trades, 0 wins, 4 losses,
gross loss 0.837949, largest loss 0.229999, worst drawdown 12 bps, 4 mandates funded. Each
trade's recorded loss equals its mandate's actual loss to the unit, open fee included.

All four lost a small amount, which is the honest outcome of opening and immediately closing a
position: you pay the spread and the fee both ways. The protocol earned nothing from any of
them, exactly as designed.

### The negotiation, on chain

The fourth mandate was not funded — it was agreed. Six transactions, in consecutive slots from
501,038,400 to 501,038,482, about 33 seconds, on the upgrade deployed at slot 501,032,312:

| # | Move | Account | What the chain holds now |
|---|---|---|---|
| 1 | Trader lists, asking 70/30 | listing `9HyC3HA9xXHr2P2G39JgK7VSyFPhj1BxrYtdYZmtKDU5` | open |
| 2 | Investor offers 60/40, $200 into escrow | offer `GcmUAhtoUWoEs6kyK8A9x9m7v8bncPpcWFC1BMnXgFV5` | |
| 3 | Trader declines, with a reason | same offer | reply stored on the offer |
| 4 | Investor revokes; $200 back out | same offer | `Revoked` |
| 5 | Investor offers 70/30, $200 into escrow | offer `3yBsqe9yPdUMcehak79K55uRGfcJr8DTxUZ2majZ1ezg` | |
| 6 | Trader accepts: mandate, vault and transfer in one transaction | same offer | `Accepted` |

Both sides' words are on the opening offer, decoded from the account rather than quoted from
the client that sent them:

> **Investor:** "60/40. The capital and the risk are both mine."
> **Trader:** "70/30, as the listing says. Happy to trade it at that."

There is no chat. The offer *is* the message, the escrow is the proof it is serious, and the
acceptance is the signature — so the terms the trader accepted are provably the terms the
investor published, with no step in between where either could substitute a number.

### An evaluation, on chain

`9WTKz4b1QmQTfkVoqfiaUfSoBCLFr6sMoc3gsionDp1A` — Phase 1 of an evaluation on a simulated
$10,000 account, read back from the account:

| | |
|---|---|
| Stake | **$50 of real USDC**, held by the evaluation's own vault — the only real money involved |
| The trade | one simulated BTC/USD long, $1,000.999974 notional, entry 81,196.08 against an oracle of 81,114.97 — above it, because a buy crosses the spread exactly as a real fill does |
| Fee | 0.100100, the same fee a real open of that size pays |
| Hold | closed after **602 seconds**, against a 600-second minimum |
| Result | balance 9,996.236574; 1 trade, 0 wins, 1 loss; still `Active` in Phase 1 |

The 602 seconds is worth a sentence. The minimum hold is judged against the cluster's clock,
which is a stake-weighted estimate that may run ahead of or behind wall time, so the client
waited on the Clock sysvar rather than its own clock and closed two seconds after the minimum —
not early, which would have been refused, and not needlessly late.

### Check the accounts

```bash
solana account 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx --url devnet
solana account Bf7aVemJQwTq5trPgqo6t7vjmHy4M3FcxjjHSp1BCyrc --url devnet
```

Or open any address above in [Solana Explorer](https://explorer.solana.com/?cluster=devnet).

### Check the code matches what is deployed

The repository is public, so a deterministic build can be compared against the deployed
bytecode:

```bash
cargo install solana-verify
solana-verify build --library-name noxfunds
solana-verify get-executable-hash target/deploy/noxfunds.so
solana-verify get-program-hash -u devnet 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx
```

If the two hashes match, the deployed program is this source code.

**One honest caveat:** Solana's *remote* verification service — the one that puts a green
"verified" badge on Explorer — only works on mainnet. On devnet the comparison above is a local
one that you run yourself. It is just as conclusive; it simply is not displayed by a third
party.

### Run the whole lifecycle yourself

```bash
cargo run -p solfx-keeper --bin nox -- lifecycle                 # fund a mandate directly
cargo run -p solfx-keeper --bin nox -- lifecycle --via-market    # reach it by negotiating
cargo run -p solfx-keeper --bin nox -- eval                      # stake and trade an evaluation
```

Without `--execute` none of these sends anything. Each reads the chain, checks every rule the
trade would be judged against, and prints the plan. With it, every move is simulated before it
is sent, so a refusal prints the program's own reason and costs nothing.

---

## 10. What is not built

This section exists because a document that only lists what works is marketing.

**Not built:**

- **Passing an evaluation has never happened on devnet.** One has been staked, traded, marked
  and closed there (Part 9), but a stage passes only on an 8% target over at least ten trades on
  five distinct days, so `claim_stage_pass` and the stake refund are proven in LiteSVM only.
- **Half of the marketplace has not run on a cluster.** The investor-to-trader direction —
  listing, offer, decline, revoke, accept — ran end to end on devnet. The other direction —
  an investor's listing and a trader's funding request — has zero accounts on devnet and is
  proven in LiteSVM only.
- **The evaluation's simplifications.** A simulated trade does not move open interest; the
  whole simulated balance is the margin, so there is no simulated liquidation; fees are the
  entry tier's; only single-leg markets can be traded; and the $50 stake is flat — the plan says
  it rises for larger evaluations but never says by how much. Passing records no tier: tiers
  come from funded trading, as they always have.
- **Resting entry orders do not exist, on either protocol.** SolFX's trigger orders attach to
  an open position, so they can close one and cannot open one. A "limit order" on the trader
  dashboard would be the browser watching a price and sending when it hits — which stops the
  moment the tab closes — so it is not offered. The stop and the take-profit are real on-chain
  orders a keeper fires. Adding a genuine resting entry would be a change to `solfx-core`.
- **The browser ticket trades single-leg markets only.** `funded_open_position` accepts the
  secondary and quote-conversion oracle legs a synthetic or non-USD-quoted market needs; the
  page resolves one price account per market and so lists only the markets that need one.
  EUR/JPY and USD/INR are `nox market --execute` for now, and the panel says so rather than
  offering a trade that would be refused.
- **A mandate's SOL is not recoverable.** The mandate signer is a PDA that pays the rent for
  the SolFX account and for every resting order, and is refunded when those close. Nothing in
  the program sweeps what is left, and a PDA has no key, so roughly 0.02 SOL per mandate stays
  there after settlement. Small, and stated rather than discovered.
- **The public verification page.** The claim in Part 8 that every statistic is re-derivable
  from events is true of the event data; the page that does the re-deriving does not exist.
- **The off-chain keeper.** The cranks are public instructions, but nothing runs them
  automatically yet. A mandate is currently marked when someone marks it.

**Not done:**

- **No security audit. Zero, by anyone.** Not a formal audit, not an informal one.
- **No fuzzing.** SolFX's own fuzzing is a later-phase exit criterion; NOXFUNDS has none.
- **Never deployed to mainnet, and no mainnet date.** Devnet only, with test USDC.
- **Not economically tested at size.** The largest mandate ever run is $200.
- **The upgrade authority is a single key**, and it is the same key that holds authority over
  SolFX. That means one signature controls both protocol revenue and the ability to replace the
  program. It is stored as a config field so it can be moved to a multisig; it has not been.

**What has been measured** rather than assumed: compute cost, transaction size, account count
and stack usage per instruction, all pinned by tests that fail if they regress. Those figures
are in [`docs/noxfunds-budgets.md`](noxfunds-budgets.md).

Green tests mean the behaviours someone thought to test behave as expected. Nothing more.
NOXFUNDS is not audited, not safe, and not production-ready.

---

## Further reading

- [`docs/NOXFUNDS-PLAN.md`](NOXFUNDS-PLAN.md) — the full internal design, 752 lines
- [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) — the SolFX exchange underneath
- [`docs/FOREX-EXPLAINED.md`](FOREX-EXPLAINED.md) — trading concepts from zero
- [`architecture/`](../architecture/) — diagrams
- [`programs/noxfunds/src/`](../programs/noxfunds/src/) — the program itself
