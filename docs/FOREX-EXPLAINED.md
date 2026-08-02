# SolFX Explained — In Plain English

**Who this is for:** you, before you make decisions about this project.
**What it assumes:** nothing. No forex knowledge, no DeFi knowledge.
**How to read it:** in order. Each part builds on the one before.

Every idea here comes with real numbers. If a section feels abstract, look at the example — the example is the actual explanation.

---

## Table of Contents

**Part 1 — Forex basics**
1. [What forex trading actually is](#1-what-forex-trading-actually-is)
2. [Pips, lots, leverage, margin](#2-the-four-words-that-matter)
3. [How XM and Exness actually make money](#3-how-xm-and-exness-actually-make-money)

**Part 2 — What we are building**
4. [Long, short, and where profit comes from](#4-long-short-and-where-the-money-comes-from)
5. [What a "perpetual" is and why we use one](#5-what-a-perpetual-is-and-why-we-use-one)
6. [Oracles, Pyth, and confidence](#6-oracles-pyth-and-confidence)
7. [The LP vault — who pays you when you win](#7-the-lp-vault--who-pays-you-when-you-win)
8. [Liquidation and health factor](#8-liquidation--the-most-important-mechanic)
9. [Swap and carry — the overnight fee](#9-swap-and-carry--the-overnight-fee)

**Part 3 — Our strategy**
10. [Why gold, why weekends, why emerging markets](#10-why-gold-why-weekends-why-emerging-markets)
11. [How we make money](#11-how-we-make-money)
12. [How adding new pairs works](#12-how-adding-new-pairs-works)

**Part 4 — The build**
13. [Every phase in plain English](#13-every-phase-in-plain-english)
14. [Every big decision and why](#14-every-big-decision-and-why)
15. [Glossary](#15-glossary)

---

# Part 1 — Forex Basics

## 1. What forex trading actually is

Forex = foreign exchange = swapping one currency for another.

When you see **EUR/USD = 1.0850**, it means:

> 1 Euro costs 1.0850 US Dollars.

The first currency (EUR) is the **base**. The second (USD) is the **quote**. The number tells you how much *quote* you need to buy one *base*.

Trading forex means betting on that number going up or down.

### A complete trade, start to finish

You think the Euro will get stronger against the Dollar.

| Step | What happens | Numbers |
|---|---|---|
| 1 | EUR/USD is at **1.0850** | |
| 2 | You buy 100,000 Euros | Costs 100,000 × 1.0850 = **$108,500** |
| 3 | You wait. Price rises to **1.0900** | |
| 4 | You sell your 100,000 Euros back | Get 100,000 × 1.0900 = **$109,000** |
| 5 | **Profit** | $109,000 − $108,500 = **$500** |

That's it. That's the whole game. Buy low, sell high — the only unusual part is that the "thing" you're buying is money.

**But notice the problem:** you needed **$108,500** to make **$500**. That's a 0.46% return. Currencies barely move — EUR/USD typically shifts about 0.5% in a whole day. Nobody would bother.

That is why leverage exists, and why leverage is the entire retail forex industry.

---

## 2. The four words that matter

If you learn only four things about forex, learn these.

### Pip — the unit of movement

A **pip** is the smallest normal price move. For most pairs it's the 4th decimal place.

```
1.0850  →  1.0851     =  1 pip
1.0850  →  1.0900     =  50 pips
1.0850  →  1.0750     =  −100 pips
```

For pairs involving Japanese Yen, it's the 2nd decimal place instead (USD/JPY 157.20 → 157.21 = 1 pip), because Yen numbers are ~100x bigger.

Traders talk entirely in pips. "I made 30 pips today." Not percentages. Our interface must speak this language.

### Lot — the unit of size

A **lot** is a standard trade size. There are three:

| Name | Units of base currency | What 1 pip is worth |
|---|---|---|
| **Standard lot** | 100,000 | **$10** |
| **Mini lot** | 10,000 | **$1** |
| **Micro lot** | 1,000 | **$0.10** |

Why is 1 pip always $10 on a standard lot? Simple multiplication:

```
100,000 units × 0.0001 (one pip) = $10
```

This holds for any pair ending in USD. It's why traders memorise it.

**So:** 1 standard lot, 50 pips of profit = 50 × $10 = **$500**. Which matches our example above. Good.

### Leverage — borrowing to trade bigger

**Leverage** lets you control a big position with a small amount of money.

At **50x leverage**, $2,170 controls a $108,500 position.

```
Position size ÷ Leverage = Money you must put up
$108,500 ÷ 50 = $2,170
```

Now redo our trade:

| | Without leverage | With 50x leverage |
|---|---|---|
| Money needed | $108,500 | **$2,170** |
| Profit on +50 pips | $500 | $500 |
| **Return** | +0.46% | **+23%** |

Same profit. 50x less money. That's the appeal.

**But it cuts both ways.** If the price had dropped 50 pips instead:

| | Without leverage | With 50x leverage |
|---|---|---|
| Loss | −$500 | −$500 |
| **Return** | −0.46% | **−23%** |

You'd lose a quarter of your money on a move that happens most days.

> **This is the single most important fact about forex:** leverage doesn't change how much you win or lose in dollars. It changes how much money you had to risk to get there. Same dollars, smaller cushion.

### Margin — your safety cushion

**Margin** is the money you put up. Two kinds:

- **Initial margin** — what you need to open. At 50x on $108,500, that's $2,170.
- **Maintenance margin** — the minimum you must keep, or you get closed. Usually about half the initial. Say $1,085.

The gap between them is your survival room. When your losses eat through it, you get **liquidated** (§ 8).

---

## 3. How XM and Exness actually make money

Understanding this tells you how *we* make money, and where they're vulnerable.

### The spread

Brokers quote two prices, not one:

```
EUR/USD    SELL 1.08495    BUY 1.08505
                   ↑              ↑
              you sell here   you buy here
                   └──── 1 pip apart ────┘
```

You always buy slightly high and sell slightly low. That gap is the **spread**, and the broker keeps it.

On 1 standard lot, a 1-pip spread = **$10**, taken the instant you open. You start every trade already down $10.

**This is their main income.** Small per trade, enormous at scale — Exness processes trillions of dollars of volume monthly.

### The swap (overnight fee)

Hold a position past 5pm New York, and you pay or receive interest. It's based on the interest-rate difference between the two currencies (explained fully in § 9).

Brokers add a hidden markup here. You have no way to see the real rate versus their cut.

### The B-book

Here's the uncomfortable part. When you trade with XM, in most cases **nobody buys Euros for you.** The broker just records your bet and takes the other side.

If you lose, they keep your money. If you win, they pay from their pocket.

This is called **B-booking**, and it's legal and normal. But it means your broker profits when you lose. Most retail traders lose, so the maths works for them.

It also creates an incentive problem: the broker controls your price feed, your execution, and your stop-loss. There's a documented industry history of "stop hunting" and widened spreads at convenient moments.

**This matters for us because we're building a B-book too** — the difference is *ours runs on rules nobody can change mid-trade, and you can read the code*. More in § 7.

### IBs — how they actually grew

XM and Exness didn't win on spreads. They won on **Introducing Brokers** — affiliates paid a rebate for every lot their referrals trade. Thousands of them, worldwide, running YouTube channels and Telegram groups.

Every IB has the same complaint: **the broker owns the scoreboard.** Rebates get miscounted, clients get reassigned, payouts get delayed. You cannot audit it.

**That's our opening.** On a blockchain, the scoreboard is public and the payout is automatic. Nobody can shave your rebate. This is our main growth plan (§ 11).

---

# Part 2 — What We Are Building

## 4. Long, short, and where the money comes from

**Long** = betting the price rises. **Short** = betting it falls.

Shorting sounds strange (how do you sell something you don't own?) but on our platform it's simple, because you never actually touch Euros. You're only recording a bet.

The formula is one line:

```
Profit = Size × (Exit price − Entry price)      ← if you're LONG
Profit = Size × (Entry price − Exit price)      ← if you're SHORT
```

Worked, 1 standard lot:

| | Entry | Exit | Move | Long profit | Short profit |
|---|---|---|---|---|---|
| A | 1.0850 | 1.0900 | +50 pips | **+$500** | −$500 |
| B | 1.0850 | 1.0800 | −50 pips | −$500 | **+$500** |

Perfectly mirrored. Every dollar a winner makes, someone loses. **Trading is zero-sum before fees, and negative-sum after them.**

Who's the someone? On SolFX, the LP vault (§ 7).

---

## 5. What a "perpetual" is and why we use one

We are **not** actually exchanging currencies. No Euros move anywhere. Doing real forex would require bank accounts, custody, settlement, licences — a decade of work.

Instead we build a **perpetual swap** (perp): a contract that just tracks a price.

Here's the whole idea:

> You deposit **USDC** (a digital dollar). You tell the program "I'm long 1 lot of EUR/USD at 1.0850." The program writes that down. Later you close, the program checks the price, and pays or takes USDC based on the difference.
>
> No Euros were ever involved. Only the *number* mattered.

**"Perpetual"** means it never expires. Traditional futures contracts have an end date; you have to roll them over. A perp just sits there until you close it — exactly how a retail forex position behaves. That's why it's the right instrument for our audience.

### The three things a perp needs

1. **A trusted price.** Since nothing is really traded, we need an honest outside source. → the oracle (§ 6)
2. **Someone to pay winners.** → the LP vault (§ 7)
3. **A way to stop losers from going below zero.** → liquidation (§ 8)

Those three sections are the heart of the entire protocol. Everything else is plumbing.

---

## 6. Oracles, Pyth, and confidence

### What an oracle is

A blockchain can't see the outside world. It doesn't know what EUR/USD costs. An **oracle** is a service that brings real-world prices onto the chain.

We use **Pyth**. Banks, exchanges, and trading firms publish their prices to Pyth; Pyth combines them and puts the result on Solana, updating sub-second.

Why Pyth over alternatives:
- It was built on Solana (native, fast)
- It has forex feeds — including emerging-market ones almost nobody else has
- ~~It has 24/7 gold and silver indices (the basis of our weekend feature)~~ — **false, measured 2026-08-01: every Pyth metals feed closes with the Friday market. See § 10.**
- It gives you a **confidence interval**, which turns out to be critical

### Confidence — the thing most people miss

Pyth doesn't just say "EUR/USD is 1.08500." It says:

```
EUR/USD = 1.08500  ±  0.00005
                       ↑
                  confidence
```

Meaning: *"Our best estimate is 1.08500, and we're confident the true price is between 1.08495 and 1.08505."*

That band is half a pip wide. It widens during news events — sometimes to 5+ pips.

**Why this can destroy us.** Suppose someone has a faster price feed than Pyth. They see the real price is 1.08505 while Pyth still says 1.08500. They buy from us at 1.08500 — instantly up half a pip, no risk taken. Then they do it again. And again. Thousands of times a day, with bots.

This is called **toxic flow** or **latency arbitrage**, and it is the single most common way trading protocols like ours bleed to death.

**The fix — quote the edge, not the middle.** If you're buying, we charge you 1.08505 (the top of the band). If you're selling, we pay you 1.08495 (the bottom). Whatever side of the uncertainty hurts you, that's your price.

Costs an honest trader about $5 on a standard lot. Costs an arbitrage bot its entire business model. Worth it.

And when Pyth gets less certain (news event, band widens to 5 pips), your spread automatically widens too. We charge more precisely when we know less. That's not a trick — it's the only defensible way to price against an uncertain feed.

---

## 7. The LP vault — who pays you when you win

If you make $500, that money comes from somewhere. On SolFX it comes from the **liquidity vault**.

### How it works

People deposit USDC into a shared pool. In return they get **LP tokens** — a receipt for their share.

That pool is the counterparty to every trade:

- Traders lose → the pool grows
- Traders win → the pool shrinks
- Every trade → the pool earns a cut of the fees

So LPs are essentially **the house**. They earn steady fee income and take on trading risk.

### Numbers

Vault has **$1,000,000**.

| Month | What happened | Effect |
|---|---|---|
| Fees earned | $50M volume × 1 bp × 55% LP share | **+$2,750** |
| Trading result | Traders lost $8,000 net | **+$8,000** |
| **Total** | | **+$10,750** (+1.08%) |

Good month. Now a bad one:

| Month | What happened | Effect |
|---|---|---|
| Fees earned | Same | **+$2,750** |
| Trading result | Gold spiked, longs won $40,000 | **−$40,000** |
| **Total** | | **−$37,250** (−3.7%) |

**LPs can lose money.** They must be told this clearly. Anyone hiding it deserves what follows.

### Being honest about what this is

Here's something to internalise, because reviewers will raise it:

**The LP vault is a B-book.** It profits when traders lose. Structurally identical to XM.

The original project proposal claims SolFX "removes the counterparty conflict of interest." That claim will not survive a knowledgeable reader, and asserting it damages the credibility of everything else in the document.

**What's actually true, and is a genuinely strong claim:**

| XM's B-book | Our B-book |
|---|---|
| One company, secret balance sheet | Public pool, anyone can join or exit |
| They control your price | Price comes from Pyth, we can't touch it |
| They can change rules silently | Rules are code; changes are visible on-chain |
| They can freeze withdrawals | Your collateral is in your own account |
| You cannot audit anything | You can audit everything |

Say *that*. It's true, it's checkable, and it's still much better than the incumbent.

### Keeping the vault balanced

If everyone is long EUR/USD, the vault is short EUR/USD — a big directional bet nobody chose. Two tools fix it:

1. **Funding** — the crowded side pays the empty side a small hourly fee, which attracts traders to balance it out. This money moves between traders, not from the vault.
2. **Skew pricing** — it gets progressively more expensive to add to the crowded side, and cheaper to trade against it.

---

## 8. Liquidation — the most important mechanic

Leverage means you can lose more than you put up. Liquidation stops that.

### The rule

> When your losses have eaten through most of your margin, we close your position automatically.

We *have* to. If your position went negative, the vault would eat the shortfall, and LPs would be funding your losses. Not acceptable.

### Full worked example

You go long 1 standard lot of EUR/USD at 1.0850, 50x leverage.

```
Position size:          $108,500
Your margin:            $2,170        (108,500 ÷ 50)
Maintenance margin:     $1,085        (1% of position)
```

Now watch what happens as the price falls. Remember: 1 pip = $10.

| Price | Pips | Your P&L | Equity | Health Factor | Status |
|---|---|---|---|---|---|
| 1.0850 | 0 | $0 | $2,170 | **2.00** | Fine |
| 1.0830 | −20 | −$200 | $1,970 | **1.82** | Fine |
| 1.0800 | −50 | −$500 | $1,670 | **1.54** | Fine |
| 1.0790 | −60 | −$600 | $1,570 | **1.45** | Getting warm |
| 1.0770 | −80 | −$800 | $1,370 | **1.26** | Warning |
| 1.0750 | −100 | −$1,000 | $1,170 | **1.08** | Danger |
| **1.07415** | **−108.5** | **−$1,085** | **$1,085** | **1.00** | ⚠️ **LIQUIDATED** |

**Health Factor** is just:

```
Health Factor = Equity ÷ Maintenance margin
```

- Above 1.0 → you're fine
- Exactly 1.0 → liquidation triggers
- Think of it as "how many times over can I cover the minimum"

### The uncomfortable takeaway

Your position died on a **1% move**. EUR/USD does 1% in about two normal days — or in ninety seconds when a central bank speaks.

**That is what 50x leverage means.** Not "50x the profit." It means a 1% move against you ends everything.

This is exactly why the plan starts at 20–50x and not the 500x that GMTrade offers. Higher leverage sounds better in marketing and is worse in every other way.

### Who does the closing?

**Keepers** — bots that watch every position and submit a liquidation when the health factor breaks 1.0. Anyone can run one; they earn a share of the liquidation fee.

They must be fast. If the price gaps past the liquidation point before the bot fires, the position goes negative and the vault absorbs the difference. That's called **bad debt**, and it's why we keep an **insurance fund** — a reserve, built from fees, that absorbs shortfalls before LPs feel them.

---

## 9. Swap and carry — the overnight fee

Every currency has an interest rate set by its central bank. Holding a currency means earning (or paying) that rate.

**Right now, roughly:** Euro ≈ 2%/year, US Dollar ≈ 4%/year.

If you're long EUR/USD, you're holding Euros and effectively borrowing Dollars. You earn 2%, pay 4%. Net: **you pay 2% per year.**

On a $108,500 position:

```
$108,500 × 2% ÷ 365 = $5.95 per day
```

Short EUR/USD, and you *receive* roughly that instead.

### The part brokers hide

Your broker doesn't charge you $5.95. They charge maybe $9.00 and pocket the difference. You cannot see the split. You find out at rollover, if you notice at all.

The original proposal correctly names this as vulnerability #3. **Our answer:**

```
┌─────────────────────────────────────────────┐
│  EUR/USD — Overnight Swap                   │
│                                             │
│  Interest rate differential   −$5.95/day    │
│  SolFX markup                 −$0.60/day    │
│  ─────────────────────────────────────────  │
│  You pay                      −$6.55/day    │
└─────────────────────────────────────────────┘
```

Both numbers, on screen, before you trade, verifiable on-chain.

That's a small feature that is genuinely impossible for XM to match without giving up revenue — and it's the kind of concrete honesty that earns trust faster than any whitepaper claim.

---

# Part 3 — Our Strategy

## 10. Why gold, why weekends, why emerging markets

### First, the bad news

The original proposal's core claim — *"first forex brokerage on Solana"* — is **not true**.

**GMTrade** launched forex on Solana in **January 2026**, with the exact four pairs we planned to launch with. They're now the **#1 perpetuals exchange on Solana by volume**: about $554 million a day, $2.57M in fees per month.

It's better to know this now than to have a reviewer discover it. And it doesn't sink the project — it redirects it.

### The three real openings

**1. Gold, and the weekend.** — **This one did not survive contact with reality — see below.**

XAU/USD (gold) is the most-traded instrument at retail forex brokers. At XM and Exness it frequently beats EUR/USD in volume. Forex traders love gold.

Normally forex closes Friday evening and reopens Sunday evening. Nobody trades the weekend.

~~But in **June 2026, Pyth launched 24/7 index prices for gold and silver** — continuous prices even when the metal markets are shut. Coinbase, Kraken, and dYdX already use them for exactly this. **So we can keep gold open all weekend.**~~

> ### 🚨 Checked on a Saturday. It isn't there.
>
> On **Saturday 1 August 2026** we asked Pyth directly what gold was worth. The answer came back
> stamped **Friday at 21:00** — nearly a full day stale. We then checked *every* metal and commodity
> price Pyth publishes. **All 137 of them were frozen at the same Friday close.** Not one was live.
>
> To be sure the problem wasn't at our end, we asked for Bitcoin's price in the very same request.
> It came back current to the second. Pyth was working fine. Gold was simply shut.
>
> **So the weekend-gold feature, as planned, cannot be built.** The 24/7 gold price we designed
> around is not something a Solana program can actually read today.
>
> **What this cost us:** nothing but a week of measurement — which is precisely the point. That
> claim had already worked its way into the full technical design: a special "continuous market"
> mode, four weekend risk settings, and a requirement to staff on-call seven days a week. All of it
> resting on a vendor's launch announcement that nobody had checked against a live Saturday. Phase 0
> exists to catch exactly this, and it caught it **before a single line of protocol code was written.**
>
> **One thread remains.** Pyth may sell this as a separate paid product on a different endpoint. Worth
> one email. Until it's answered, we plan as though weekend gold does not exist.
>
> **What about the "gold" prices that *are* live at weekends?** There are three — XAUT, PAXG and
> XAUM. They are *tokenised* gold: crypto tokens that promise to be worth gold, which trade all
> weekend. But they don't agree with each other or with real gold — they were 23 to 37 basis points
> apart when we measured, against real gold's normal 1.4. Calling those "gold" to a forex trader
> would be misleading, and settling someone's liquidation on them is a different product with its own
> risks. We're measuring them properly before deciding anything.

**Where that leaves us:** gold stays on the list, on normal weekday hours, where it is the
best-quality feed we measured of anything. It just isn't a weekend product, and we should stop
saying it is. The remaining two openings below are unaffected — and they were always the stronger pair.

**2. Emerging-market currencies.**

Pyth has feeds for **Indian Rupee, Indonesian Rupiah, Philippine Peso, Korean Won**, and others.

**No platform on any blockchain offers leveraged trading on these.** Not GMTrade, not Ostium, not anyone.

And demand is real: these are countries with high crypto adoption, currency depreciation worries, capital controls, and large remittance flows. People there genuinely want dollar exposure and often can't easily get it.

> ⚠️ **The honest trade-off:** this niche is empty partly *because* it's legally messy. India and Indonesia restrict currency derivatives for residents. Their currencies are managed by central banks and can jump suddenly when policy changes. On devnet with test money that's zero risk — but before real money, this needs a lawyer and geographic blocking.

**3. The IB network.**

Covered in § 3. Nobody on-chain has built one. It's the actual growth engine of retail forex and it maps perfectly onto smart contracts.

### Our one-line position

> ~~Gold that trades all weekend,~~ Currencies nobody else lists, collateral that never leaves your own account, and a partner-rebate ledger you can audit — built for forex traders, not crypto traders.
>
> *(Weekend gold struck 2026-08-01 — the feed it needed does not exist. See § 10.)*

---

## 11. How we make money

### The key insight

Crypto perpetual exchanges make most of their money from **overnight fees**, because crypto positions are held for days.

**Forex is the opposite.** Forex traders open and close constantly — many round trips per day. So forex revenue is **spread × volume**. That's how XM and Exness earn; volume is everything.

This means we must not copy crypto fee levels. Compare:

| Venue | Cost to open and close 1 lot |
|---|---|
| XM / Exness | ~$10 (1 pip spread) |
| Jupiter Perps (crypto) | ~$130 |
| **Original proposal's 0.02%** | **~$43** |
| **Our target (1 bp/side)** | **~$22** |

The proposal's 2 bps would have made us **4x more expensive than the broker we're trying to beat.** Crypto perps get away with high fees because Bitcoin moves 3% a day. EUR/USD moves 0.5%. The same fee hurts six times more.

At ~$22 round trip we're roughly 2x XM — which is a fair price for not surrendering custody, plus pairs they don't have, plus a rebate ledger you can audit.

### The five income streams

| # | Source | Share | How it works |
|---|---|---|---|
| 1 | **Trading fee** | ~85% | ~1 bp each way, cheaper at higher volume |
| 2 | **Swap markup** | ~7% | Small transparent markup on the overnight rate |
| 3 | **Liquidation fees** | ~4% | A slice of each liquidation penalty |
| 4 | **LP performance fee** | ~3% | 10% of LP profits above their high-water mark |
| 5 | **LP exit fee** | ~1% | 0.05% on withdrawal, discourages gaming |

Every fee splits automatically:

```
LP vault      55%   ← must be biggest, or nobody provides liquidity
Treasury      25%   ← your income
Insurance     10%
IB rebates    10%
```

**Don't take a bigger treasury cut early.** Under-paying LPs is the most common way platforms like this die: no liquidity → no depth → no traders → no fees. Raise your share later from strength.

### What you'd actually earn

| Scenario | Daily volume | Your income |
|---|---|---|
| **Beta** (month 6–9) | $2M | **~$53/day** → $19k/year |
| **Early traction** (year 1–2) | $50M | **~$1,325/day** → $484k/year |
| **Established** (year 3) | $500M | **~$13,000/day** → $4.7M/year |
| **GMTrade's current scale** | $554M | ~$14,400/day |

**Read the first row honestly.** Year one loses money. Infrastructure alone (dedicated node access, keeper servers, database, monitoring) runs $1,500–3,000/month — before audits, which are $40k–150k each. Plan for 12–18 months of negative cash flow.

**The number that decides which row you land in is distribution, not code.** Which is why the IB network is a core phase and not a nice-to-have.

---

## 12. How adding new pairs works

You asked: *"once deployed on a blockchain you can't change it — so how do I add pairs later?"*

Good instinct, but **on Solana this is mostly not true.** Two separate things:

### Solana programs CAN be upgraded

This is a genuine difference between blockchains.

- **Ethereum:** contracts really are frozen forever. Developers need complicated workarounds ("proxies") to change anything.
- **Solana:** programs have an **upgrade authority** — a key that can push new code to the *same address*. Users see nothing change.

You can voluntarily give up that key later to prove you'll never change the code (some projects do this as a trust signal). But you don't have to, and you shouldn't early on.

### But adding a pair doesn't even need an upgrade

This is the important part, and it's why the architecture is built the way it is.

We keep **code** and **data** separate:

- The **program** is a general engine. It doesn't know EUR/USD exists. It knows how to run *any* market.
- Each **market** is a small data record, created by sending a normal transaction.

Adding EUR/JPY looks like this:

```
initialize_market(
    symbol:        "EURJPY",
    price_feed:     0x...,
    max_leverage:   30,
    ...
)
```

**One transaction. About one second. Roughly $0.50 in rent.**

No redeploy. No downtime. No migration. Existing positions completely untouched. You could add 50 pairs in an afternoon.

> **Think of it like a spreadsheet.** The program is the *formula*. Markets are *rows*. Adding a row doesn't mean rewriting the formula.

### The part your instinct got right

There *is* something you genuinely can't undo: **the shape of stored data**.

Solana reads accounts by counting bytes. If a record is laid out as `[size][price][collateral]` and you later reorder it to `[price][size][collateral]`, every existing record becomes garbage — the program reads the wrong bytes for every field.

**Rules that follow:**
- You may **add** new fields
- You may **never** remove, reorder, or change the type of existing ones

Our fix: every data structure carries **128 bytes of reserved empty space**. Need a new field later? Take it from the reserve. Nothing breaks.

### Getting to 100+ pairs

XM has 55 currency pairs. Exness has 100+.

**Good news from Phase 0b:** Pyth actually carries **290 FX feeds**, about 180 of them crosses served
directly. We measured them, and the crosses are *tighter* than the majors — EUR/JPY at 0.59 bps
versus EUR/USD at 1.28 bps.

So the original plan to *compute* crosses from two feeds is unnecessary for v1: computing EUR/JPY
from EUR/USD and USD/JPY would produce a **worse** price than the one Pyth already publishes, while
doubling the number of things that can break. We list direct feeds.

The machinery for computed crosses stays in the design for pairs Pyth genuinely lacks — it just
isn't on the critical path any more.

### And crypto later?

BTC/USD, ETH/USD, SOL/USD are just more `initialize_market` calls — **provided the engine never assumes "this is forex" anywhere in its code.**

That's a rule we follow from the very beginning. Crypto is actually *easier* than forex (already 24/7, no market-hours calendar). Cost of building it generic now: basically nothing. Cost of retrofitting later: a rewrite.

---

# Part 4 — The Build

## 13. Every phase in plain English

| Phase | Time | In one sentence | Done when |
|---|---|---|---|
| **0a** | 3 days | Write this document | You understand the project well enough to make decisions |
| **0b** | 1 wk | Check Pyth's feeds are actually good enough | We know which pairs we can safely list |
| **1** | 2 wks | Set up the project and write the maths | Every calculation is proven correct by tests |
| **2** | 3 wks | Deposits, withdrawals, and reading prices | You can put USDC in and take it out; prices arrive correctly |
| **3** | 4 wks | Opening and closing positions | A full trade works, and can't be gamed for free money |
| **4** | 4–5 wks | Risk: liquidations, market hours, insurance | Survives a simulated market crash without losing money |
| **5** | 2 wks | The LP vault | LPs can deposit, earn, and withdraw with exact accounting |
| **6** | 2 wks | The IB rebate program | Partners earn and claim automatically, verifiably |
| **7** | 3 wks | The keeper bots | Liquidations happen within 2 seconds, 7 days a week |
| **8** | 5 wks | The website | You can trade from a browser |
| **9** | 4 wks | Add more pairs, stress-test, go public on devnet | 20+ markets live, 30 days with no money-losing bugs |

**Total: about 7.5 months.** At that point you have a complete, working, tested platform running publicly on Solana's test network — a genuinely strong capstone.

**After that** (only if there's traction or grant money): security audits, real-money launch, legal work.

### Why Phase 0b comes before any code

Our entire strategy depended on Pyth's emerging-market and 24/7 gold feeds being good enough. **We hadn't confirmed that.**

So before writing a single line of the protocol, we ran a small script for 7 days that recorded: how often each feed updates, how many banks contribute, how wide the uncertainty band is, and exactly when each market opens and closes.

**It paid for itself immediately.** It found that eight of the emerging-market pairs we planned around
have never published a single price, and that the 24/7 gold feed the weekend product depended on is
not reachable at all. One week of measuring saved four months of building on a bad assumption.

### Two tests that matter more than the rest

**1. "Open and immediately close must always lose money."**

If a trader can open and instantly close for a profit, they've found free money and bots will extract it until the vault is empty. We test this across millions of random combinations. It must *never* pass.

**2. "Survive the Swiss Franc crash."**

On 15 January 2015, the Swiss central bank removed its currency peg without warning. EUR/CHF fell 30% in minutes. It bankrupted real brokers — including **Alpari UK, which the original proposal names as an example.**

We replay that exact price data through our engine. If the insurance fund survives, we have something real. If not, we have a demo.

---

## 14. Every big decision and why

| Decision | What we chose | The alternative | Why |
|---|---|---|---|
| **Who takes the other side** | One shared LP vault | An order book matching traders | An order book needs professional market makers before it needs traders. We'd have neither. A vault gives instant fills from day one. |
| **How to price** | Oracle price, adjusted for uncertainty | Just use the oracle mid-price | Using the mid-price hands free money to anyone with a faster feed. This is how similar platforms have died. |
| **Collateral** | USDC only | Accept several currencies | Multiple collateral types means valuing and liquidating the collateral itself. Big extra risk surface for a first version. |
| **Margin type** | Isolated (each position separate) | Cross (all positions share) | Isolated is what forex traders already understand, and it makes liquidation a cheap single-account operation. |
| **Maths** | Whole numbers only, no decimals | Floating-point | Floating-point maths can differ slightly between computers. On a blockchain, that breaks consensus. Non-negotiable. |
| **Starting leverage** | 50x | 500x, like GMTrade | Every leverage *increase* is easy PR. Every *decrease* looks like distress. Start low. |
| **Which pairs first** | Gold, silver, EUR/USD, GBP/USD | Everything at once | Gold measured the best-quality feed of anything we tested. Two majors give credibility. Prove the engine before scaling. |
| **Weekend trading** | ~~Gold yes, currencies no~~ **Nothing, pending Q1b** | 24/7 everything | Measured 2026-08-01: metals freeze at the weekend exactly like currencies. Trading a frozen price is free money for anyone reading the news. |
| **Cross pairs** | Use Pyth's direct feeds | Compute them from two feeds | Phase 0b measured Pyth's crosses as *tighter* than the majors. Computing them would be worse and more fragile. |
| **Token** | LP token only | Launch a governance token | A governance token adds legal complexity and distribution work while adding zero engineering value before launch. |
| **Who controls upgrades** | Multisig (3 of 5 people) | One person's key | If one key can change everything, users trust you exactly as much as they trust XM. The non-custodial claim needs this. |
| **Open source** | At audit time | Day one | Publishing before an audit invites people to fork your unaudited bugs. The transparency promise only has to be true at launch. |
| **Build or buy** | Build the full protocol | Layer on top of GMTrade | Layering is faster to revenue but you own no economics and it's a much weaker capstone. |

---

## 15. Glossary

| Term | Meaning |
|---|---|
| **ADL** | Auto-deleveraging. Last-resort: force-close winning positions when the insurance fund runs out. |
| **Anchor** | The framework we use to write Solana programs in Rust. |
| **Bad debt** | When a position closes below zero and the vault is short the difference. |
| **Base currency** | The first one in a pair. In EUR/USD, that's EUR. |
| **B-book** | When a broker takes the opposite side of your trade instead of passing it to the market. |
| **Collateral** | Money you deposit to back your positions. |
| **Confidence interval** | Pyth's own estimate of how uncertain its price is. |
| **Devnet** | Solana's test network. Fake money, real code. |
| **Funding rate** | Small hourly payment between long and short traders to keep the two sides balanced. |
| **Health Factor** | Equity ÷ maintenance margin. Below 1.0 = liquidated. |
| **IB** | Introducing Broker. An affiliate paid per lot their referrals trade. |
| **Insurance fund** | Reserve built from fees that absorbs bad debt before LPs feel it. |
| **Keeper** | A bot that performs liquidations and other upkeep. Anyone can run one. |
| **Leverage** | Controlling a large position with a small deposit. |
| **Liquidation** | Automatic closure of a position that has run out of margin. |
| **Long** | Betting the price goes up. |
| **Lot** | Standard trade size. 1 standard lot = 100,000 units. |
| **LP** | Liquidity Provider. Someone who funds the vault. |
| **Maintenance margin** | The minimum you must keep, or you're liquidated. |
| **Margin** | The money backing your position. |
| **Notional** | The full size of your position, not just your deposit. |
| **Oracle** | Service that brings real-world prices onto a blockchain. |
| **PDA** | Program Derived Address. A Solana account owned by a program rather than a person. |
| **Perpetual (perp)** | A contract tracking a price with no expiry date. |
| **Pip** | Smallest normal price move. Usually the 4th decimal. |
| **Pyth** | The oracle network we use. |
| **Quote currency** | The second one in a pair. In EUR/USD, that's USD. |
| **Short** | Betting the price goes down. |
| **Skew** | How lopsided the long/short balance is. |
| **Slippage** | Getting a worse price than you expected. |
| **Spread** | The gap between buy and sell price. How brokers earn. |
| **Swap / carry** | Overnight interest for holding a position. |
| **Toxic flow** | Traders exploiting stale prices for risk-free profit. |
| **TVL** | Total Value Locked. How much money is in the protocol. |
| **USDC** | A digital dollar. Our collateral currency. |

---

## Three things to remember

1. **Leverage doesn't multiply your profit — it shrinks your cushion.** A 1% move ends a 50x position. That's why we start conservative while GMTrade offers 500x.

2. **The vault is a B-book, and that's fine — as long as we say so.** Our advantage isn't "no conflict of interest." It's that the rules are public, the price isn't ours to touch, and your money stays in your own account. That claim survives scrutiny. The other one doesn't.

3. **Code isn't the hard part. Distribution is.** GMTrade proved the technology works and already has the volume. What nobody has built is a *broker* — for forex traders, with pairs nobody else lists, and a rebate ledger partners can actually audit. That's the whole bet.

---

*Companion to `ARCHITECTURE.md` (the technical specification). If anything here is unclear, that's a bug in this document — say so and it gets fixed.*
