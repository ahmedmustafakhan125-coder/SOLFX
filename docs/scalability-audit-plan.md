# Scalability & concurrency audit — plan

Plan for the 8-phase brief. Written 2026-08-28, before any measurement.
Findings land in `docs/scalability-audit.md`; this file is the plan and the division of labour.

> **Scope: this is a final sanity check before devnet, not a production scalability programme.**
> That rules a lot in and out. Phases 0–3 answer the question that matters — *where does this
> break first, and at what load* — from consensus-enforced limits and CU figures we already
> have, needing nothing from you and no validator. Phase 7 is struck (§4). Phases 4–6 are
> worth doing only if 0–3 turn up something that changes the devnet decision.
>
> Devnet will not see 500 TPS. The value of this exercise is knowing the ceiling **before**
> building a frontend against an architecture that cannot be scaled without changing it.

---

## 1. The brief's constants — verified, not assumed

All three checked against live Solana documentation through the MCP server. **All three are
correct**, and one carries a consequence the brief does not draw out.

| Constant | Brief says | Verified | Source |
|---|---|---|---|
| Block compute limit | 100,000,000 CU | ✅ live on mainnet **29 July 2026, epoch 1009** | [SIMD-0286](https://github.com/solana-foundation/solana-improvement-documents/blob/HEAD/proposals/0286-raise-block-limits-to-100M.md), [solana.com/upgrades/100m-cu-blocks](https://solana.com/upgrades/100m-cu-blocks) |
| Per-writable-account limit | 12,000,000 CU | ✅ **unchanged** by SIMD-0286 | same upgrade page: *"Max Writable Account Compute Units: 12M → 12M"* |
| Block time | ~400 ms | ✅ current | SIMD-0525 (250/200 ms) is status **Draft** |

**The consequence.** Solana's own upgrade note is explicit:

> *"Max writable account units, the most CUs that can write to any single account within one
> block, stays at 12M… A congested market or popular program still hits its 12M per-account
> ceiling at the same point, but the block has room for far more activity on unrelated
> accounts alongside it."*

So the 100M block limit is **irrelevant to a design with a global hot account**. It buys
parallel capacity across *unrelated* accounts. If every SolFX trade writes the same account,
SolFX is capped at 12M CU per block no matter how large blocks get.

**Pending change worth tracking:** [SIMD-0306](https://github.com/solana-foundation/solana-improvement-documents/blob/HEAD/proposals/0306-raise-account-cu-limits.md)
would raise the per-writable-account limit (title says 20M; body argues for 40% of the block
limit, i.e. 40M at 100M blocks). Status **Review — not activated.** Model 12M today. If it
activates, every per-account ceiling below multiplies by 1.7× or 3.3×. It is the single
biggest external lever on our headline number, and we control none of it.

---

## 2. Preliminary Phase 1 answer — the reason to reorder the work

**Not yet measured. Derived by reading `OpenPosition` during the security audit, plus the
already-measured 59,211 CU from `docs/compute-budget.md`. Phase 1 must confirm it properly.**

`open_position` declares these as `mut` ([open_position.rs:20-78](../programs/solfx-core/src/instructions/trader/open_position.rs#L20-L78)):

| Account | Scope | Contention |
|---|---|---|
| `user_account` | per-user PDA | none |
| `position` | per-user/market/nonce PDA | none |
| `market` | **per-market** | all traders on that pair |
| `protocol` | **GLOBAL** | every trade, every market |
| `lp_pool` | **GLOBAL** | every trade, every market |
| `insurance_fund` | **GLOBAL** | every trade, every market |
| `collateral_vault` | **GLOBAL** token account | every trade, every market |
| `lp_vault` | **GLOBAL** token account | every trade, every market |
| `insurance_vault` | **GLOBAL** token account | every trade, every market |
| `fee_vault` | **GLOBAL** token account | every trade, every market |

**Seven global mutable accounts on the hot path.** Not one — seven. The write-lock cost model
charges a transaction's full CU against *every* writable account it locks, so all seven hit
the same 12M ceiling simultaneously.

Preliminary arithmetic, using the measured 59,211 CU:

```
12,000,000 / 59,211  ≈  202 transactions per block
202 / 0.4 s          ≈  506 TPS
```

**≈500 TPS for the entire protocol — all markets combined, not per market.** Listing 33
markets does not raise it, because the ceiling is not `market`; it is `protocol` and the four
shared vaults. The brief asks "is there anything that serializes ALL users into a single write
lock?" — on this reading, yes, and there are seven of them.

If Phase 1 confirms this, it is the finding, and Phases 4–7 are detail. That is why the
sequence below front-loads Phases 0–3.

---

## 3. Phase-by-phase: feasibility and owner

| Phase | Owner | Status | Notes |
|---|---|---|---|
| 0 — Inventory | me | ready | 37 instruction handlers to tabulate |
| 1 — Write-lock map | me | ready | **the phase that matters**; preliminary answer above |
| 2 — CU measurement | me | ready, head start | `tests/compute_budget.rs` (734 lines, 8 tests) already measures typical CU on LiteSVM and asserts ceilings; `docs/compute-budget.md` has the table. Missing: **worst case** |
| 3 — Capacity math | me | ready | pure arithmetic once 1+2 land |
| 4 — LiteSVM volume harness | me | ready, needs scoping | see §5 |
| 5 — Concurrent load generator | **you deploy, me write** | blocked on you | see §6 |
| 6 — Adversarial concurrency | split | partly blocked | see §7 |
| 7 — Off-chain load test | — | **strike it** | see §4 |
| 8 — Final report | me | after the rest | |

---

## 4. Phase 7 does not apply — recommend striking it

The brief asks to load-test API endpoints, count RPC calls per page load, simulate 10k
WebSocket price subscribers, and measure indexer lag.

**None of those exist.** Verified:

- no `package.json`, no `node_modules`
- no `app/`, `frontend/`, `web/`, `ui/`, `backend/`, `api/`, `server/`, `indexer/`, `sdk/`
- `scripts/` contains exactly `deploy-localnet.sh`, `run-keeper.sh`, `test-100.sh`

The frontend, SDK and indexer are Phase 8 of the product roadmap and have not started. There
is no endpoint to point k6 at. Running Phase 7 would mean inventing a backend to load-test,
which measures the invention, not SolFX.

**Recommendation:** strike Phase 7 and re-run it when Phase 8 ships. Its substance — *"the
chain often survives while the infrastructure dies"* — is correct and should be preserved as a
requirement on the frontend, not faked now. The one part worth doing today is a **paper count**
of RPC calls per user action implied by the current keeper and CLI, which is cheap and
predicts the frontend's shape.

---

## 5. Phase 4 scoping — where the brief's numbers need adjusting

The brief asks for 10k user PDAs and 100k open/close cycles in LiteSVM.

LiteSVM holds all account state in memory in-process. 10k `UserAccount` PDAs is fine. 100k
sequential open/close is the risk: each cycle is two transactions with full oracle validation,
and the suite already runs 210 LiteSVM tests in ~14 s. Extrapolating, 200k transactions is
plausibly 20–40 minutes and several GB — on a box that was OOM-killed in August.

**Proposed adjustment:** run it as a *tiered* harness, `#[ignore]`d so it never runs in the
normal suite:

- tier 1 — 1,000 cycles, runs in CI, asserts invariants after every cycle
- tier 2 — 10,000 cycles, invariants every 100
- tier 3 — 100,000 cycles, invariants every 1,000, run manually with `--ignored`

Rounding drift is cumulative and monotonic, so if it exists tier 1 shows the direction and
tier 3 confirms the magnitude. We do not need 100k transactions to *detect* drift — only to
quantify it. This keeps the machine alive and still answers the question.

The audit already found the shape of what to look for: **B-3** (`thirty_day_volume` never
decays) is exactly a cumulative-drift bug that single-user tests missed.

---

## 6. Phase 5 — what only you can do

The load generator needs a running validator with the program deployed.
`scripts/deploy-localnet.sh` deploys via `solana program deploy`
([lines 93-100](../scripts/deploy-localnet.sh#L93-L100)), which is on the deny list and is your
standing rule: **you run deploys, I never do.**

**Recommendation on language: write it in Rust, not TypeScript.** The brief suggests
`scripts/loadgen.ts`. There is no TS project here, so that means building one from scratch
*and* reimplementing every instruction's Borsh encoding, account ordering and PDA derivation in
a second language. the project's Solana rules §10 warns about exactly this — a reimplementation
drifts silently and fails on chain rather than in CI, and the four operator binaries share
`contracts.rs` and `pyth::price_account` via `#[path]` specifically to avoid it.

`crates/solfx-keeper/src/bin/trade.rs` already builds every instruction we need to hammer.
Proposal: **`crates/solfx-keeper/src/bin/loadgen.rs`**, reusing those builders, with
`--users`, `--rate`, `--duration`, `--market` flags. It gets the correct encoding for free and
cannot drift from the CLI.

---

## 7. Phase 6 — a limitation to state up front

LiteSVM executes transactions **sequentially in-process**. It cannot reproduce a genuine
same-slot race, because there is no scheduler and no parallel execution. So:

| Test | Where it can actually run |
|---|---|
| 50 keypairs close the same position | LiteSVM proves *sequential* correctness (first wins, other 49 get a named error). True same-slot behaviour needs the validator. |
| Open and close in the same slot | validator — `min_hold_slots` is a slot-based check |
| Liquidate while owner closes | LiteSVM for ordering; validator for the race |
| Deposit + withdraw simultaneously | LiteSVM for ordering |
| Stale oracle exploitability | LiteSVM — this is a clock question, fully testable in-process |
| Vault drain by any tx ordering | LiteSVM, and this is the valuable one — `assert_invariants()` after every permutation |

The honest framing: LiteSVM answers *"is any ordering unsafe?"*, the validator answers *"does
the runtime actually produce that ordering?"* The first is the security question and I can do
it now. The second needs your validator.

---

## 8. Proposed sequence

1. **Phases 0–3 first** (me, no validator needed). If the ≈500 TPS global-lock finding holds,
   it changes what is worth testing at all — there is no point tuning a load generator to
   probe a ceiling we can derive.
2. **Show you the number and the contention map.** Decide together whether to fix the
   architecture before measuring it, or measure first to prove it.
3. **Phase 4** (me), tiered as in §5.
4. **Phase 6 LiteSVM half** (me) — the vault-drain and ordering tests.
5. **Phase 5 + Phase 6 validator half** — you start the validator, I run the generator.
6. **Phase 8** report.

Phases 0–4 and the LiteSVM half of 6 need nothing from you.

---

## 9. Your guide — the parts only you can run

Everything here is for Phase 5 and the validator half of Phase 6. Do it when we get there,
not now.

### Before you start

```bash
cd .
pgrep -f "[s]olana-test-validator"     # must print nothing
```

The bracket around `s` matters: `pgrep -f solana-test-validator` matches its own shell and
reports a false positive. That is not hypothetical — it misled this session earlier today.

### Step 1 — build everything FIRST, with no validator running

```bash
cargo build --workspace
anchor build
```

Never build while the validator is up. That is what OOM-killed WSL2 on 24 August.

### Step 2 — terminal 1: validator + deploy (leave running)

```bash
./scripts/deploy-localnet.sh
```

This clones the Pyth receiver and Wormhole accounts, then runs `solana program deploy` for
both programs. It takes a couple of minutes.

### Step 3 — terminal 2: initialise

```bash
solana config set --url http://127.0.0.1:8899
solana airdrop 100
./target/debug/init-protocol
```

### Step 4 — terminal 3: price poster (leave running)

```bash
./target/debug/price-poster --rpc-url http://127.0.0.1:8899
```

**Not optional.** Without it every trade fails on `OracleStale` after 60 seconds, and a load
test would measure nothing but the staleness gate.

### Step 5 — terminal 2: run the generator

```bash
./target/debug/loadgen --users 500 --rate 50 --duration 60 --market BTC/USD
```

(That binary does not exist yet — I write it in Phase 5.)

Use **BTC/USD**, not EUR/USD. FX and metals only trade Sunday 21:00 → Friday 21:00 UTC, and
outside that window the protocol correctly refuses every order, so a weekend load test on
EUR/USD measures the session calendar. Only BTC/ETH/SOL are continuous. Check `date -u`.

### Step 6 — afterwards, reclaim memory

```bash
sync && echo 3 | sudo tee /proc/sys/vm/drop_caches >/dev/null
```

A load test leaves several GB of page cache, which is what makes `vmmemWSL` balloon.

### What to tell me

The console output of steps 2–5, especially any error text. If a transaction fails, the error
name is the finding — `AccountInUse` means write-lock contention, a blockhash error means
client-side queueing, and a SolFX error name means we found something single-user tests missed.

---

## 10. Caveat to carry into the report

A single-node `solana-test-validator` has no leader schedule, no gossip, no fee market, no
competing traffic and no block propagation. **Its TPS number is not the network's TPS**, and
must never be quoted as a capacity result. Phase 5 is for failure modes and correctness under
concurrency. The capacity claim comes from Phases 1–3 arithmetic, which is grounded in
consensus-enforced limits rather than one machine's behaviour.
