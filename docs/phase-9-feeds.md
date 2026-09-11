# Phase 9 — how many markets can actually be listed

**Measured 2026-09-11 19:43 UTC**, Friday, FX in session. Raw output:
[`devnet-feed-probe-2026-09-11.json`](devnet-feed-probe-2026-09-11.json). Re-run with
`./target/release/devnet-feed-probe --cluster devnet --json` and a working `PYTH_API_KEY`.

Phase 9's exit criteria include **≥20 live markets**. This is the measurement that says whether
that number is reachable at all, and it changes the plan in two places.

## The ceiling is 24, not 33

**24 of 33 planned symbols have a Hermes feed id.** The other 9 have none, so no
amount of poster work lists them — they are not a delivery problem, they do not exist:

| Symbol | Group |
|---|---|
| GBP/JPY | FX crosses |
| CAD/JPY | FX crosses |
| CHF/JPY | FX crosses |
| EUR/AUD | FX crosses |
| EUR/CHF | FX crosses |
| AUD/JPY | FX crosses |
| USD/CLP | LATAM (session-bound) |
| USD/PEN | LATAM (session-bound) |
| USOILSPOT/USD | Commodities |

**This contradicts `ARCHITECTURE.md`'s market table**, which plans 8 FX crosses. Six of them
have no Hermes feed. Only **EUR/JPY** and **EUR/GBP** survive of that group. Oil
(`USOILSPOT/USD`) has no feed either, which retires the Commodities row. Two of the three LATAM
pairs are gone; only USD/BRL remains.

So ≥20 live markets is reachable — with **four symbols of slack, not thirteen.**

## Sponsored devnet feeds are not the path, and this run says so numerically

Every sponsored account that is still being written was **306 seconds old** at probe time, and
the protocol's gate is `MAX_ALLOWED_STALENESS_SECONDS = 60`. A sponsored devnet feed is
therefore never tradeable on SolFX even when it exists and is maintained. Of the 33:

- **8** sponsored accounts alive but ~5 min stale (EUR/USD, GBP/USD, AUD/USD, XAU/USD, XAG/USD,
  BTC/USD, ETH/USD, SOL/USD)
- **8** sponsored accounts abandoned — 34 to 225 days old, including USD/JPY at 127 days
- **8** served by Hermes with no sponsored account at all
- **9** no feed anywhere

**The poster is the only route to a tradeable market on devnet**, for all 24. That was already
the design; this is the number that justifies it.

## What ≥20 markets actually costs

The poster publishes through the five-instruction full-verification path, ~5 transactions per
feed. Six feeds at `--concurrency 2 --max-rps 2` currently take ~35 s a pass. Twenty feeds is
not twenty-over-six times the work — concurrency absorbs much of it — but the binding number is
**worst on-chain age against the 60 s gate**, and the last measurement at scale
(2026-09-01, `--concurrency 8 --max-rps 8`) reached a worst age of 38 s on six feeds.

**Nobody has measured 20 feeds.** That experiment is the real Phase 9 risk and it needs a
window where disrupting the poster is acceptable — not one taken while the venue is up.

The keeper moved off Helius on 2026-09-09, so the poster now has that budget almost to itself,
which is the headroom the experiment would draw on.

## Decision, 2026-09-11: the venue stays at six pairs

**Taken by the user, and it retires the "≥20 live markets" criterion.** A free Pyth tier plus
the poster's throughput will not carry twenty feeds inside a 60-second gate, and 20 feeds is
~100 transactions a pass in devnet SOL and RPC budget for no proportionate gain on a devnet
demo. The six live pairs already span the variants that matter — direct USD-quoted (EUR/USD,
XAU/USD, BTC/USD) and non-USD-quoted (USD/JPY, USD/CNH).

What replaces it is the *point* of the original criterion rather than its number: § 12.5's
extensibility test, run on the live deployment, proving a new market needs no redeploy and that
pre-existing positions are untouched. See [`phase-9-prompt.md`](phase-9-prompt.md) Task 6.

## Two crypto markets are the cheap win

**ETH/USD** and **SOL/USD** have Hermes feeds, are `FeedKind::Crypto`, and trade 24/7 — so
unlike every FX pair they are testable on a weekend, when only BTC/USD is currently open. They
are the obvious next two listings and they exercise the § 12.5 extensibility test on a live
deployment without waiting for Sunday 21:00 UTC.
