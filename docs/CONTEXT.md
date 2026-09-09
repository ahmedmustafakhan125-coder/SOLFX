# SolFX — Complete Context and Build Status

**Last updated:** 2026-09-07
**Status:** Phases 1–7 complete. **Phase 8 (frontend + SDK) substantially built.**
555 Rust tests + 128 SDK tests passing, clippy clean.

**Live on devnet.** Program `2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi`, 9 markets listed
(6 active), on-chain IDL current at 38 instructions. Real trades open and close from the
browser with Phantom.

**The off-chain half runs 24/7 on the VPS.** Poster, keeper, gateway and web tier are all up
and all `enable`d at boot as of 2026-09-09; six markets are `Active` and the keeper is
cranking, liquidating and executing triggers. See *Deploying to a VPS* and *What unblocked
it* below. Next: the test-suite honesty pass, then Phase 9. NOXFUNDING still starts only
after all of that.

> **Session log:** [`CLAUDE-SESSION.md`](../CLAUDE-SESSION.md) at the repo root records what
> changed on 2026-09-09 and when, including the things that are true but not yet fixed. Read it
> if you are picking this up cold; this document is what *is*, that one is what *changed*.

> **Deadline: 11 Sep 2026 — the Pyth API key trial expires.** Every Hermes endpoint now
> requires a key (measured, see *Pyth access*), so there is no free endpoint to fall back to.
> The key and the endpoint are both configuration, not code, so swapping either is a one-line
> change plus a restart.

---

## What SolFX Is

**SolFX is a non-custodial forex broker on Solana.** The protocol provides:

- **Forex trading** with lot-based sizing (micro, mini, standard lots) and pip-denominated P&L
- **Collateral custody** — trader's USDC never leaves their own account. The protocol holds only LP capital
- **Oracle-verified pricing** from Pyth (no reliance on centralized price feeds)
- **Permissionless liquidation** — anyone can liquidate an undercollateralized position and earn a reward
- **Verifiable partner rebates** — brokers/referrers earn transparent, on-chain rebates recorded in events

The venue is a **B-book** model: the LP pool is the counterparty to every trade. Traders profit from the pool; the pool earns spread, fees, and the losses of unprofitable traders.

---

## Market Coverage (29 + 4 = 33 markets)

### Phase 0b confirmed (29 markets, weekday/session-bound)

| Category | Pairs | Count |
|---|---|---|
| **FX Majors** | EUR/USD, GBP/USD, USD/JPY, USD/CAD, AUD/USD | 5 |
| **FX Crosses** | EUR/JPY, GBP/JPY, CAD/JPY, CHF/JPY, EUR/GBP, EUR/AUD, EUR/CHF, AUD/JPY | 8 |
| **Precious Metals** | XAU/USD (gold), XAG/USD (silver), XPT/USD (platinum), XPD/USD (palladium) | 4 |
| **Emerging Markets** | USD/MXN, USD/ZAR, USD/PHP, USD/INR, USD/TRY, USD/TWD, USD/KRW | 7 |
| **LATAM (session-bound)** | USD/BRL, USD/CLP, USD/PEN (1 s cadence in-session, 14–21 UTC windows) | 3 |
| **Reduced leverage** | USD/CHF, NZD/USD | 2 |
| **Minimum leverage** | XPD/USD (28.7 bps confidence p95) | 1 |

**Metals:** Gold (XAU) and silver (XAG) are session-bound (close Friday 21:00 UTC). Pyth Indices for 24/7 metals do not exist in the public catalogue (Q1b is unresolved).

### Cryptocurrency pairs (planned for v1 launch)

| Pair | Basis |
|---|---|
| **BTC/USD** | Major crypto; 24/7 feed |
| **ETH/USD** | Major crypto; 24/7 feed |
| **SOL/USD** | Native Solana; 24/7 feed |

All three publish at ~1s with tight confidence (~1–3 bps).

### Commodities (crude oil)

| Pair | Feed | Basis |
|---|---|---|
| **USOILSPOT/USD** | WTI crude (US); major trading pair | Session-bound (open 17:00–16:00 UTC); 1 s cadence |
| (optional) **UKOILSPOT/USD** | Brent crude (UK); lower volatility | Session-bound; secondary commodity |

**Note:** Oil feeds close Friday ~20:45 UTC. Weekend trading is not possible. Session calendar required (implemented in Phase 7, per-market `feed_kind` state machine).

**Total implemented on launch: 33 markets** (29 FX/Metals + 3 Crypto + 1 Commodity).

---

## Phases 1–7: What Was Built

> **Note on phase-to-file mapping:** the test suite is organised **by subject, not by phase**,
> so there is no per-phase test count. The files below are the real ones.

| Phase | Subject | Where the tests live |
|---|---|---|
| 1 | Protocol init, markets, collateral | `protocol.rs`, `markets.rs`, `collateral.rs` |
| 2 | Pyth oracle, staleness, confidence, synthetic + converted feeds | `oracle.rs` |
| 3 | Margin, health factor, liquidation | `risk_engine.rs` |
| 4 | Referral rebates | `referral.rs` |
| 5 | Position lifecycle, triggers, partial closes | `positions.rs`, `triggers.rs` |
| 6 | LP pool, session calendars | `liquidity.rs` |
| 7 | Keeper (liquidator, triggers, cranks) | `crates/solfx-keeper/src/` + `scenarios.rs` |

## Test Coverage

**490 passed, 0 failed, 0 ignored** — `cargo test --workspace`, run 2026-08-23, exit 0.
(A static grep counts 488; it misses two macro-generated tests. The run is authoritative.)

### Integration tests — `programs/solfx-core/tests/` (203)

| File | Tests |
|---|---:|
| `markets.rs` | 32 |
| `positions.rs` | 26 |
| `oracle.rs` | 26 |
| `risk_engine.rs` | 23 |
| `referral.rs` | 20 |
| `liquidity.rs` | 20 |
| `triggers.rs` | 14 |
| `collateral.rs` | 14 |
| `protocol.rs` | 13 |
| `compute_budget.rs` | 8 |
| `scenarios.rs` | 7 |

### Math crate — `crates/solfx-math/` (248)

Unit tests live beside the code (`oracle.rs` 33, `funding.rs` 23, `pricing.rs` 24,
`margin.rs` 22, `pnl.rs` 19, `fees.rs` 17, `fixed.rs` 14, `lp.rs` 13, `referral.rs` 9,
others 8) plus `tests/properties.rs` (56 property tests) and `tests/broker_parity.rs` (9).

### Keeper — `crates/solfx-keeper/` (11)

`services.rs` 6, `pyth.rs` 5.

**Framework:** LiteSVM.

### Invariants I1–I8 — how they are actually enforced

`assert_invariants()` is a **test harness**, defined at
`programs/solfx-core/tests/common/mod.rs:2241`. It does **not** run on chain — there are zero
references to it in `programs/solfx-core/src/`.

That is the correct design (a runtime invariant sweep would burn compute on every
instruction), but it means the invariants are guaranteed **by test coverage, not by runtime
assertion**. Any new instruction must call it in its tests or the guarantee does not extend to
that path.

### Compute budget — measured, asserted in CI

From [`docs/compute-budget.md`](compute-budget.md), the source of truth:

| Instruction | CU | Ceiling |
|---|---:|---:|
| `initialize_protocol` | 64,800 | 200,000 |
| `open_position` (synthetic) | 59,211 | 140,000 |
| `open_position` (converted) | 57,729 | 140,000 |
| `open_position` (referred) | 59,886 | 120,000 |
| `increase_position` | 52,437 | 120,000 |
| `close_position` | 51,724 | 120,000 |
| `deposit_collateral` | 16,904 | 60,000 |
| `withdraw_collateral` | 16,938 | 60,000 |
| `initialize_market` | 14,645 | 60,000 |
| `crank_market_price` (1 feed) | 11,766 | 60,000 |

Every figure is asserted against its ceiling in CI, so a regression fails the build.

## Code Architecture

### Program structure
```
programs/solfx-core/
  ├── src/
  │   ├── instructions/      # All 25+ instruction handlers
  │   ├── state/             # Account structures (Position, Market, UserAccount, etc.)
  │   ├── constants.rs       # Seeds, precision constants, thresholds
  │   ├── oracle.rs          # Pyth price loading, staleness checks, confidence gates
  │   ├── risk.rs            # assess() margin formula, liquidation logic
  │   ├── lib.rs             # Anchor program definition
  │   └── errors.rs          # 40+ domain-specific error codes
  │
  ├── tests/
  │   ├── common/mod.rs      # Harness + assert_invariants() (I1–I8), line 2241
  │   ├── markets.rs         # 32
  │   ├── positions.rs       # 26
  │   ├── oracle.rs          # 26
  │   ├── risk_engine.rs     # 23
  │   ├── referral.rs        # 20
  │   ├── liquidity.rs       # 20
  │   ├── triggers.rs        # 14
  │   ├── collateral.rs      # 14
  │   ├── protocol.rs        # 13
  │   ├── compute_budget.rs  # 8  — CU measurements, asserted against ceilings
  │   └── scenarios.rs       # 7  — end-to-end
```

### Math library (pure Rust, no Solana deps)
```
crates/solfx-math/
  ├── src/
  │   ├── pnl.rs            # unrealized/realized PnL, notional, equity
  │   ├── margin.rs         # health factor, maintenance/liquidation thresholds
  │   ├── pricing.rs        # bid/ask spread, slippage, execution price
  │   ├── fees.rs           # trading fees, funding, rebate rates
  │   └── constants.rs      # QUOTE_PRECISION (1e6), PRICE_PRECISION (1e9), etc.
```

### Keeper (off-chain, Rust async)
```
crates/solfx-keeper/
  ├── src/
  │   ├── main.rs           # CLI, config, service startup
  │   ├── book.rs           # Local protocol state mirror
  │   ├── chain.rs          # RPC client, transaction submission
  │   ├── pyth.rs           # Hermes price feed monitoring
  │   ├── config.rs         # Config parsing (RPC URL, keypair, etc.)
  │   ├── danger.rs         # Liquidatability assessment
  │   └── services.rs       # Liquidator, trigger executor, crank keeper loops
```

---

## Phase 8 — what is built, 2026-09-07

Devnet, `2EQzy2…ZVKi`. `./scripts/deploy-devnet.sh` verifies bytecode and IDL against the
local build and changes nothing when they already match.

**Working end to end from the browser:** create account, deposit, withdraw, open, close,
stop-loss and take-profit, live liquidation price, P&L in pips and dollars, the swap-rate
table with the SolFX markup split out, real OHLC candles at 15m/30m/1H/4H/1D with entry and
liquidation lines drawn on them, and a risk disclosure on first connect.

**Added 2026-09-09.** Positions | History | Summary under the chart, read from
`PositionOpened`/`Decreased`/`Liquidated` events in transaction logs rather than an indexer —
a closed position leaves no account, only its events. A **liquidity pool page** at `/pool`:
AUM, NAV per share, the fee schedule and lifetime flows public with no wallet, then deposit
and the request → cooldown → settle withdrawal state machine. A **partners page** at
`/partners`: the referral pool's real accrual from `solfx-core`, the tier ladder recomputed
from `solfx-math`'s rules, and a trader's own referrer and generated fees.

**Phase 8's feature list is complete.** Partial closes (a Reduce drawer on each position row,
routing 100% to `close_position` because `decrease_position` requires `size_delta < size_base`
strictly), market search and favourites, and an ADL tab. What remains unwired is IB
**registration and claiming**, which are written and tested in the SDK but wait on the
referral programme being initialised — see below.

**ADL is not a control and the page says so.** § 6.9 requires the venue to state in plain
language, before a position is opened, that a trader's *profit* can be taken to cover someone
else's loss. The risk disclosure did not mention it at all; it does now. The tab shows the
insurance fund, whether a shortfall is outstanding, and each position ranked the way § 6.9
ranks candidates — unrealised P&L against collateral, descending, with only positions in
profit eligible at all.

**The app has a test runner.** `vitest` in `app/`, wired into `npm run ci`. Note that
`format:check` **was already failing on 21 files before any of this work** — verified against
a pristine `HEAD` — so `npm run ci` is red at that step for reasons that predate Phase 8's
final tranche.

### The referral programme is deployed and switched off

`solfx-referral` (`J7dwkNcy…MHsyt`) is on devnet and holds **zero accounts**:
`initialize_referral` has never run, and `Protocol.referral_authority` is still
`Pubkey::default()`, which is the protocol's own way of saying it pays no rebates. Turning it
on is two admin transactions by two different authorities — `initialize_referral` on the
referral program, then `set_referral_authority` on the core — and the admin wallet is
deliberately not on the VPS. Until then `/partners` reports that state rather than rendering
buttons that would fail.

Measured while building it: `fee_split_referral_bps` is **500** on devnet, not the 1000
`ARCHITECTURE.md` § 8.3 assumes. Since `entitlement` caps a tier's share at what the pool
actually holds, every tier from Bronze (8%) up is currently capped at the pool. The maths
handles it; the number is a configuration decision nobody has made yet.

### `solfx_referral` has no generated client

`codama.json` names one IDL, `target/idl/solfx_core.json`. Neither program has an IDL account
at the classic Anchor address on devnet either — `anchor idl fetch` has nothing to return —
so `clients/js/src/referral/` is **hand-written**: three accounts, four instructions, three
PDAs, and the tier maths ported from `crates/solfx-math/src/referral.rs`. Its tests re-derive
every discriminator from `sha256("global:<name>")` and assert account order, because Anchor
matches accounts positionally and a swapped pair is a runtime constraint violation that names
neither account. **Add `solfx_referral` to `codama.json` and delete that directory.**

### Five client bugs found by trading it, all worth remembering

The pattern from localnet repeated exactly: **the protocol rejected nothing incorrectly, and
every failure was in the client.** Three of the five were errors that had been swallowed.

| Symptom | Cause |
|---|---|
| "The provided transaction plan failed to execute" | kit attaches the real cause to `context.transactionPlanResult` **non-enumerably**, and the failure sits inside a `plans[]` array. A walker that enumerates keys finds nothing. `diagnoseSendError` in `clients/js/src/errors.ts` reaches both. |
| "Multiple distinct signers were identified for address …" | The instruction carried `useSigner()`'s signer while `prepare` was handed the raw wallet *session*, so the client built a second signer for the same address. `useMemo` alone cannot fix it — its cache is per component — so the signer is now cached in a module-level `WeakMap` keyed on the session. |
| Position open on chain, terminal said "Open Positions (0)" | `usePositions` caught every failure and rendered it as an empty list. Indistinguishable from having none, and the more dangerous reading. It now says so and prints the cause. |
| Unrealised P&L frozen at the price on connect | `loadPositions` baked prices in at load and only re-ran when the wallet or market set changed. `repricePositions` now marks against the live oracle each poll. |
| New position invisible until a manual reload | The ticket refreshed the account panel and told nothing else. It now calls back on a fill. |

## Pyth access — measured 2026-09-05, and the 11 Sep deadline

**Every Hermes endpoint requires an API key.** The docs page describing
`hermes.pyth.network` as a free public endpoint with a 10-req/10s limit is **out of date** —
it returns `unauthorized` on BTC/USD, the most basic feed there is:

| Endpoint | No key | With our key |
|---|---|---|
| `hermes.pyth.network` | **401** | 200 |
| `pyth.dourolabs.app/hermes` (in use) | — | 200 |
| `hermes-beta.pyth.network` | **401** | — |

So there is no free endpoint to fall back to when the trial expires on **11 Sep 2026**. What
happens to the key on that date is account state on pythdata.app and cannot be determined from
any doc — check the dashboard before the date, not after.

**Swapping the key is configuration, not code.** It lives in one place, `PYTH_API_KEY` in
`.env`, and all three consumers read it from there: `price-poster` and `solfx-keeper` via
clap's `env = "PYTH_API_KEY"`, and the browser via the Vite proxy, which injects the `Bearer`
header server-side so the key never ships in client JavaScript. It is read at process start,
so a restart is required. **The endpoint is equally swappable** — `SOLFX_HERMES_URL`,
`--hermes-url`, `VITE_HERMES_URL`, `VITE_PYTHPRO_URL` — which is what makes a move to another
provider, or to a self-hosted Hermes, a config change rather than a rewrite.

**Load is small**, which widens the options: the poster fetches 6 feeds per pass, one pass per
~27 s — about **0.22 requests/second**. The expensive dependency is the browser chart (Pyth Pro
History), not the protocol.

Three ways out, most independent first: self-host Hermes (Pyth documents it as open-source,
listening to Pythnet and Wormhole — the only option with no expiry date); pay for a Pyth plan
(Pro has no hard rate limits, parameters come from a service agreement); or split the
dependency so charts fall back to `benchmarks.pyth.network` while the poster keeps a keyed
Hermes.

## RPC — the free tier is the binding constraint

All off-chain processes share one endpoint. Measured on a free Helius key: **the poster alone
ran 11 consecutive clean passes, and lost 491 feeds across 371 passes with an unthrottled
keeper beside it.** The poster is the one that suffers, because its calls are the only ones
carrying a 60-second deadline.

Both now share the token bucket in `crates/solfx-keeper/src/throttle.rs`, and the budget is
split deliberately: **poster `--max-rps 5`, keeper `--max-rps 4`, browser polling every 8 s.**
Raise all three together on a paid endpoint; raising one alone starves the others.

Free tiers, for when Helius's 10 RPS is the wall:

| Provider | Free tier |
|---|---|
| **Chainstack** | **3M requests, 25 RPS** ← best free option |
| Alchemy | ~1.11M requests |
| Helius | 1M credits, 10 RPS ← currently in use |
| QuickNode | no perpetual free tier |

Measured with both running at 5 and 4 against the 10 RPS tier: **14 passes, 6 of 6 posted,
zero failures**, worst on-chain age 21 s against the 60 s gate.

## Deploying to a VPS

**The program is not on the VPS.** It is on devnet already. The VPS runs the off-chain half —
the parts that otherwise die when a laptop sleeps.

| What | Why it must run 24/7 |
|---|---|
| `price-poster` | Publishes prices on-chain. Stop it and every trade fails `OracleStale` after 60 s. |
| `solfx-keeper` | Liquidations, trigger execution, funding cranks, watchdog. Stop it and stops never fire. |
| Static web build (`app/dist`) | Served by nginx. |
| **A small API service — does not exist yet** | Replaces the two Vite dev proxies. |
| nginx + TLS | Domain, Let's Encrypt, proxies `/hermes` and `/pythpro` to the API service. |

### The API service is not optional

`/hermes` and `/pythpro` are **Vite dev-server proxies** (`app/vite.config.ts`). They exist
only while `npx vite` is running. A static production build has no server, so the ticker and
the charts 404 — and those proxies are also what keep `PYTH_API_KEY` server-side. Moving them
into the browser would publish the key. This has to be written before the app can be hosted.

### Two traps that fail silently

**Copy `price-accounts/` — 35 keypair files.** These are the accounts the markets point at.
The directory is gitignored, so a fresh clone will not have it, and the poster then
**generates new accounts**. The markets keep pointing at the old ones and go stale
permanently, while the poster reports `6 posted, 0 failed` throughout. `deployment.json` and
`.env` are gitignored for the same reason and must be copied too.

**Do not put the admin wallet on the VPS.** `7ktphn…BdWs` is simultaneously the program
upgrade authority, the protocol admin and the USDC mint authority. None of that privilege is
needed out there: every keeper instruction — `crank_funding`, `crank_market_price`,
`crank_market_session`, liquidation, triggers — takes a plain `Signer` named `keeper` with
**no admin check**. Generate a dedicated operator keypair, fund it with a few SOL, and give
the VPS only that. Losing it costs devnet SOL and nothing else.

### Also worth knowing

`scripts/run-devnet-stack.sh` starts the three local processes with the right budgets and
health-checks them (`--stop`, `--status`). It is the local equivalent of what systemd will do.

When stopping processes by name, **match the absolute binary path**. `pkill -f price-poster`
also matches the shell running the command and kills that instead — a trap this project fell
into three times in one session.

### As built on the VPS — 2026-09-07

The box is **not** a dedicated machine. It already runs an n8n stack and `noxyra-ai.com`, and
two assumptions in the plan above were wrong because of it.

**There is no nginx.** A Traefik container owns ports 80 and 443 and terminates TLS for n8n
and noxyra. Installing nginx there does not "add a vhost", it fights for the port and loses —
and on a reboot it might win, which takes the other two sites down. nginx is installed but has
been `systemctl disable`d for exactly that reason. SolFX is published by adding a router to
the existing Traefik, not by running a second web server.

**The build must be constrained.** 7.8 GB of RAM, no swap, ~2 GB free with the other services
up. `cargo build --release` at default parallelism OOM-killed itself on `init_protocol`, and
the kernel could as easily have picked a container. Build like this instead:

```
systemd-run --scope -p MemoryMax=3G -p CPUQuota=150% \
  cargo build --release -j 1 --bin solfx-keeper --bin price-poster
```

Only those two binaries are needed to run the stack; the other four are development tools.
The cgroup means an overrun kills the build and nothing else.

**`npm install` runs in two places.** `clients/js` has its own `package.json`, and the app
consumes the SDK as TypeScript source. Install only in `app/` and `tsc -b` fails with
`Cannot find module '@solana/kit'` across every generated file.

| Path | What it is |
|---|---|
| `.` | the clone — note the doubled directory |
| `$SOLFX_OPERATOR_KEYPAIR` | operator keypair, `EyvqeDSh2Y4ZhobJY4bF8ueEZAjRx2V3r2GDf35ktPyo` |
| `services/api/server.mjs` | serves `app/dist`, proxies `/hermes` and `/pythpro` with the bearer |
| `scripts/vps-health.sh` | the probe behind `solfx-health.timer` |
| `/docker/solfx/docker-compose.yml` | the web tier — a compose project of its own |

**The site is https://solfx.cloud** (and `www.`), certificate from the Traefik ACME resolver
the n8n stack already had.

The web tier is a **container, not a unit**. Traefik discovers backends over the Docker
socket, so only a container can be routed without adding a file provider — and adding one
means restarting Traefik, which would drop n8n and noxyra. `/docker/solfx` is a separate
compose project that joins the external `n8n_default` network: it never edits the n8n compose
file, and `up`/`down` on it leaves the other three containers running. It publishes 8787 on
loopback only, for the health probe; the public port is Traefik's.

It mounts just `app/dist` and `services/`, so `price-accounts/` and `deployment.json` are not
visible to the internet-facing process. `.env` is an `env_file` rather than a bind mount:
an scp that replaces it writes a new inode, which would silently detach a file mount.
`docker compose up -d` re-reads it — **after copying a new `.env`, restart the container.**

Three units in `/etc/systemd/system`, all `solfx-`prefixed:

| Unit | State | Notes |
|---|---|---|
| `solfx-price-poster` | installed, stopped | needs `.env`, `deployment.json`, `price-accounts/` |
| `solfx-keeper` | installed, stopped | same, plus a funded operator wallet |
| `solfx-health.timer` | enabled | every 5 min; POSTs to `$SOLFX_ALERT_WEBHOOK` if set |

`solfx-api.service` still exists but is disabled — it was the host-side version of the web
tier, replaced by the container. Do not start both; they contend for 8787.

Both chain units declare `Environment=SOLFX_RPC_URL=...` *before* `EnvironmentFile=`, so `.env`
still overrides it — but the keeper's own default is localnet, which on devnet would look like
a keeper that simply never does anything. The keeper also pins `SOLFX_KEYPAIR` *after*
`EnvironmentFile=` so it wins: a `.env` copied off the Windows machine names a path that does
not exist here, and names the admin wallet, which must never reach this box.

The health check tests for a *completed pass*, not for a live process. A poster that is up but
making no progress is still `active`, and that is the failure mode that matters. It also probes
`https://solfx.cloud/healthz` as well as loopback, because a router misconfiguration or a
certificate that failed to renew looks perfectly healthy from inside the box.


## Known Limitations & Open Items

### Q1 — Weekend metals (UNRESOLVED)
Pyth Indices for 24/7 gold/silver do not exist in the public Hermes catalogue. They are published by Pyth but access requires an entitlement or commercial agreement (not a technical problem).

**Impact:** XAU/USD and XAG/USD are session-bound (close Friday 21:00 UTC). Weekend trading on metals is not possible. Workaround: trade tokenised gold (XAUT/USD, PAXG/USD, etc.) — they are crypto assets and trade 24/7, but they are not spot gold.

### Q3 — LATAM session windows (RESOLVED, 2026-08-01)
USD/BRL, USD/CLP, USD/PEN publish at 1s during their local sessions (14–21 UTC windows) but go silent outside. Fully usable in-session via `feed_kind` state machine per market.

### Stack frame risk (MITIGATED for Phase 7, UNPROVEN for NOXFUNDING)
SolFX hit BPF stack frame errors at 13 and 18 accounts. Phase 7 kept it under 18 by not deserializing accounts only passed through. NOXFUNDING will need ~21 accounts for a CPI wrapper — this is why Stage 0 (the feasibility spike) is the first NOXFUNDING task.

---

## Pre-devnet audit — 2026-08-23

Audited: business logic, mutability, borrowing, PDA/bump handling, CPI safety, and every
financial formula. **No code defects found.**

| Area | Result |
|---|---|
| **PDA derivation** | 0 runtime `find_program_address` in program source — all via Anchor `seeds`/`bump` constraints, the safe pattern |
| **`UncheckedAccount`s** | 3, all constrained. `rent_destination` (liquidate, adl) carries `address = user_account.authority`, so a liquidator cannot redirect rent to themselves. `lp.rs` authority is double-constrained (`address` + `has_one`) |
| **Arbitrary CPI** | Closed. All 8 `Accounts` structs declare `token_program: Program<'info, Token>`; the bare `AccountInfo` in `VaultTransfer` is the unwrapped form of an already-validated account |
| **Reinit / resize** | No `init_if_needed`, no `realloc` anywhere |
| **Unchecked arithmetic** | None on money paths — everything routes through `checked_*` / `mul_div_*` in `fixed.rs` |
| **Rounding discipline** | Consistently adverse to the party initiating. Fees `ceil`, payouts `floor`, and `weighted_entry_price` rounds **direction-aware** (long up, short down — both reduce trader profit) |
| **Liquidation test** | Strict `<`. Equity == maintenance margin is *not* liquidatable — closes griefing threat T7 |
| **Spread direction** | Buy pays oracle + adj, sell receives oracle − adj; `ceil` on the adjustment makes any non-zero spread bite by ≥1 unit, so a same-price round trip strictly loses (closes drain T14) |
| **LP inflation attack** | Closed. Deposit and withdrawal both round down, `require!(shares > 0)` guards zero-share deposits, and **there is no donation path** to inflate AUM without minting |
| **Synthetic composition** | Confidences propagate in *relative* terms at `RATE_PRECISION` (not bps, which would truncate sub-1bp legs to zero); composed `publish_time` is the **staler** leg |
| **Funding** | Hourly index accrual; floors so a payment rounds up and a receipt rounds down |

### The strongest evidence: 56 property tests

`crates/solfx-math/tests/properties.rs` asserts the invariants across randomised inputs rather
than fixed examples — `round_trip_at_the_same_price_always_loses`, `pnl_is_antisymmetric`,
`partial_closes_never_realise_more_than_the_whole`, `the_book_is_never_crossed`,
`fees_always_round_toward_the_protocol`, `fee_splits_conserve_every_unit`,
`health_factor_agrees_with_the_liquidation_test`, and 49 more.

### Verified by execution

`cargo test --workspace` run 2026-08-23: **490 passed, 0 failed, 0 ignored**, exit 0. Includes
the full LiteSVM integration suite (both `.so` binaries loaded) and the 56 property tests.

---

## Localnet: verified working, 2026-08-24

The first real trade. Not a test harness — a validator, live Pyth prices, and the same
instructions a trader would send.

```
open   BTC/USD long, $1,000 notional, $200 margin
       entry 77,485.88   oracle 77,408.47   (above oracle: a buy crosses the spread)
close  same position
```

| | |
|---|---|
| Collateral before | 100,000.00 |
| Collateral after | **99,997.90** |
| Round-trip cost | **$2.10** |
| LP pool | 1,000,000.00 → **1,000,002.02** |

The trader lost $2.10, the pool gained $2.02, the remainder went to the fee splits. **The
B-book works, and invariant I2 holds against a real cluster rather than only in tests.**

### The four tools this needed

All in `crates/solfx-keeper/src/bin/`. None of them touch `programs/`.

| Binary | What it is for |
|---|---|
| `devnet-feed-probe` | Which markets have a usable Pyth feed on a given cluster |
| `price-poster` | Publishes Pyth prices where no sponsor does — required on devnet *and* localnet |
| `init-protocol` | Creates a test mint, initialises the protocol, lists and activates markets |
| `trade` | setup / status / open / close, so the protocol can be exercised before Phase 8 |

`crates/solfx-keeper/src/contracts.rs` holds the contract-size table, shared by `#[path]` so
the three tools cannot drift apart.

### Five failures on the way, and what each one proved

Every one was a **client** bug. The protocol rejected all five correctly, which is the more
useful result.

| Error | Cause | What it says about the protocol |
|---|---|---|
| `MarketClosedForOpens` | `initialize_market` leaves a market `Initialized`; nothing called `set_market_status` | Deliberate: a mistyped feed id or risk parameter cannot be traded the instant it is listed |
| `WrongOracleFeed` | `post_update_atomic` can only produce `VerificationLevel::Partial` | SolFX requires `Full`. The receiver SDK itself warns partial updates lower the collusion threshold |
| `OracleStale` | Posting 33 feeds at ~5 tx each took 102s, serially, so each feed aged by every feed before it | The 60s staleness gate does exactly what it should |
| `SlippageExceeded` | The client passed `price_limit: 0` expecting "disabled" | There is no disabled. `validate_slippage` always compares, so no order can go out unbounded |
| `PositionSizeOutOfBounds` | Position bounds derived from the FX lot for every asset, making BTC's *minimum* ~0.1 BTC | Bounds are enforced, and they were wrong in the config, not the code |

### Contract size is a client concern, and clients must not get it wrong

`Market` stores **no** contract size, and `solfx-math`'s `LOT_SIZE_BASE` is the FX figure,
commented there as "a presentation concept only". The engine is right to work in base units.
But a lot is not one thing:

| Class | 1 lot | Source |
|---|---|---|
| FX | 100,000 base units | MT4/MT5 standard |
| Gold | 100 troy ounces | XAUUSD CFD standard |
| Crypto | 1 coin | BTCUSD = 1 BTC at XM and Exness |

Applying the FX number to Bitcoin makes `--lots 0.01` mean **1,000 BTC instead of 0.01** —
five orders of magnitude, and it reads as a plausible order throughout.

> **Phase 8 decision:** every client needs this table. Decide whether it belongs on `Market`
> before building the UI, or the frontend will carry its own copy and eventually disagree
> with the CLI about what a lot is.

Sizing by exposure sidesteps it entirely and is how people think about crypto:
`trade open --market BTC/USD --notional 1000` is $1,000 of BTC at the current oracle price.

### Reaching `Full` verification takes four transactions

Measured, not assumed: `post_update_atomic` carrying every guardian signature builds a
**2364-byte transaction against a 1232-byte limit**. That is why the receiver offers
`minimum_signatures` at all, and why the trimmed form yields `Partial`.

So `price-poster` does what the sponsored publishers do:

```
init_encoded_vaa       allocate a buffer on the Wormhole program
write_encoded_vaa      stream the VAA in 700-byte chunks
verify_encoded_vaa_v1  check every guardian signature   <- this earns Full
post_update            the receiver writes the price at Full
close_encoded_vaa      reclaim the rent
```

**This needs the Wormhole program on the cluster**, not just Pyth's. `deploy-localnet.sh`
now derives and clones it along with the receiver's config and treasury PDAs and the
guardian set — `price-poster --print-clone-args` computes them from a live cluster.

**Cost:** ~5 transactions per feed. The poster reads `deployment.json` and posts only the
listed markets by default; posting all 33 leaves everything stale before its next turn.

**Feeds are posted concurrently, and the RPC rate is capped.** Both are load-bearing for the
60s gate, and each was measured after the naive version failed:

- *Serial posting.* A price is dated when Hermes serves it, not when it lands, so the last
  feed in a serial pass carried a price as old as every preceding feed's transactions put
  together. BTC/USD, posted last of six, landed **142s** old against the 60s gate.
- *Unthrottled concurrency.* Fixing the ordering exposed the next ceiling. Helius's free tier
  allows ~10 RPC calls/s, and `RpcClient::send_and_confirm_transaction` fetches its own
  blockhash per transaction and then makes **two** calls per unconfirmed poll —
  `getSignatureStatuses` *and* `isBlockhashValid` — twice a second. Five transactions per feed
  across six feeds is ~400 calls in a ~25s pass, about 16/s. Whole feeds were lost to
  `429 Too Many Requests` at random, most often on the longest step, `verify_encoded_vaa_v1`.

Retrying the 429 would have made it worse: `solana-rpc-client`'s HTTP sender already retries
one five times, honouring `Retry-After` for up to 120s each (`solana-rpc-client-3.1.14`,
`src/http_sender.rs:147`), so a throttled call can hold a feed for minutes. The poster
instead sends less: one blockhash per *feed*, one status call per second, the rent reclaim no
longer confirmed on the critical path, and a token bucket above the client (`--max-rps`,
default 8) that spaces every call so the burst never forms.

### The blockhash is per feed, and preflight is skipped — 2026-09-08

Both were one bug, and it cost exactly two feeds per pass for weeks.

A blockhash was originally fetched once per *pass* and shared by all ~30 transactions in it,
on the reasoning that a pass is ~25 s and a blockhash lives ~60 s. That holds at
`--concurrency 8`, where all six feeds go out together. At `--concurrency 2` — which the VPS
runs, because Helius throttles *transaction* sends far harder than reads — a six-feed pass is
three sequential waves, and the last wave starts ~40 s in, then spends five more transactions
getting to `post_update`. BTC/USD and XAG/USD failed every pass because they are simply last,
never because of anything about those feeds.

It presented as two different errors, neither of which named the cause:

| Symptom | What it actually was |
|---|---|
| `verify_encoded_vaa_v1 — is the guardian set cloned and current?` | the poster's own hint on the longest step, which is just the step most likely to still be in flight |
| `Transaction simulation failed: Blockhash not found` | **preflight**, run on a load-balanced node a few slots behind the one that served the blockhash — Helius documents this as the mismatched-RPC case |
| `was not confirmed in 45s` | what remained after preflight was skipped: the blockhash had genuinely expired, so the send was accepted and the transaction silently dropped |

So: `skip_preflight` (with `preflight_commitment` still set to the level the blockhash was
fetched at, which is what Helius prescribes even when preflight is skipped), and a blockhash
per feed rather than per pass. Six `getLatestBlockhash` calls a pass instead of one is a
rounding error against the ~30 sends and ~100 status polls it already makes, and unlike a
concurrency setting it stays correct however this is tuned.

**Measured 2026-09-08**, devnet, `--concurrency 2 --max-rps 2`: **14 consecutive passes, 6 of
6 posted, zero failures**, all six feeds inside the 60 s gate — including BTC/USD, whose price
account had never been successfully created before this.

**Measured 2026-09-01**, devnet, six markets, `--interval-secs 2 --concurrency 8 --max-rps 8`:
seven consecutive passes, **42 of 42 feeds posted, zero 429s**, worst on-chain age **38s**
against the 60s gate, with all six feeds within **two seconds of each other**. Raise
`--max-rps` on a paid endpoint or a local validator; it is the binding constraint here, not
concurrency.

## The RPC ceiling is the binding constraint, measured — 2026-09-08

**The poster and the keeper cannot both run against this endpoint.** Not a configuration
mistake and not something scheduling can fix: the demand exceeds the supply.

It works on a dev machine because a dev machine runs a local validator, where an RPC call is
free. `--scan-ms` defaults to **400** — two and a half book reads a second, forever — which
localnet absorbs without noticing. Pointed at a metered endpoint it is, on its own, more than
the whole budget.

Measured here, same keypair, same everything, one variable:

| Running | gateway traffic (60 s) | upstream 429s | poster |
|---|---|---|---|
| poster + keeper | high=152 low=135 | **160** | `0 posted, 6 failed` |
| poster alone | high=110 low=0 | **0** | `6 posted, 0 failed` |

Slowing the keeper ten-fold (`--scan-ms 4000`) was not enough: it still drew ~3.3 calls/s,
and the poster stayed at 0/6. The endpoint starts refusing somewhere around 5–6 calls/s
sustained, well under the ~10/s the free tier advertises.

### The gateway, and what it does not do

`services/rpc-gateway/` holds one budget for every off-chain process and serves the poster
first — `/high` for the poster, `/low` for the keeper, one token bucket, the poster queued
almost without bound and the keeper shed under pressure. It is correct and it is installed,
and it did not make both fit, because **allocating a budget is not the same as raising one**.
It earns its place the moment there is enough capacity to divide, and until then it is what
keeps the poster from being crowded out by the browser or by an operator command.

Two traps it cost to learn, both now in comments:

- A single queue cap sheds the *poster*, which is the one caller that must never be shed. Its
  log filled with `429 ... for url (http://127.0.0.1:8899/high)` — a rate limit it had
  imposed on itself.
- `EnvironmentFile=` overrides `Environment=` **regardless of order in the unit file**. The
  keeper kept the raw endpoint from `.env` and bypassed the gateway entirely while appearing
  configured. Verified by reading `/proc/<pid>/environ`. Pass the URL on the command line.

### What unblocked it — 2026-09-09, and it was free

**A second endpoint, not a paid tier.** The framing above was wrong in one respect: the
answer was never to divide a budget that is too small, it was to stop sharing. The poster
keeps the keyed Helius endpoint through the gateway; the keeper goes straight to Solana's
public devnet RPC. Measured before switching: 19 keeper-shaped calls (4 `getProgramAccounts`
+ 15 `getMultipleAccounts`) against `api.devnet.solana.com` in **0.6 s, zero 429s**, and a
`getProgramAccounts` returning all 9 markets in **50 ms**. With both running: poster
**6 posted, 0 failed** on every pass, keeper cranking, zero rate limiting on either side.

The Solana MCP supplied the two facts that made the choice: Helius bills `getProgramAccounts`
at **10 credits** and rate-limits it *separately from* the plan's RPS — which is why slowing
the keeper's scan loop ten-fold had not helped, since the cost was never the call count — and
`getProgramAccountsV2` bills at 1. Chainstack's free tier (**3M requests, 25 RPS**) is the
fallback if the public endpoint starts throttling.

Both units are now `enable`d. Before this they were not: the poster was running but
`disabled`, so a reboot would have taken the venue down silently and left it down.

### A stopped keeper is a closed venue, not just an unliquidated one

Found the same day, and the more important half. Eight of nine markets were sitting in
`MarketStatus::Halted` — the *correct* fail-closed response to the stale feeds of 8 Sep, set
by `crank_market_session`. But `crank_market_session` is also the **only** instruction that
can move a market back out of `Halted`, and only the keeper sends it. So the markets stayed
shut for 23 hours while the poster published six perfect feeds. Recovery is deliberately
two-step — `Halted → GapWindow → Active`, `GAP_WINDOW_SECONDS` = 5 min apart — and took
about six minutes once the keeper was cranking again.

### `--refresh-secs` must be shorter than `--max-book-age-secs`

The shipped defaults are refresh **30 s** against a tolerance of **20 s**, so the book is
stale by construction for a third of every cycle. Worse, the crank tick (60 s) is an exact
multiple of the refresh interval, so once a tick lands inside that stale window it lands
there every time. Measured: **165 `book is stale; standing down this pass` in ten minutes and
zero cranks in eight** — while systemd reported the unit `active (running)` and
`scripts/vps-health.sh` reported it healthy. The deployment now runs `--refresh-secs 15
--max-book-age-secs 45`; after the change, zero stale-book errors and 28 cranks in two
minutes. **The defaults themselves are still wrong and should refuse to start** — a startup
check that `refresh_secs < max_book_age_secs` is a one-line guard in `crates/solfx-keeper`
and is not yet written.

The health probe deserves the same criticism it levels at the poster: it tests that the unit
is active, which a keeper doing nothing at all still is.

### Three markets are listed that nothing can price

`EUR/JPY` (#1), `USD/INR` (#2) and one of the two `BTC/USD` markets (#4) carry feed ids the
poster does not publish, so they will stay `Halted` for as long as that is true, and the
keeper logs `price account missing on chain` for #4 on every pass. #1 and #2 are a
`deployment.json` change — the poster posts what is listed there. #4 is a **duplicate
listing** (#5 is the BTC/USD that works) and can only be retired with `set_market_status`,
which needs the admin wallet that is deliberately not on this box.

## Devnet feed availability — measured 2026-08-23

**Probe:** `cargo run -p solfx-keeper --bin devnet-feed-probe`
(source: [`crates/solfx-keeper/src/bin/devnet_feed_probe.rs`](../crates/solfx-keeper/src/bin/devnet_feed_probe.rs),
raw: [`devnet-feed-probe.json`](devnet-feed-probe.json))

It derives addresses with the keeper's own `pyth::price_account`, shared via `#[path]`, so it
checks the accounts the protocol will actually read rather than merely similar ones.

### Result: 8 of 33 markets are usable from sponsored devnet feeds

| Group | Tradeable | |
|---|---|---|
| FX majors | 3/5 | USD/JPY, USD/CAD abandoned (107d, 116d) |
| FX crosses | **0/8** | 2 abandoned, 6 have no account at all |
| FX reduced-leverage | **0/2** | both abandoned 116d |
| Metals | 2/4 | XPT, XPD abandoned 205d |
| Emerging markets | **0/7** | USD/INR abandoned 15d; the other 6 absent |
| LATAM | **0/3** | all absent |
| Crypto | 3/3 | all LIVE, ~30–90s |
| Commodities | **0/1** | absent |

`ABANDONED` is the trap: the account **exists**, so a naive existence check passes, but it
has not published in months and `MAX_ALLOWED_STALENESS_SECONDS = 60` means the protocol
refuses every trade against it.

### This is not a blocker, and the reason is structural

`price_update` is declared `Box<Account<'info, PriceUpdateV2>>` with **no seeds constraint**
([open_position.rs:72](../programs/solfx-core/src/instructions/trader/open_position.rs#L72)).
The protocol never requires the *sponsored* PDA. It requires only:

1. the account is owned by the Pyth receiver — enforced by Anchor's `Account<T>` owner check;
2. the update's internal `feed_id` equals `market.pyth_feed_id` — enforced by
   `get_price_unchecked(expected_feed_id)` in [oracle.rs](../programs/solfx-core/src/oracle.rs).

So **SolFX will accept a price update we post ourselves.** Pyth is a pull oracle; the data
stays guardian-signed, we merely relay it instead of waiting for a sponsor who is not
publishing these feeds on devnet. Both programs are live there:

| Program | Devnet |
|---|---|
| Pyth receiver `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ` | ✅ executable |
| Push oracle `pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT` | ✅ executable |

### Solved — the price poster

[`crates/solfx-keeper/src/bin/price_poster.rs`](../crates/solfx-keeper/src/bin/price_poster.rs)

Fetches signed updates from Hermes, decodes the accumulator blob, trims the VAA to the
receiver's `minimum_signatures` (3 on devnet — a full 13-signature VAA does not fit the
packet alongside everything else), and posts `post_update_atomic` per feed.

Verified against devnet 2026-08-23: **33 of 33 symbols resolved and built**, including every
EM pair that has no sponsored feed there. Hermes carries the mainnet aggregate for all of
them; only the on-chain delivery was missing.

Design notes worth keeping in mind:

- **Addresses are stable.** `post_update_atomic` requires the price-update account to sign,
  so each feed gets a keypair persisted under `--keys-dir`, reused across restarts. The
  emitted `price-accounts.json` (symbol → feed id → account) is what the keeper and frontend
  must read; **these are not the canonical `[shard, feed_id]` PDAs**, so anything that
  derives addresses the sponsored way will look in the wrong place on devnet.
- **The discriminator is hardcoded** (`sha256("global:post_update_atomic")[..8]`) because
  this crate has no sha256 dependency, but a test re-derives it from the name, so a drift
  fails the suite rather than a devnet transaction.
- **Security is unchanged.** The data is guardian-signed on Pythnet and verified on chain by
  the receiver. The poster is a courier: it cannot forge a price, only fail to deliver one,
  which the protocol already treats as staleness.

### What it adds to the plan

A **price poster** — fetch VAAs from Hermes, submit `post_update` to the receiver on devnet,
keep 33 accounts fresh. The keeper already has a Hermes client (`pyth.rs`), and Phase 8
already budgets a "price relayer", but that one *caches VAAs for the frontend to read*. This
one has to *write on chain*, which is a different job and is currently in no phase.

Re-run the probe during the London/NY overlap before finalising: the five `SESSION-STALE`
entries were measured on a Sunday and may revive. The `ABANDONED` and `NOT ON CLUSTER` ones
will not.

---

## Running it locally

Four terminals, but only two need to stay open.

```bash
# 1 — validator. Leave running.
./scripts/deploy-localnet.sh

# 2 — commands
solana config set --url http://127.0.0.1:8899
solana airdrop 100
./target/debug/init-protocol

# 3 — price poster. Leave running. Without it every trade fails on staleness after 60s.
./target/debug/price-poster --rpc-url http://127.0.0.1:8899

# 2 — trade
./target/debug/trade setup --amount 100000
./target/debug/trade open --market BTC/USD --direction long --notional 1000 --collateral 200
./target/debug/trade status
./target/debug/trade close --market BTC/USD
```

**Never run `cargo` while the validator is up.** WSL2 was OOM-killed on 2026-08-24 doing
exactly that: a 12-job build alongside a validator on a 7.7 GB ceiling. Mitigated by
`.wslconfig` (`memory=12GB`) and `.cargo/config.toml` (`jobs = 6`), but the habit is the real
fix — build first, then start the validator, then use `./target/debug/*` directly.

`init-protocol` and `price-poster` are both idempotent and safe to re-run.

### Market hours decide what you can test

Only the three crypto feeds are continuous. FX and metals follow the interbank week —
**Sunday 21:00 UTC to Friday 21:00 UTC** — and outside it their feeds do not publish, so the
protocol correctly refuses to trade them. Test with BTC/USD, ETH/USD or SOL/USD at a weekend;
their prices are the only ones moving.

## Deployment Readiness

### Pre-devnet checklist
- [x] **Full suite green** — 490/490, verified 2026-08-23 on Anchor 1.1.2
- [ ] **Verify CPI feature** works (`solfx-core/Cargo.toml features = ["cpi"]`)
- [ ] **Check fee vault initialization** (where trading fees accumulate)
- [ ] **Verify RPC URLs** for devnet (typically `https://api.devnet.solana.com`)

### Deployment steps
1. Derive all PDA addresses (protocol, markets, vaults, LP pool)
2. Call `initialize_protocol` once to set up the singleton state
3. Call `initialize_market` for each of the 33 markets (FX + Crypto + Commodity)
4. Mint initial LP tokens to a test account (~100k USDC initial pool)
5. Deploy keeper binary and start background services

### Post-devnet verification
- Make a test trade on each market type (FX, EM, Crypto, Commodity)
- Verify oracle pricing works for all markets
- Test liquidation on an undercollateralized position
- Verify trigger orders fire correctly
- Check keeper services are running and processing positions
- Confirm events are being emitted (for the indexer)

---

## Sequence for Next Phase: NOXFUNDING

**Do NOT start NOXFUNDING until:**
1. ✅ SolFX is deployed to devnet
2. ✅ All 33 markets are listed and testable
3. ✅ Keeper is running stably
4. ✅ Test trades complete end-to-end

**Then:**
- **Stage 0:** Feasibility spike — prove a PDA-authority CPI into `open_position` fits the stack frame
- **Stages 1–7:** Full NOXFUNDING implementation (evaluation, mandates, settlement, keeper, UI)

**Total NOXFUNDING timeline:** ~8–12 weeks after SolFX devnet is live.

---

## Key Numbers (verified 2026-08-23)

| Metric | Value | Verified against |
|---|---|---|
| **Phases complete** | 7 of 7 | git |
| **Tests (static count)** | 488 | grep of `#[test]`/`#[tokio::test]` |
| **Markets to list** | 33 (29 FX/Metals + 3 Crypto + 1 Commodity) | Phase 0b + user spec |
| **Max oracle staleness** | **60 s** | `constants.rs:52` `MAX_ALLOWED_STALENESS_SECONDS` |
| **Max oracle confidence (hard ceiling)** | **3000 bps** | `constants.rs:65` `MAX_ALLOWED_CONF_BPS` |
| **Per-market confidence gates** | configured per market | `Market::liquidation_max_conf_bps` (`market.rs:315`) |
| **Leverage** | per market, `Market::max_leverage` | `market.rs:175` — not a global constant |
| **Liquidation fee** | per market, `Market::liquidation_fee_bps` | `market.rs:180` |
| **Funding accrual** | **hourly** (`funding_rate_per_hour`) | `crates/solfx-math/src/funding.rs:125` |
| **Keeper scan interval** | 400 ms (configurable) | `services.rs` |
| **`open_position` CU** | 59,211 (synthetic) | `docs/compute-budget.md` |
| **`liquidate_position` packet** | 758 bytes measured | `compute_budget.rs` (limit 1232) |

> **Risk parameters are per-market, not global.** Leverage, liquidation fee and confidence
> ceilings live on the `Market` account and are set at listing time. There is no single
> protocol-wide leverage or margin number — do not quote one.

---

## Contributors and decisions

**Author:** Ahmed Mustafa Khan  
**Build start:** February 2026  
**Phase 7 complete:** August 2026  
**Scope:** Forex broker, non-custodial, Solana, B-book model  

**Key decisions locked:**
- Use Pyth V2 for oracle (pull model, no VAA)
- Isolated margin model (no cross-collateral)
- Permissionless liquidation (anyone can liquidate)
- B-book LP pool (protocol holds risk)
- No leverage beyond 50x (stack safety)
- Session calendars per market (not a global rule)
- Anchor 1.1.2 (stable, audited)

---

## How to use this document

**For future work:**
- Refer to this whenever you need to remember what SolFX is, what's been tested, and what's still open.
- Before any NOXFUNDING stage, check the "Deployment Readiness" section.
- The "Known Limitations" section lists open items that do not block SolFX but affect feature completeness.

**For external communication:**
- Sections 1–2 are suitable for explaining SolFX to non-technical stakeholders.
- Sections 3–5 are for developers.
- The "Key Numbers" table is useful for marketing or pitch decks.

---

**Document version:** 1.0 (locked at Phase 7 completion)  
**Next update:** When SolFX ships to devnet (2026-08-??)
