# Phase 0 Guide — Explainer & Oracle Feasibility

**Flowchart:** [`architecture/phase-0.png`](../../architecture/phase-0.png)
**Deliverables:** `docs/FOREX-EXPLAINED.md` (0a) · `scratch/feed-probe/` + `docs/oracle-feasibility.md` (0b)
**Exit criteria:** a reader with no FX background can evaluate the trade-offs; the listable-market set is decided from live data; go/no-go on the EM and weekend theses.
**Result:** both met. The weekend thesis got a **no** — which is the phase working, not failing.

---

## 1. What this phase is, and why it exists

Phase 0 is the rule *measure before you build* applied to the whole project. The v0 proposal
rested on three assumptions nobody had checked against a live feed:

1. **"Pyth Indices publishes 24/7 gold and silver"** → the entire weekend-trading product,
   a two-regime state machine, four weekend risk fields, and a 7-day on-call requirement.
2. **"A symbol in Pyth's catalogue means the feed exists"** → the EM market list.
3. **"Crosses must be synthesised from two legs"** → double oracle reads, compounded
   confidence, wider spreads on ~180 pairs.

Every one of those was wrong or backwards, and each error would have been *built into the
protocol* before anyone noticed. Phase 0 cost one week of measurement; retrofitting any one
of those mistakes after Phase 4 would have cost months.

**Where it is used later:** every leverage tier (§ Phase 1 margin), every session calendar
(Phase 2 `Market.session_*`), every oracle guard ceiling (`max_conf_bps`,
`max_staleness_seconds`), the `initialize_market` validation rules, and the Phase 9 listing
checklist all take their numbers from this phase's output. Nothing downstream invents a risk
parameter; it inherits a measurement.

---

## 2. Phase 0a — FOREX-EXPLAINED.md

A plain-English companion to the architecture, written **before** code so that every design
decision could be evaluated by a reader with no FX or DeFi background. It fixed seven
corrections (C-1 … C-7) into the architecture, of which three carry financial logic used
everywhere later:

| Correction | What it says | Where it lands in code |
|---|---|---|
| **C-1** stale-price exploit | Trading against a frozen Friday price is free money: see a weekend gap coming, open risk-free, the vault pays the whole gap | The session state machines (Phase 4), the two-sided freshness gate (Phase 2), `GapWindow` |
| **C-3** quote-currency PnL | `uPnL = size × Δprice` lands in the **quote currency**. For USD/INR that is Rupees — booking it as USDC mis-prices by ×88 | `QuoteConversion` type (Phase 1), `quote_conversion_feed` on `Market`, every PnL path |
| **C-4** confidence is not optional | At 200x, one pip is 1.84% of margin; Pyth's uncertainty band alone is worth 1–4% of a max-leverage trader's margin. Quote the mid and latency arbitrageurs harvest the band | Adverse-side `execution_price`, `conf_spread`, the hard confidence reject (Phases 1–3) |

---

## 3. Phase 0b — the probe, function by function

Everything lives in `scratch/feed-probe/`. It is deliberately throwaway code — its output
(`docs/oracle-feasibility.md`) is the durable artifact.

### `feeds.txt`
The candidate list: 34 feeds spanning majors, crosses, EM pairs, metals — **plus BTC/USD and
SOL/USD as liveness controls** (see the instrument correction below). Why it matters: the
probe can only answer questions about feeds it watches, so the list was drawn from every
market the strategy might list, not just the launch set.

### `probe.py` — the collector
Polls Pyth's Hermes API (`/v2/updates/price/latest`) every second for every feed in
`feeds.txt` and appends one CSV row per feed per poll: `observed_utc, symbol, price, conf,
expo, publish_time, prev_publish_time`.

Design points that mattered:
- **Appends to daily CSVs** — a restart after a crash loses nothing.
- **Records `prev_publish_time`** — Hermes' own statement of the feed's last update, which
  makes cadence measurable *from the oracle's clock*, immune to gaps in our own polling.
- **Detached, restartable** (`run-probe.sh` guards against double-starts with
  `pgrep -f '[p]robe\.py'` — the brackets stop the pattern matching its own shell).

### `analyse.py` — the report
Reduces the CSVs to per-feed verdicts:

- **Confidence percentiles** `p50 / p95 / p99` of `conf/price` in basis points. This is the
  single number that sizes everything: a feed's uncertainty band is a floor under the spread
  we must charge (C-4), and therefore a ceiling on the leverage we can offer.
- **Cadence** — median interval between `prev_publish_time` values, and the count of
  intervals over 60 s. A feed that pauses cannot support liquidation during the pause.
- **Session windows** — first/last publish per UTC day, which is how the LATAM local
  sessions (BRL 14–21 UTC, COP 14–18, CLP 14–20, PEN 14–19) were discovered.
- **PASS/FAIL verdicts** against explicit limits (see the test-case catalogue): conf p95
  within the class limit, no in-session silence, gap-charging only during open hours.

### `check-silent.py`
For feeds reporting nothing: asks Hermes directly for their latest update and its age.
This is the tool that produced the headline finding — on Saturday 2026-08-01 it swept **all
3,056 catalogue feeds** and found every one of the 137 Metal/Commodities feeds frozen at the
Friday 21:00 UTC close while BTC/ETH/SOL in the same requests were current to the second.

---

## 4. The financial logic this phase produced

### Confidence ⇒ leverage (the tiering formula)

The reasoning chain, which every later margin number inherits:

```
A trader's margin  =  notional / leverage
One pip of EUR/USD ≈ 0.92 bps of price
At leverage L, a move of X bps costs the trader  L·X bps of their margin.

Pyth's confidence band is uncertainty we must not hand to arbitrageurs (C-4),
so the effective spread ≥ conf.  For the spread to stay a small fraction of
margin (rule of thumb ≤ ~5%):     L ≤ 500 / conf_p95_bps    (approximately)
```

Applied to the measurements:

| conf p95 (bps) | Example feeds | Max leverage granted |
|---:|---|---:|
| ≤ 2.5 | XAU/USD 2.84 · EUR/USD 4.08* · crosses 0.75–1.4 | 50x |
| 2.5–5.2 | GBP/USD, USD/JPY, USD/CAD, AUD/USD | 30–50x |
| 5–11 | USD/CHF, NZD/USD, EM: MXN ZAR PHP INR TRY TWD KRW | 10–20x |
| 18–29 | XPT/USD, XPD/USD | minimum leverage |
| > 30 | USD/IDR (30.11), USD/COP (68.4) | **do not list** |

*p50 values place EUR/USD in tier 1; the p95 tail is what the `conf_spread` absorbs.

### The catalogue-≠-feed rule

Eight EM symbols (PKR, NGN, EGP, VND, BDT, LKR, THB, MYR) exist in the catalogue with
`publish_time = 0` — registered names that have **never published a price**. A market
pointed at one could be created but never priced, opened, or liquidated. Consequences in
code: `require_feed_id_set` at `initialize_market` (the on-chain half), and a mandatory
live-data check in the Phase 9 listing checklist (the half that cannot be done on-chain).

### The weekend finding (Q1 answered NO, Q1b open)

The Pyth Indices *product* is real (launched 10 June 2026) but is **not distributed through
the public Hermes catalogue** — no metals index exists under any asset type. So a Solana
program cannot read it, and the weekend product cannot ship. The machinery
(`FeedKind::ContinuousIndex`, `WeekendMode`, `PreOpenWindow`) was still built and tested in
Phase 4, because if the commercial question Q1b resolves, turning it on is a **config
change, not a code change** — but `initialize_market` rejects the variant until then, so
nothing can be listed against a promise the feed does not keep.

### The instrument corrections (why the probe itself needed debugging)

Three defects in the probe biased early findings, and each is a lesson the later phases
reuse:

1. **A scheduled closure scored as feed failure** — the analyser charged weekend silence
   against the feeds, flagging the whole protocol infeasible. Fix: charge gaps only during
   open hours. *(Lesson: a control must understand the calendar it polices — reappears in
   the session state machine.)*
2. **Probe downtime was indistinguishable from feed outage** — a 41-hour WSL suspension
   produced the false "LATAM feeds are hourly" verdict. Fix: BTC/SOL as liveness controls;
   crypto never closes, so a gap in the controls dates *our* downtime. *(Lesson: measure
   the instrument as well as the subject.)*
3. **Local timestamps are untrustworthy across suspends** — all timing derives from oracle
   timestamps, never `observed_utc`.

---

## 5. Where every downstream phase consumes this

| Phase 0 output | Consumed by |
|---|---|
| conf p50/p95 per feed | Phase 1 margin tiers; Phase 2 `max_conf_bps` per market; Phase 3 `conf_spread` |
| cadence = 1 s for all live feeds | Phase 2 `max_staleness_seconds = 10` (60 s protocol ceiling) |
| session windows | Phase 2 `session_*` fields; Phase 4 calendar signal of `crank_market_session` |
| dead-feed list | `require_feed_id_set`; Phase 9 listing checklist |
| crosses-are-native finding | synthetic path moved off the v1 critical path (kept, tested, unused) |
| Saturday metals result | `ContinuousIndex` blocked at listing; weekend clause struck from positioning |

**Test cases for this phase:** [`../test-cases/phase-0-tests.md`](../test-cases/phase-0-tests.md)
