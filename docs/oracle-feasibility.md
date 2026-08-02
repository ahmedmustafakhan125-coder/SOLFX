# Oracle Feasibility — Phase 0b

**Status:** 🔴 **Q1 ANSWERED — NEGATIVE.** Collection started 2026-07-28, running ~7 days.
**Final report due:** 2026-08-04, after the full weekend (Sat 1 – Sun 2 August) is in the window.
**Probe:** `scratch/feed-probe/` — running detached, PID recorded in `probe.log`.

This document gates every risk parameter, leverage tier, and session calendar in
[`ARCHITECTURE.md`](ARCHITECTURE.md). **No protocol code should be written against assumptions
this report has not confirmed.**

---

## 🚨 Headline, 2026-08-01: there is no 24/7 metals feed. The weekend product does not exist as specified.

Measured live against Hermes on **Saturday 2026-08-01, 18:13–18:24 UTC** — the first Saturday in
the observation window, and the measurement this entire phase was built to take.

**All 137 `Metal` and `Commodities` feeds in Pyth's public catalogue are frozen.** Every one last
published at the Friday interbank close and has not ticked since:

| Feed | Last publish (UTC) | Age at measurement |
|---|---|---:|
| XAU/USD | 2026-07-31 21:00:03 | 21.4 h |
| XAG/USD | 2026-07-31 21:00:03 | 21.4 h |
| XPT/USD | 2026-07-31 21:00:03 | 21.4 h |
| XPD/USD | 2026-07-31 21:00:02 | 21.4 h |
| USOILSPOT/USD, UKOILSPOT/USD | 2026-07-31 20:45:03 | 21.7 h |
| …all 137 Metal + Commodities feeds | — | **0 publishing** |

**This is not an outage on our side.** BTC/USD, ETH/USD and SOL/USD were sampled in the same
requests and were publishing at ~1 s with zero age. Hermes is healthy; the metals feeds are shut.

### What this refutes

[`ARCHITECTURE.md`](ARCHITECTURE.md) § 2 C-1 and [`FOREX-EXPLAINED.md`](FOREX-EXPLAINED.md) § 10 both
assert that *"Pyth Indices (10 June 2026) publishes proprietary 24/7 benchmarks for gold and
silver."* Selling point #3 and the one-line positioning statement in § 1 rest on it.

**No such feed is reachable through the public Hermes catalogue.** The catalogue exposes eleven
asset types — `Commodities, Crypto, Crypto Index, Crypto NAV, Crypto Redemption Rate, ECO, Equity,
FX, Kalshi, Metal, Rates`. There is a `Crypto Index` type. There is **no** metals or commodity
index type, and no gold-linked symbol of any type published during the closure.

### Resolved the same day: the product is real, the distribution is the problem

The architecture's claim was **not fabricated.** Pyth Indices launched **10 June 2026** across US
equities, oil and metals, with gold and silver added **24 June 2026** — confirmed by Pyth's own
announcement, Businesswire, The Block and Markets Media. Coinbase, Kraken, dYdX and Nado are named
as integrators. The product exists.

**It is simply not distributed through the public Hermes catalogue.** A systematic sweep of all
3,056 feeds for any symbol or description mentioning gold, silver, platinum, palladium or bullion
returns 29 feeds. During the Saturday closure exactly **4** were live — and every one is a
tokenised-gold crypto instrument, not a metals index:

| Live during closure | Asset type | What it actually is |
|---|---|---|
| PAXG/USD | Crypto | Pax Gold token |
| XAUT/USD | Crypto | Tether Gold token |
| XAUM/USD | Crypto Redemption Rate | Matrixdock issue price |
| XAGM/USD | Crypto Redemption Rate | Matrixdock silver issue price |

The four real spot metals (XAU, XAG, XPT, XPD) were 21.5 h stale. No Pyth-branded metals index
appears in the catalogue under any asset type. For reference `PYTHOIL/USD` **does** exist as a
`Commodities` entry — so proprietary indices are catalogued when published — and it last ticked
**2026-03-30**, four months ago.

**Therefore Q1b is not a technical question but a commercial one:** Pyth Indices requires an
entitlement, a different endpoint, or a paid agreement. That is an email to Pyth, and it is the
single item blocking the weekend product.

> **Until it resolves, plan for no weekend metals.** Do not build `WeekendMode` behaviour, and do
> not staff the 7-day on-call rota that § 11 makes conditional on continuous markets.

### The only 24/7 gold-linked prices that do exist

Sampled live during the same closure, all publishing at ~1 s:

| Symbol | Asset type | Price | conf (bps) | Basis vs XAU Friday close |
|---|---|---:|---:|---:|
| XAUT/USD (Tether Gold) | Crypto | 4035.10 | 16.4–21.0 | **−29.3 bps** |
| PAXG/USD (Paxos Gold) | Crypto | 4037.65 | 9.6–12.9 | **−23.0 bps** |
| XAUM/USD | Crypto Redemption Rate | 4061.90 | **0.00** | **+36.9 bps** |
| XAU/USD (reference) | Metal | 4046.95 | frozen | — |

These are **tokenised claims on gold, not spot gold**, and they do not agree with each other: the
XAUM–XAUT spread is ~66 bps, against a weekday XAU/USD confidence of 1.43 bps p50. Marketing them
as "gold" would be a misrepresentation, and settling liquidations on them is a materially different
risk product requiring its own basis and Monday-convergence analysis.

**XAUM/USD reports confidence of exactly 0.00 bps.** A feed asserting zero uncertainty is not
credible as a liquidation price; it is a redemption rate, not a traded market. Do not build on it.

All three are now under measurement through the Monday reopen so the basis and convergence gap are
quantified rather than assumed.

---

## Headline: the gate already caught something

Phase 0b exists because the strategy rested on unverified assumptions about Pyth's feed
coverage. Within an hour of measurement it found a real one.

**Eight of the emerging-market symbols we planned around are listed in Pyth's catalogue but
have never published a single price.** Their `publish_time` is `0` — Unix epoch. They are
registered symbols with no data behind them:

```
USD/PKR   USD/NGN   USD/EGP   USD/VND
USD/BDT   USD/LKR   USD/THB   USD/MYR
```

Had we built on the catalogue listing alone, these would have shipped as markets that could
never be priced, opened, or liquidated. **A symbol appearing in the catalogue is not evidence
that a feed exists.** Every future market listing must be validated against live data before
`initialize_market` is called — this belongs in the Phase 9 listing checklist.

---

## 1. Pyth catalogue — what actually exists

Retrieved from Hermes `/v2/price_feeds`, 2026-07-28. **3,056 feeds total.**

| Asset type | Feeds |
|---|---:|
| Equity | 1,772 |
| Crypto | 585 |
| **FX** | **290** |
| Crypto Redemption Rate | 165 |
| Commodities | 126 |
| ECO | 44 |
| Rates | 26 |
| Kalshi | 19 |
| **Metal** | **11** |
| Crypto NAV | 10 |
| Crypto Index | 8 |

### FX composition (290 feeds)

| Shape | Count | Note |
|---|---:|---|
| `XXX/USD` | 4 | AUD, EUR, GBP, NZD only |
| `USD/XXX` | 106 | The standard quoting convention for everything else |
| Crosses (`EUR/XXX`, `XXX/JPY`, …) | ~180 | **Carried natively — see § 4** |

### Metals (asset_type = `Metal`)

| Symbol | Feed ID (prefix) |
|---|---|
| XAU/USD | `765d2ba906dbc32c…` |
| XAG/USD | `f2fb02c32b055c80…` |
| XPT/USD | `398e4bbc7cbf89d6…` |
| XPD/USD | `80367e9664197f37…` |

> ✅ **RESOLVED 2026-08-01 — they are session-bound spot.** This note correctly identified that
> metadata could not settle the question and that only a Saturday observation would. It was taken,
> and all four metals feeds were frozen at the Friday close. See the headline.

---

## 2. Measurements so far

16.6h window, dense sampling from 2026-07-28 10:33 UTC. Confidence in basis points of price;
cadence from Hermes' own `prev_publish_time`.

### Metals

| Symbol | conf p50 | conf p95 | Cadence | Verdict |
|---|---:|---:|---:|---|
| **XAU/USD** | **1.80** | **2.84** | 1.0s | ✅ PASS — best-in-class |
| XAG/USD | 6.09 | 7.45 | 1.0s | ✅ PASS |
| XPT/USD | 15.13 | 18.11 | 1.0s | ✅ PASS (within 40bps metals limit) |
| XPD/USD | 16.66 | 28.66 | 1.0s | ⚠️ PASS but wide — low leverage only |

### FX majors

| Symbol | conf p50 | conf p95 | Cadence | Verdict |
|---|---:|---:|---:|---|
| USD/JPY | 0.85 | 2.07 | 1.0s | ✅ PASS |
| EUR/USD | 1.28 | 4.08 | 1.0s | ✅ PASS |
| GBP/USD | 1.36 | 4.60 | 1.0s | ✅ PASS |
| USD/CAD | 1.70 | 5.11 | 1.0s | ✅ PASS |
| AUD/USD | 1.87 | 4.41 | 1.0s | ✅ PASS |
| USD/CHF | 4.82 | 9.79 | 1.0s | ✅ PASS |
| NZD/USD | 5.72 | 7.66 | 1.0s | ✅ PASS — widest major |

### Crosses — **tighter than the majors**

| Symbol | conf p50 | conf p95 | Cadence | Verdict |
|---|---:|---:|---:|---|
| EUR/JPY | **0.59** | 0.75 | 1.0s | ✅ PASS |
| GBP/JPY | 0.67 | 0.92 | 1.0s | ✅ PASS |
| CAD/JPY | 0.69 | 1.07 | 1.0s | ✅ PASS |
| CHF/JPY | 0.75 | 1.00 | 1.0s | ✅ PASS |
| EUR/GBP | 0.82 | 1.40 | 1.0s | ✅ PASS |
| EUR/AUD | 0.86 | 1.17 | 1.0s | ✅ PASS |
| EUR/CHF | 0.86 | 1.24 | 1.0s | ✅ PASS |
| AUD/JPY | 0.88 | 1.40 | 1.0s | ✅ PASS |

### Emerging markets — three distinct tiers

| Symbol | conf p50 | conf p95 | Cadence | Verdict |
|---|---:|---:|---:|---|
| USD/MXN | 1.81 | 2.53 | 1.0s | ✅ PASS |
| USD/ZAR | 4.35 | 5.49 | 1.0s | ✅ PASS |
| USD/PHP | 4.69 | 9.62 | 1.0s | ✅ PASS |
| USD/INR | 5.86 | 6.02 | 1.0s | ✅ PASS |
| USD/TRY | 6.05 | 7.16 | 1.0s | ✅ PASS |
| USD/TWD | 6.39 | 9.63 | 1.0s | ✅ PASS |
| USD/KRW | 6.34 | 11.11 | 1.0s | ✅ PASS |
| USD/IDR | 19.50 | **30.11** | 1.0s | ❌ **FAIL** — p95 exceeds the 25bps limit |
| USD/BRL | 19.69 | — | hourly | ❌ **FAIL** — session-bound, ~1 update/hour |
| USD/COP | 10.45 | — | hourly | ❌ **FAIL** — session-bound |
| USD/CLP | 4.21 | — | hourly | ❌ **FAIL** — session-bound |
| USD/PEN | 5.03 | — | hourly | ❌ **FAIL** — session-bound |
| USD/PKR, USD/NGN, USD/EGP, USD/VND, USD/BDT, USD/LKR, USD/THB, USD/MYR | — | — | never | ❌ **DEAD** |

> ### ⚠️ Correction, 2026-08-01: the LATAM feeds are **not** hourly. Q3 is answered.
>
> The interim reading below — "session-bound, ~1 update/hour", "not listable at leverage" — was an
> artefact of a 16.6 h window that happened to catch only the closing tick of each local session.
> Measured across the full dataset, from Pyth's own `prev_publish_time`:
>
> | Symbol | Intervals sampled | Median interval | Intervals > 60 s | Active window (UTC) |
> |---|---:|---:|---:|---|
> | USD/BRL | 3,769 | **1 s** | **0** | 14–21 |
> | USD/COP | 3,723 | **1 s** | **0** | 14–18 |
> | USD/CLP | 3,769 | **1 s** | **0** | 14–20 |
> | USD/PEN | 3,768 | **1 s** | **0** | 14–19 |
>
> **Not one interval above 60 seconds in over 15,000 samples.** These feeds publish at 1 s like
> every other live feed; they simply keep a short local session. The liquidation-engine objection
> does not apply in-session, and USD/BRL, USD/CLP and USD/PEN return to the listable set behind a
> per-market session calendar (§ 7.3). USD/COP stays out — on confidence (68.4 bps p95), not cadence.
>
> **Method note.** The original figure came from `max_gap`, which measures *silence we observed*,
> not *silence the feed had*. With the probe down, the two are indistinguishable. That is now fixed —
> see "Instrument corrections" below.

**Superseded interim reading:** during the first observation window USD/BRL last published at
21:00:03 UTC, USD/COP at 18:00:03, USD/CLP at 20:00:04, USD/PEN at 19:00:03 — all landing exactly
on the hour, then silent for 13–16 hours.

---

## 3. Provisional listing decision

> **Revised 2026-08-01.** Metals move to weekday-only; the LATAM trio returns behind a session
> calendar; USD/COP is re-failed on confidence rather than cadence. Listable count rises from ~23
> to **29** — but the *weekend* product is gone, and that was the differentiator, not the count.

| Tier | Markets | Basis |
|---|---|---|
| **List, weekday only** | XAU/USD, XAG/USD | ~~Pending the weekend result~~ — **Q1 answered: these feeds close Fri 21:00 UTC.** Still excellent in-session (XAU 1.43 bps p50, best of any feed measured); they simply are not 24/7 |
| **List, session calendar** | USD/BRL, USD/CLP, USD/PEN | 1 s cadence in-session; short local windows (see Q3 correction) |
| **List** | EUR/USD, GBP/USD, USD/JPY, USD/CAD, AUD/USD | conf p95 ≤ 5.2 bps |
| **List** | EUR/JPY, GBP/JPY, CAD/JPY, CHF/JPY, EUR/GBP, EUR/AUD, EUR/CHF, AUD/JPY | Tightest of all |
| **List, reduced leverage** | USD/CHF, NZD/USD, XPT/USD | conf p95 7.7–18.1 bps |
| **List, EM leverage (10–20x)** | USD/MXN, USD/ZAR, USD/PHP, USD/INR, USD/TRY, USD/TWD, USD/KRW | conf p95 2.5–11.1 bps |
| **List, minimum leverage** | XPD/USD | conf p95 28.7 bps |
| **Do not list** | USD/IDR | Confidence exceeds limit (32.1 bps p95) |
| **Do not list** | USD/COP | Confidence 68.4 bps p95 — ~~cadence~~ **corrected: cadence is fine, confidence is not** |
| **Do not list** | USD/PKR, USD/NGN, USD/EGP, USD/VND, USD/BDT, USD/LKR, USD/THB, USD/MYR | No feed exists |

**Count: 29 listable markets** (revised 2026-08-01 from ~23) — comfortably past the ≥20 exit
criterion for Phase 9. **The exit criterion is met on breadth; it is the weekend differentiator,
not the market count, that Q1 removed.**

---

## 4. Finding that simplifies the build: crosses are native

`ARCHITECTURE.md` § 3.5 specified synthetic cross composition (`EUR/GBP = EUR/USD ÷ GBP/USD`) to
reach Exness-level coverage, accepting double oracle reads and compounding confidence as the cost.

**That machinery is not needed for v1.** Pyth carries ~180 cross pairs as direct feeds, and they
measure *tighter* than the majors — EUR/JPY at 0.59 bps p50 versus EUR/USD at 1.28 bps.
Composing EUR/JPY from two feeds would produce a **worse** price than reading the one Pyth
already publishes, while doubling the failure surface.

**Consequences:**
- `price_source: Synthetic` moves off the Phase 2/3 critical path. Keep the enum variant — it costs
  nothing to reserve and is needed later for pairs Pyth lacks — but v1 lists direct feeds only.
- Phase 3's exit criteria lose the "synthetic cross" case and keep the non-USD-quoted case.
- The § 3.5 rollout tiers collapse: Exness-parity coverage is reachable with direct feeds alone.

**`quote_conversion_feed` remains mandatory.** Nearly every listable market is `USD/XXX` or a
`XXX/JPY` cross, so PnL lands in the quote currency, not USDC. Correction C-3 stands unchanged and
is still the highest-consequence arithmetic detail in the protocol.

---

## 5. Still open

| # | Question | Status |
|---|---|---|
| **Q1** | **Do XAU/USD and XAG/USD publish on Saturday and Sunday?** | ✅ **ANSWERED — NO.** 0 of 137 Metal/Commodities feeds published during the Saturday closure. See headline. |
| Q3 | Are the LATAM feeds genuinely hourly, or continuous during a local session? | ✅ **ANSWERED — continuous in-session at 1 s.** The "hourly" reading was a measurement artefact. |
| **Q1b** | **How is Pyth Indices for metals actually distributed?** Confirmed to exist (launched 10 Jun 2026, metals 24 Jun) but absent from the public Hermes catalogue. Only route by which weekend metals survives. | 🔴 **OPEN — highest-priority item. Commercial, not technical: email Pyth about entitlement/endpoint/pricing.** |
| Q2 | Exact session boundaries per market, especially each EM pair's local calendar (§ 7.3 requires one calendar per market). | Partial — LATAM windows bounded above (BRL 14–21, COP 14–18, CLP 14–20, PEN 14–19 UTC); needs a clean 7-day run to fix edges |
| **Q4** | Gap size across the Friday-close / Sunday-open boundary — sizes the `GapWindow` and MMR buffer. | 🟡 **In progress — measuring now.** Sunday ~21:00 UTC reopen is the capture. |
| Q4b | Basis and Monday-convergence gap for XAUT/PAXG vs XAU/USD. Decides whether a tokenised-gold weekend market is viable at all. | 🟡 In progress — feeds added 2026-08-01 18:21 UTC |
| Q5 | Publisher count per feed. Not exposed by Hermes; needs a Solana RPC read of the price account. | Open — separate one-off task |
| Q6 | Does GMTrade halt metals at weekends? | Open — **check today**, while the closure is live |

**Q1 is answered and the answer is negative.** The positioning falls back to EM + brokerage UX
unless Q1b rescues it. Everything else is a parameter.

**Q6 is worth doing today specifically.** If GMTrade is quoting gold right now, during a closure in
which every Pyth metals feed is frozen, they are sourcing it somewhere we have not found — and that
source is the answer to Q1b.

---

## Instrument corrections, 2026-08-01

The probe and analyser had three defects, all of which biased the report. Fixed:

**1. A scheduled market closure scored as feed failure.** `analyse.py` failed any feed silent >4 h.
The moment Saturday entered the window every market read `FAIL` — the summary line was
`FAIL=31`, i.e. the entire protocol looked infeasible. Gaps are now charged only for the portion
falling in open hours (`weekend_closed()` / `open_seconds()`, Fri 21:00 → Sun 21:00 UTC). Corrected
summary: **`PASS=32  FAIL=2  LIVE=2`**, the two genuine failures being USD/COP and USD/IDR on
confidence.

**2. Probe downtime was indistinguishable from feed outage.** `max_gap` measured silence *we
observed*. When WSL suspended on 29–31 July it produced a 41.4 h hole that was charged to all 31
feeds as if the feeds had stopped — the same error that produced the bogus "hourly LATAM" verdict.
**BTC/USD and SOL/USD are now collected as liveness controls.** Crypto never closes, so a
simultaneous gap across the controls dates probe downtime precisely; gaps predating control
coverage are reported as unattributable rather than blamed on the feed.

**3. `observed_utc` is not trustworthy.** WSL clock skew across suspend wrote rows on 30 July
stamped a full day *behind* their own oracle `publish_time`. Confidence and cadence are unaffected —
both derive from oracle timestamps and Hermes' `prev_publish_time`, never from local time — but
`observed_utc` and the daily CSV partitioning must not be used for timing analysis.

Two latent bugs in `run-probe.sh` were also fixed: `pgrep -f "probe\.py"` matched the shell running
it (self-match, so the guard could see itself — use `'[p]robe\.py'`), and the launcher truncated
`probe.log` on every start, destroying the only record of when collection was down. It now appends.

---

## Reproducing

```bash
cd ./scratch/feed-probe

bash run-probe.sh          # start detached (safe to re-run; won't double-start)
python3 analyse.py         # report from data collected so far
python3 analyse.py --markdown
python3 check-silent.py    # staleness check on non-reporting feeds
tail -f probe.log
pkill -f '[p]robe\.py'     # stop
```

The probe appends to daily CSVs in `data/`, so a reboot mid-run loses nothing — restart it and
collection continues.

**One caveat:** it runs inside WSL. If Windows is shut down or WSL is stopped, the probe dies.
Check it is still alive with `pgrep -f '[p]robe\.py'` before relying on a date range, and restart if
needed — a gap in the data is visible in the analysis as an inflated max gap.
