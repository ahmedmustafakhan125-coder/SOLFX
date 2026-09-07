# SolFX — Complete Context and Build Status

**Last updated:** 2026-09-07
**Status:** Phases 1–7 complete. **Phase 8 (frontend + SDK) substantially built.**
555 Rust tests + 128 SDK tests passing, clippy clean.

**Live on devnet.** Program `2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi`, 9 markets listed
(6 active), on-chain IDL current at 38 instructions. Real trades open and close from the
browser with Phantom.

**Next: put the off-chain half on a VPS** so it runs 24/7 instead of dying when a laptop
sleeps. See *Deploying to a VPS* below. After that: the test-suite honesty pass, then Phase 9.
NOXFUNDING still starts only after all of that.

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

**Not built.** Named so a missing feature is not mistaken for a bug: LP pool page, IB referral
dashboard, trade history / portfolio (needs an indexer — the largest remaining piece),
partial-close UI (the program supports it; the ticket does not expose it), market search and
favourites, ADL controls.

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
instead sends less: one blockhash per *pass* shared by every transaction in it, one status call per second, the rent reclaim no longer confirmed on the
critical path, and a token bucket above the client (`--max-rps`, default 8) that spaces every
call so the burst never forms.

**Measured 2026-09-01**, devnet, six markets, `--interval-secs 2 --concurrency 8 --max-rps 8`:
seven consecutive passes, **42 of 42 feeds posted, zero 429s**, worst on-chain age **38s**
against the 60s gate, with all six feeds within **two seconds of each other**. Raise
`--max-rps` on a paid endpoint or a local validator; it is the binding constraint here, not
concurrency.

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
