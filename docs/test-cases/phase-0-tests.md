# Phase 0 — Test Cases (the probe's measurement criteria)

**Guide:** [`../guides/phase-0-guide.md`](../guides/phase-0-guide.md) ·
**Flowchart:** [`architecture/phase-0.png`](../../architecture/phase-0.png)

Phase 0 has no `#[test]` functions — its "tests" are the **PASS/FAIL criteria the probe
applied to live oracle data**, and the cross-checks that validated the instrument itself.
They are documented here in the same shape as the code catalogues because they gate every
risk parameter downstream: a feed that fails here never reaches `initialize_market`.

**Run:**
```bash
cd scratch/feed-probe
bash run-probe.sh          # start collection (restart-safe)
python3 analyse.py         # verdicts from data collected so far
python3 check-silent.py    # direct staleness check on quiet feeds
```

## Feed acceptance criteria (applied per feed by `analyse.py`)

| # | Criterion | PASS condition | FAIL consequence |
|---:|---|---|---|
| 1 | Feed exists | at least one publish with `publish_time > 0` ever observed | symbol is a catalogue ghost → **never listable** (caught PKR, NGN, EGP, VND, BDT, LKR, THB, MYR) |
| 2 | Confidence, majors/crosses | `conf/price` p95 ≤ ~5 bps | demote to a lower leverage tier |
| 3 | Confidence, EM | p95 ≤ 25 bps | do not list (caught USD/IDR at 30.11, USD/COP at 68.4) |
| 4 | Confidence, metals | p95 ≤ 40 bps | minimum-leverage tier only (XPD/USD at 28.7 passes narrowly) |
| 5 | Credible uncertainty | conf > 0 on a traded market | a feed asserting **zero** uncertainty is a redemption rate, not a market — never a liquidation price (caught XAUM/USD at exactly 0.00 bps) |
| 6 | Cadence | median publish interval ≈ 1 s; **zero** in-session intervals > 60 s | cannot support liquidation → not listable at leverage |
| 7 | Session honesty | silence charged **only during open hours** (Fri 21:00 → Sun 21:00 UTC excluded, local windows respected) | without this, every session-bound feed "fails" every weekend — the analyser's own first defect |
| 8 | Weekend behaviour (Q1) | publishes during Sat/Sun | metals answered **NO** (all 137 Metal/Commodities frozen at Friday close) → weekend product withdrawn, `ContinuousIndex` blocked pending Q1b |

## Instrument self-checks (measuring the measuring stick)

| # | Check | How | What it caught |
|---:|---|---|---|
| 1 | Probe-down vs feed-down | BTC/USD + SOL/USD liveness controls — crypto never closes, so a simultaneous gap across controls dates *probe* downtime | the 41 h WSL suspension that produced the false "LATAM feeds are hourly" verdict |
| 2 | Clock trust | all timing from oracle `publish_time` / `prev_publish_time`, never local `observed_utc` | WSL clock skew wrote rows stamped a day behind their own oracle time |
| 3 | Self-match guard | `pgrep -f '[p]robe\.py'` (bracket trick) in the launcher | the guard matching its own shell and double-starting |
| 4 | Log preservation | launcher **appends** to `probe.log` | start-up truncation destroying the only record of downtime |
| 5 | Same-request control | Saturday sweep queried BTC/ETH/SOL in the *same* Hermes requests as the metals | proves the freeze was the feeds, not our connectivity |

## Findings as regression tests downstream

Each Phase 0 finding became executable in a later phase, so it can never silently regress:

| Finding | Now permanently tested by |
|---|---|
| Dead catalogue symbols | `rejects_an_all_zero_feed_id` (Phase 2) + the Phase 9 listing checklist |
| Metals frozen on Saturday | `a_friday_close_read_on_saturday_is_rejected` (Phase 2) · `the_weekend_stale_price_exploit_is_refused` (Phase 4) |
| No 24/7 metals feed reachable | `continuous_index_markets_are_blocked_pending_q1b` (Phase 2) |
| EM pairs on local hours | `an_em_market_follows_its_own_local_hours` (Phase 4, unit + integration) |
| USD/IDR band too wide | `a_band_wider_than_the_market_ceiling_halts_the_market` (Phase 2/4) — the fixture uses IDR's measured 30.11 bps |
| Confidence ⇒ leverage tiers | `rejects_an_initial_margin_that_contradicts_the_leverage_cap` and the per-market fixtures (EM 20x vs majors 50x) |
