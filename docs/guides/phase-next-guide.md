# What Remains — Phases 7–9 and the Commercial Track

**Flowchart:** [`../diagrams/phase-next.png`](../diagrams/phase-next.png)
**Where we stand:** Phases 0–6 complete. 454 tests, two programs (~905 KB + 257 KB), all
invariants (I1, I2, I4–I8) asserted after every instruction, the CHF-depeg replay passing.
Everything below is specified in `ARCHITECTURE.md` § 15 with **testable exit criteria** —
a phase finishes when its criteria pass, not when time runs out.

---

*(Phase 6 is complete — see [`phase-6-guide.md`](phase-6-guide.md).)*

## Phase 7 — Keepers, ~3 weeks

**What:** the off-chain machinery without which a perp protocol does not liquidate.
Liquidator (Rust: state from Geyser, prices from Hermes SSE, sorted danger list, pre-built
transactions, aggressive priority fees — < 2 s from HF < 1 to landed), trigger executor
**plus the on-chain TP/SL trigger orders** (deferred from Phase 4 precisely so orders never
exist before something can fire them), funding/session crankers, three independent
instances, monitoring.

**Also settles the last oracle unknown:** transaction *size* with real Hermes payloads —
compute is measured (~12k CU/feed) but the 1232-byte limit is not; the fallback (post
updates in a preceding transaction, rely on `max_staleness_seconds`) is already designed.

**Exit criteria:** sub-2-second liquidations under load; survives an instance being killed.
**Risk register R13:** recruit a second engineer before this phase — keeper operations need
cover.

## Phase 8 — Frontend + SDK, ~5 weeks

**What:** a broker terminal, not a perps UI (§ 10.2): size in lots, P&L in pips *and*
dollars; the all-in cost (spread + fee + est. carry) shown **before** submit; the
liquidation price live; the swap-rate table with the interest differential and the SolFX
markup as separate line items (the transparency pillar — the fields have been separate
on-chain since Phase 4); per-market regime clock; confidence-driven spread visible as it
widens. TypeScript SDK generated from the IDL; indexer consuming the event stream (which is
why every mutation has emitted an event since Phase 2). Plus the deferred admin
instructions for breaker thresholds and interest rates — deferred *to* here because tuning
belongs with the surface that displays it.

**User-added requirements (2026-08-12):**

- **Design system supplied by the owner** — colour combinations and layout will be
  provided before build; the terminal is skinned to that spec rather than a default theme.
- **TradingView integration** — the § 10 stack already plans TradingView Lightweight
  Charts; the price relayer (Hermes SSE cache) doubles as the chart datafeed, so candles,
  the live confidence-driven spread, and the trader's entry/liquidation lines can all be
  drawn on the same chart. If the full TradingView Charting Library is wanted instead
  (drawing tools, indicators), it drops into the same datafeed interface.
- **Admin analytics panel (admin wallet only):**
  - leaderboards: most profitable traders and single trades (realised PnL from
    `PositionClosed` / `PositionLiquidated` events)
  - volume per day / per market, fees per stream, LP NAV history, insurance-fund level
  - **live terminal**: a real-time feed of protocol activity — deposits, opens, closes,
    liquidations as they land — streamed over websocket from the indexer
  - Access control is wallet-signature auth on the BFF (admin pubkey allowlist). Honest
    caveat, stated here so it is never oversold: **every trade is already public on-chain**
    (traders are pseudonymous wallet addresses). The admin gate restricts the convenient
    aggregated view, not the underlying data — anyone could rebuild it from the event
    stream, which is by design (§ 8.5's auditable-ledger pitch depends on it).

  No new on-chain work is needed for any of this: every mutation has emitted a typed event
  since Phase 2 precisely so the indexer can reconstruct the whole ledger. The panel is an
  indexer query surface plus a websocket.

**Exit criteria:** an end-to-end browser trade on devnet.

## Phase 9 — Expansion + hardening, ~4 weeks

**What:** list Tiers 2–3 (~15 more markets) **as configuration only** — the § 5.6 claim
proven on a *running* devnet deployment with the binary hash checked unchanged (§ 12.5,
step 5: one market per price/quote shape). The listing checklist operationalises Phase 0b's
hardest lesson: every feed validated against live data before `initialize_market`, because
a catalogue entry is not a feed. Trident fuzzing (24 h clean), >90% line coverage, the
per-user concentration cap and single-block-loss breaker (both need state or baselines no
current account carries), load tests at 1,000 concurrent positions.

**Exit criteria:** ≥ 20 live markets · all scenarios pass · § 12.5 passes · 30 days with
zero fund-loss bugs.

---

## The capstone line, then the commercial track (gated)

Phases 0–9 = the complete, publicly deployed devnet protocol — the capstone. Everything
after is **gated on traction or grant funding**, not started speculatively:

| Step | Cost | Note |
|---|---|---|
| Grant applications | — | **Apply now, during Phase 7, not at the end** — a working risk engine that survives a CHF-depeg replay is a stronger application than a finished protocol asking for audit money. Targets: Solana Foundation, Pyth (SolFX is a flagship consumer of their EM feeds), Colosseum |
| Audit #1 (core) | $40–80k | before public devnet marketing |
| Audit #2 (full, different firm) | $60–150k | before mainnet |
| Legal opinion | $5–20k | per-jurisdiction (R9 — the EM strategy *raises* regulatory risk; India/Indonesia/Korea analysis before the fee switch, not after) |
| Keys & governance | — | Squads 3-of-5 multisig, 48 h parameter timelock, 7-day upgrade timelock, guardian stays pause-only |
| Insurance seeding | 2% of max OI | never launch it empty — the replay shows exactly why |
| Guarded mainnet | — | TVL caps $250k → $1M → $5M |

## Open questions carried forward

| # | Question | State |
|---|---|---|
| **Q1b** | Pyth Indices entitlement — the only route weekend metals returns. One email to Pyth; the continuous regime is built, tested, and blocked at listing until it resolves | 🔴 open, commercial |
| Q2 | Exact EM session boundaries — a clean 7-day probe run before those markets list | 🟡 partial |
| Q4 | Friday-close → Sunday-open gap size — tunes `GAP_WINDOW_SECONDS` and MMR buffers (constants, no code change) | 🟡 measuring |
| — | Transaction size with 3 real Pyth payloads | 🟡 Phase 7 |
| R13 | Second engineer before Phase 7 | 🔴 open |
