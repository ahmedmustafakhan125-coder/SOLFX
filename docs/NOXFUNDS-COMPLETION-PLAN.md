# NOXFUNDS — completion plan

**Written:** 2026-10-03. **Scope:** everything listed as missing in
[`NOXFUNDS.md` §10](NOXFUNDS.md#10-what-is-not-built), plus the trader-experience gaps found by
reading the code, **except an external audit**. Status of each checkpoint is kept in the table
at the bottom and updated as work lands.

Every design decision below names its source: a file in this repository, a measurement, or a
Solana MCP result. Where the MCP had nothing, that is said.

---

## 0. Ground rules this plan is built on

| Rule | Why | Source |
|---|---|---|
| `solfx-core` is not modified. Every new capability is a NOXFUNDS wrapper over an instruction `solfx-core` already has. | Hard constraint #1 | `CLAUDE.md` |
| Program changes go only into `programs/noxfunds/`. Claude never deploys. | Hard constraint #2 | `CLAUDE.md` |
| No new account field moves an existing account's offsets. New fields are carved from `_reserved`. | Anchor deserialises Borsh sequentially; an old, shorter account fails to load. New fields go into reserved padding; growing needs a migration. | MCP: Anchor *Account Types* (`Migration<From, To>` + `realloc`), Drift *Account Model* ("new fields normally go into reserved padding") |
| The one exception, `VirtualPosition`, has no `_reserved`. It may grow **only while no `VirtualPosition` exists on devnet**. | Checked 2026-10-03: **0** `VirtualPosition` accounts on devnet (2 `Evaluation`, 6 `Mandate`, 3 `TraderProfile`). Re-check immediately before the deploy. | `getProgramAccounts` with a discriminator `memcmp`, this session |
| Clients never filter `getProgramAccounts` by `dataSize`. | A size filter silently matches nothing once a type grows. Verified: no client in this repo does. | MCP: Drift *Account Model*, "Decode length-tolerantly" |
| A resting order is fired by a keeper against the oracle price, then re-checked against every rule at fill. | The standard on-chain limit-order pattern: an on-chain request, keeper-executed when the oracle crosses. | MCP: Jupiter Perps *Limit Orders*; Drift *Order types* ("triggering and filling are two separate steps") |
| Events stay `emit!`, not `emit_cpi!`. The indexer detects truncated logs and falls back to account state. | `emit_cpi!` is more durable but adds two accounts and CPI compute to **every** instruction; `funded_open_position` is already at 21 accounts / 117k CU. Truncation is the known `emit!` risk, so it is detected rather than assumed away. | MCP: *Anchor Events* (solana.com), "emit! … logs may be truncated; emit_cpi! … more durable, extra accounts and compute" |
| SOL is returned from the dataless mandate signer by a System Program transfer signed with its seeds. | The documented pattern for a `SystemAccount` PDA. A data-bearing PDA would need direct lamport mutation instead. | MCP: *CPIs with PDA Signers* (solana.com); Chainstack *PDAs and CPIs* |

---

## 1. Checkpoints

Each checkpoint lists the change, its tests, and what "done" means. Checkpoints are ordered so
that anything shippable without a program upgrade comes first.

### CP0 — Baseline

- Build `noxfunds` tests, record the passing count before any change.
- Confirm the IDL can be rebuilt without the `anchor` CLI (from `cargo test --features idl-build`
  fragments with `ANCHOR_IDL_BUILD_RESOLUTION=TRUE`, the method commit `8e083a7` used).
- **Done when:** the baseline number is written in the status table.

### CP1 — The trader screen (frontend only; no program change)

| # | Change | Unit tests (vitest) |
|---|---|---|
| 1a | Evaluation ticket: **Long / Short**. The program already accepts both (`evaluation.rs:466-468`); only the page refused. | side of a stop for each direction |
| 1b | Stop entered **as a price** or as a distance, with the other derived. Risk-at-stop shown live against the 1% rule before signing. | price↔bps round trip; risk rounds up |
| 1c | **Live P&L** per simulated position and an **estimated equity** with progress to target, from the live oracle price, labelled as an estimate (the exact figure is the program's at close). | P&L sign for long/short; estimate never shows a stage as passed early |
| 1d | **Chart in the trader view.** The terminal's `PriceChart` and `MarketPanel` embedded on `/nox/trader`, the chart following the ticket's market, with entry / stop / target lines for evaluation and funded positions. | level builder: one line per order, right kind |
| 1e | **Verified tick ✓** beside any trader with an `Evaluation` in state `Passed`, on listings, offers, requests and the profile strip. Derived from the evaluation accounts, which only the program can write. After CP2d, the profile's own counter is read as well. | derivation: passed→✓, failed/active→none, other trader's pass does not leak |

**Done when:** `npx tsc -b && npx vitest run && npx eslint . && npx prettier --check .` are clean in `app/`.

### CP2 — Evaluation trading (program: `noxfunds`)

| # | Instruction / change | Rule, and why |
|---|---|---|
| 2a | `VirtualPosition.take_profit_price` (0 = none) + `_reserved: [u8; 32]` for the next change. `eval_set_take_profit(price)` — trader; `0` clears. `eval_trigger_take_profit` — **permissionless**. | Placed only on the profit side of the current price (same `is_placeable` test SolFX uses). Fires on the oracle price (`TriggerKind::TakeProfit.is_met`), **and only once the 10-minute hold has passed**: a take-profit is a chosen exit, so it counts as a voluntary close and joins the hold average. Otherwise a target one tick away is a way round the hold rule. |
| 2b | `eval_move_stop(new_stop)` — trader. | **Tighten only** (long: up, short: down), so risk can only fall and no rule needs re-pricing. Not already met at the current price. **Only after the 10-minute hold**: stop-outs are exempt from the hold, so a stop that could be dragged to the price in minute one would be an exit that dodges the rule. |
| 2c | Entry orders: `EvalEntryOrder` at `["eorder", evaluation, order_id]`. `eval_place_entry_order` (trader), `eval_cancel_entry_order` (trader any time; anyone once expired or the evaluation has ended), `eval_fill_entry_order` (**permissionless**). `Evaluation.pending_orders` carved from `_reserved`. | Kinds: **Limit** (buy at or below / sell at or above) and **Stop** (buy at or above / sell at or below), judged on the oracle price. At fill, **every** open rule runs again through the same function `eval_open_position` uses: price, spread, leverage, risk ≤ 1%, daily loss, drawdown, max open. A fill that would break one fails, and the order waits. At most 5 pending. `claim_stage_pass` refuses while any are pending, so an order placed in Phase 1 cannot fill into Phase 2. The keeper pays the new position's rent and receives the order's, which is sized to cover it. |
| 2d | **Verification on chain.** `TraderProfile.evaluations_passed: u32`, `last_passed_at: i64`, carved from `_reserved`. `claim_stage_pass` takes the profile and increments on the Phase 2 pass. New event `TraderVerified`. | Gives the ✓ an on-chain counter a program can gate on later. It **does not** gate funding: the request was a mark, not a requirement. |

**Tests:** LiteSVM, `programs/noxfunds/tests/stage8.rs`. Every rule above has its refusal test
(wrong side, early, loosening, not met, pending blocks claim, stranger cancel before expiry) as
well as its success test. Shorts get a full round trip. Budgets for every new instruction are
asserted in `budgets.rs`. Property tests (`proptest`) for the pure predicates: tighten-only, entry-trigger
symmetry long↔short, and "risk at stop never decreases when the stop moves away".

### CP3 — Funded trading (program: `noxfunds`)

| # | Change | Rule |
|---|---|---|
| 3a | `funded_move_stop(old_order_id, new_order_id, new_stop)`. CPI `place_trigger_order` for the new stop, then `cancel_trigger_order` for the old, in one instruction. | Tighten only; only after `min_hold_slots`; the old order must be this position's `StopLoss`. The position is never without a stop, even inside the instruction. |
| 3b | **Fix:** `funded_place_take_profit` refuses before `min_hold_slots` has passed. | The take-profit fires inside `solfx-core` (`execute_trigger_order`), where NOXFUNDS' hold rule is never consulted. A target one tick from the price, placed at open, was an exit before the investor's minimum hold. Found by reading `trading.rs` against `trigger.rs`. On devnet every mandate has `min_hold_slots = 0`, so nothing live changes. |
| 3c | Funded entry orders: `MandateEntryOrder` at `["morder", mandate, order_id]`; place (trader), cancel (trader; anyone when expired or the mandate is no longer `Active`), fill (**permissionless**: full `check_rules` at fill, then the same two CPIs `funded_open_position` makes: open and stop). | Same rules as 2c, applied by the code that already guards a market order. The fill's CU and account count are measured and asserted. If it does not fit, this is reported, not forced. |
| 3d | `funded_reduce_position` (partial close via `decrease_position`). | Measured, not recomputed: the change in free collateral and in the position's margin and fee is the venue's own figure. Booked notional is released pro rata, **rounded down**, so more stays booked (adverse to the trader). The result folds into the mandate's `realized_pnl` and the profile's gross profit/loss **without counting a trade**, so splitting one position into ten closes cannot inflate the trade count tiers are decided on. |

**Tests:** LiteSVM `stage8.rs`/`stage9.rs`, budgets, and `assert_invariants()` after every
funded path, because these touch SolFX vaults.

### CP4 — Housekeeping (program: `noxfunds`)

| # | Change | Rule |
|---|---|---|
| 4a | `sweep_mandate_signer` — permissionless once the mandate is `Settled` and its SolFX account holds no positions. Sends the signer's lamports to the **trader**. | The trader's dashboard is what tops the signer up (`prepareMandateIxs`, `app/src/lib/nox.ts`). |
| 4b | `set_treasury`, `set_guardian` (admin); `propose_admin` / `accept_admin` (two-step; `pending_admin` carved from `NoxConfig._reserved`). | Rotation used to need a program upgrade (`NOXFUNDS.md` §11). Two-step so a typo cannot hand the protocol to nobody. |

### CP5 — Client, CLI and keeper

- IDL rebuilt from fragments; Codama regenerated; the discriminator test re-derives every new
  instruction, account and event from `sha256("<ns>:<name>")[..8]`.
- `app/src/lib/nox.ts` builders for every new instruction, with unit tests pinning account
  order against the IDL.
- Keeper (`nox_crank.rs`): fire evaluation take-profits; fill evaluation and funded entry
  orders; clean up expired ones; sweep settled signers. A pure `plan_*` function for each,
  unit-tested like the existing `plan_evaluation`.
- UI for 2a–2c and 3a–3d on `/nox/trader`.

### CP6 — The indexer ("subgraph")

The MCP has **no documentation** on The Graph or Substreams for Solana, and nothing that says a
hosted subgraph serves devnet. So this is not built on The Graph. What the MCP did supply is the
standard self-hosted pattern: backfill with `getSignaturesForAddress` + `getTransaction`, then
keep up by polling or streaming (Helius, *How to Index Solana Data*).

`services/indexer/` — Node 22, no new dependencies (`node:sqlite`, `fetch`):

- **Ingest:** page `getSignaturesForAddress(noxfunds)` backwards to genesis, then forwards from the
  last seen signature every 15 s at `confirmed`. `getTransaction` each, decode `Program data:`
  lines with the existing generated event decoders (`clients/js/src/nox/events.generated.ts`).
- **Store:** an append-only `events` table keyed by `(signature, index)`, so a replay is
  idempotent, and derived tables `traders`, `evaluations`, `mandates`, `trades`.
- **Truncation:** a transaction whose logs contain `Log truncated` is flagged, and the trader it
  touches is marked as needing an account-state cross-check rather than silently undercounted.
- **API:** `GET /api/nox/traders`, `/api/nox/traders/:address` (record, ✓, evaluations,
  mandates, every trade), `/api/nox/health` (last slot, lag, truncated count).
- **Verification:** the same 15-figure comparison `/nox/verify` runs, served per trader, so the
  indexed record is checked against `TraderProfile` on every refresh.
- **Tests:** vitest on the reducer with recorded event fixtures; a property that replaying the
  same events twice, or in two batches, yields the same tables.

### CP7 — Documentation

`NOXFUNDS.md` §5, §8, §9, §10 updated to what is now true; `noxfunds-budgets.md` gains every new
instruction's measured figures; `OPERATIONS.md` gains the indexer and the deploy preconditions.

### CP8 — What only the user can do

| Step | Why it is not Claude's |
|---|---|
| `anchor build` + upgrade `noxfunds` on devnet, publish the IDL | Deploys are the user's, by rule. Preconditions: no `VirtualPosition` on devnet; no mandate with an open position. |
| Restart `solfx-keeper`, start the indexer, `npm run build` in `app/` | Only **after** the upgrade lands, or the site sends layouts the live program misreads. |
| Pass an evaluation on devnet | Needs ten trades over **five distinct UTC days**. Calendar time, not code. |
| Run the investor-listing → funding-request path on devnet | Needs two funded wallets acting as the two sides. |
| Move the upgrade authority to a multisig | A custody decision about the user's keys. |
| Decide how the stake scales with evaluation size | `NOXFUNDS-PLAN.md` says it rises and never says by how much. That is a pricing decision, not something to invent. |

### Not in scope

- **External audit.** Excluded by request.
- **The `nox` CLI for the new instructions.** It sizes from one USD price and exists to replay
  the lifecycle reproducibly; the browser and the keeper cover every new instruction. Its
  evaluation calls were updated for the new account lists and nothing else.

---

## 2. Status

| CP | Item | Status | Evidence |
|---|---|---|---|
| 0 | Baseline | **done** | `noxfunds` 165 pass / 0 fail; app 94; client 239; `cargo build-sbf` reproduces the current `.so` |
| 1 | Trader screen | **done** | `app/src/lib/noxdesk.ts` + 46 vitest cases; app 140/140; `tsc`, `eslint`, `prettier` clean. Not rendered in a browser — none is installed on this host |
| 2 | Evaluation trading | **done** | `eval_orders.rs` (6 instructions), `claim_stage_pass` records the pass on the profile. `tests/stage8.rs` 21 LiteSVM tests incl. budgets; 3 proptest properties; `noxfunds` 192/192 (was 165); clippy clean; `program_autofixer` 0 issues on every changed file |
| 3 | Funded trading | **done** | `funded_move_stop`, TP hold gate, funded entry orders, `funded_reduce_position`. `tests/stage9.rs` 18 LiteSVM tests incl. two balance-to-the-unit identities and budgets (fill 113–121k CU / 892 B / 23 accounts; move 45–52k; reduce 85k); `noxfunds` 214/214; `program_autofixer` 0 issues |
| 4 | Housekeeping | **done** | `sweep_mandate_signer`, `set_guardian`, `set_treasury`, `propose_admin` / `accept_admin`. `tests/stage10.rs` 4 tests; `noxfunds` 218/218; `program_autofixer` 0 issues |
| 5 | Client, keeper | **done** | IDL rebuilt with Anchor's own `IdlBuilder` (33 unchanged instructions byte-identical, error codes preserved); Codama regenerated; client 243/243; app 161/161 with builders + UI for every new instruction; keeper fires targets, fills and clears entry orders, sweeps settled signers (keeper tests pass); workspace `cargo test` 837/0 before these, clippy clean |
| 6 | Indexer | **done** | `clients/js/src/nox/indexer.ts` (11 vitest cases: idempotence, order, replay, resume) + `services/indexer/` service and unit file + `/api/nox/*` route. Devnet smoke run 2026-10-03: 515 transactions, 485 events, 0 truncated; **51 of 51** profile figures agree with the live accounts across all 3 traders |
| 7 | Docs | **done** | `NOXFUNDS.md` §8, §10, §11 and a "Changes not yet deployed" section; `noxfunds-budgets.md` gains every new measurement; `OPERATIONS.md` gains the keeper actions, the indexer, and the deploy runbook with a working precondition command (0 `VirtualPosition` on devnet, re-run 2026-10-03) |
| 9 | Follow-ups, 2026-10-04 | **done** | `funded_add_margin` / `funded_remove_margin` (3 tests incl. balance-to-the-unit through a stop-out; 66.5k / 71.6k CU); every market shape in evaluations — optional legs on all 7 evaluation instructions, variable-shape crank (`stage11.rs`, 3 tests); keeper passes legs by market configuration (+1 test); browser prices every shape: `legFeeds`, `marketQuote`, conversion and synthetic arithmetic ported with the program's rounding (10 vitest cases, cross-checked against `stage11.rs` figures); IDL rebuilt again (7 changed, 2 added, nothing else moved); `noxfunds` 225/225, app 172, client 254 |
| 8 | User steps | **yours** | see CP8 above: deploy, restart keeper, build the app, install the indexer, pass an evaluation over 5 days, run the investor-listing path, multisig, stake schedule |
