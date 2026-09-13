# Phase 9 — report

Evidence for the tasks in [`phase-9-prompt.md`](phase-9-prompt.md). Measurements, not
restatements: where a number appears here, the command that produced it appears with it.

---

## Task 0 — the staleness margin

**Status: DONE and proven on devnet, 2026-09-13 07:45-07:55 UTC.** One shared VAA served all
six feeds, confirmed from the on-chain transactions rather than from the poster's own log. The
proof, the cadence and the one trap that made this look broken are in *Proven on devnet* below.

### What the brief concluded, and why it cannot be right

> Every one of those 429s came from `http://127.0.0.1:8899/high` — the local rpc-gateway, not
> Helius.

That conclusion rests on the URL in the poster's error message, and the URL cannot distinguish
the two. [`services/rpc-gateway/gateway.mjs`](../services/rpc-gateway/gateway.mjs) relays the
upstream status code verbatim:

```js
if (upstreamRes.status === 429) stats.upstream429 += 1;
const text = await upstreamRes.text();
res.writeHead(upstreamRes.status, { ... });
```

So a Helius 429 reaches the poster as a 429 *from the gateway*, because the gateway's URL is the
only one the poster ever sees. The gateway's own high-priority queue is 4,096 deep and sheds
almost nothing, which makes gateway-side shedding the less likely of the two explanations
before any further evidence.

**Settled by the gateway's own counters**, which log the two apart. Measured on the VPS and
recorded in commit `e100232`: **`shed=0`, `upstream429=156–181` across both failed
experiments.** Every refusal was Helius, and none was the gateway.

```bash
journalctl -u solfx-rpc-gateway --since -1h | grep -o 'shed=[0-9]* upstream429=[0-9]*'
```

### The actual constraint

**Helius's free tier limits `sendTransaction` to 1 per second**, separately from the plan's 10
req/s general RPC limit. From the Helius rate-limit documentation, via the Solana MCP:

| Endpoint | Free | Developer | Business |
|---|---|---|---|
| general RPC | 10/s | 50/s | 200/s |
| **`sendTransaction`** | **1/s** | 5/s | 50/s |
| `getProgramAccounts` | 5/s | 25/s | 50/s |

That single figure explains all three of the brief's measurements, which is what makes it the
answer rather than a candidate. Each in-flight feed is a chain of 5 sequential transactions, so
at roughly 3 s per confirmation it emits about **0.33 sends/second**:

| Poster setting | Implied send rate | Brief's measured result |
|---|---|---|
| `--concurrency 2` | ~0.67/s — under the limit | 0 rejections, median age 50 s |
| `--concurrency 4` | ~1.33/s — over | 0 clean passes, 155 × 429 |
| `--concurrency 6` | ~2.0/s — over | 0 clean passes, 193 × 429 |

It also explains the margin itself, which no amount of tuning was going to fix. Measured from
the poster's own on-chain transactions (`createAccount` records the VAA length in its `space`
field), **a VAA is 292 bytes**, so it fits one 700-byte write chunk and a feed costs exactly 5
sends. Six feeds is **30 sends per pass**, and at 1/s that is a hard **30-second floor** on the
pass before a single confirmation is waited for. Median age 50 s against a 60 s gate is what
that floor looks like from the outside.

**Consequence: raising the gateway's ceiling — the brief's first option — cannot help.** The
gateway's 9/s was never the binding constraint, and raising it only pushes more sends at a
limit of 1/s.

### The fix, measured

One Hermes request for all six feeds returns **one VAA carrying all six price updates**:

```
ONE request for 6 feeds (EUR/USD, USD/JPY, USD/CNH, XAU/USD, XAG/USD, BTC/USD)
  -> 1 accumulator blob
     blob 0: VAA 292 bytes carrying 6 price updates
  publish_time spread across the six: 0 s
```

So the VAA only needs writing and verifying **once per pass**, not once per feed:

| | sends per pass | floor at 1/s |
|---|---|---|
| today — one VAA per feed | 6 × 5 = **30** | 30 s |
| one shared VAA, six posts | 1 init + 1 write + 1 verify + 6 post + 1 close = **10** | 10 s |

A 3× reduction in the only resource that is actually scarce, on the free tier, with no change
of provider. And because all six updates share one `publish_time`, they land equally fresh
rather than staggered.

This also retires a stale comment in the poster: it says "a 13-signature VAA is ~900 bytes and
does not fit in one instruction alongside its own overhead". At 292 bytes it does, which is why
the five-transaction-per-feed structure is larger than today's VAA requires.

**The sharing is documented, not inferred** (commit `e100232`): `post_update` takes
`encodedVaa` as `isMut: false`, so several calls can share one verified VAA; Pyth describes the
accumulator as one signed Merkle root with per-update proofs; and Helium's production crank
does exactly this. I had recorded it as unproven because the Solana MCP returned no statement
of it — the documentation existed, my search for it did not find it.

Two things still unproven, and both must be before it ships:

1. **Our own five-instruction path**, end to end on localnet. The shape is documented; this
   particular sequence against this program is not yet demonstrated.
2. Whether more than one `post_update` fits in a single transaction, which would cut the floor
   further. **Answered on the VPS, 2026-09-12 18:00 UTC — and the answer is "by twelve bytes,
   no".**

   One Hermes request for the six feeds returned one 2,271-byte `PNAU` blob: **one 292-byte
   VAA carrying six updates**, each with **85 B of message and a 12-hash proof — 326 B of
   calldata per feed**. Computing the transaction from the receiver's own account list:

   | posts per tx | size | |
   |---|---:|---|
   | 1 | 801 B | fits |
   | **2** | **1,244 B** | **12 B over the 1,232 limit** |
   | 3 | 1,687 B | over |

   So the floor is **10 sends, not 7**, unless the transaction shrinks. Twelve bytes is inside
   reach of an **address lookup table**: five of the seven accounts are identical across all
   six posts (`encoded_vaa`, `config`, `treasury`, `system_program`, program id), and an ALT
   replaces each 32-byte key with a 1-byte index — about 155 B saved, enough for two posts and
   possibly three. `.claude/rules/solana.md` § 6 already names ALTs as the escape hatch for
   exactly this. **This is arithmetic with stated assumptions, not a measurement**; at a 1 %
   margin it must be confirmed by building the transaction and reading `serialize().len()`
   before anyone plans around it.

   **10 sends is already the 3× win. Take it first; the ALT is a second, separate 30 %.**

3. **A correction to the "publish_time spread: 0 s" figure above.** Re-measured on Saturday it
   was **75,689 s — about 21 hours**, which is the time since Friday's 21:00 UTC FX close. Both
   readings are right for their conditions: on a weekday all six publish together, at the
   weekend the five session-bound feeds carry Friday's last print while BTC/USD is current. It
   changes nothing about the batching — each `post_update` writes its own feed's
   `publish_time`, so a stale one still halts its own market and no other — but "0 s" should
   not be read as a property of the batch.

### Alternatives, if the fix above is not taken

- ~~**Route the poster's sends off Helius.**~~ **Measured and closed** (commit `6424fa8`):
  **531 s for one six-feed pass** against ~35 s on the Helius path, with **3 of 6 feeds lost to
  "not confirmed in 45s"**. Public devnet serves *reads* fine — the keeper has run on it since
  2026-09-09 — but it has no stake-weighted QoS, so `post_update` confirmations time out. The
  free option is gone.
- **Helius Developer, $49/month**, which takes `sendTransaction` from 1/s to 5/s.
- Chainstack's free tier is 25 RPS general, but its `sendTransaction` limit is unknown and is
  the only number that matters here.

**The acceptance test is unchanged and must run on the VPS:** one hour, asserting zero
`lag_secs >= 60` in the keeper's watchdog output.

---

### Proven on devnet — 2026-09-13 07:45-07:55 UTC

The acceptance question was never "does the poster report success" — it reported `6 posted, 0
failed` while writing one VAA per feed too. It is **whether the six `post_update` transactions
name the same `encoded_vaa` account**. That is a fact about the transactions, so it was read
back off the chain, not off the log.

For each of the six signatures in one pass, `getTransaction` -> the receiver's instruction ->
account index 1 (`encodedVaa`) and index 4 (`priceUpdateAccount`). The account order is
documented, via the Solana MCP: `postUpdate` is `payer, encodedVaa, config, treasury,
priceUpdateAccount, systemProgram, writeAuthority`, and **`encodedVaa` is `isMut: false,
isSigner: false`** — which is what permits the sharing.

```
symbol     encoded_vaa (acct #1)                          price_update (acct #4)
EUR/USD    5T4sFqVe2VbPWEqo3cJL6A8Zod81me3EeZjJvAK7AVFi   1rqo3w8a8X8MkHMavtzxJARwXYUtms2FpYvNEq3MwZ2
USD/JPY    5T4sFqVe2VbPWEqo3cJL6A8Zod81me3EeZjJvAK7AVFi   6REBHVRdJMuVd7D4mUvY285fyrhXeY3iUkkemVmzbFbf
USD/CNH    5T4sFqVe2VbPWEqo3cJL6A8Zod81me3EeZjJvAK7AVFi   DspGeiiYowHYcQ4xvEjPBUZxqrTQbzaDdhbgwsR9JzvM
XAU/USD    5T4sFqVe2VbPWEqo3cJL6A8Zod81me3EeZjJvAK7AVFi   FmV4Do3cEDCEvPrddLRjsgYTfGGhEo5baPv3wxbhZ2mY
XAG/USD    5T4sFqVe2VbPWEqo3cJL6A8Zod81me3EeZjJvAK7AVFi   6KfZXHqMQFS1R4ywAijeekNtrTAkPYkaVcbwezKi9wrt
BTC/USD    5T4sFqVe2VbPWEqo3cJL6A8Zod81me3EeZjJvAK7AVFi   HptpDroAu5BZuWWr8uEKhrhHD6FjQb2JK5yokXyQCzGS

distinct encoded_vaa accounts across 6 post_update txs: 1
```

**One VAA, six feeds, six distinct and correct price accounts.** The predicted 30 -> 10 sends
per pass is what the cadence then shows:

| | measured |
|---|---|
| passes | 8 in 100 s — **~12.5 s per pass** |
| feeds per pass | 6 posted, 0 failed, every pass |
| 429s | **0** across 32 passes |
| headroom against the 60 s gate | **4.8x** |

12.5 s against a 10 s floor at 1 send/second means the pass is now **rate-limit bound, not
code bound** — there is nothing left to win here without changing the send count, which is what
the ALT note above is about. The old 30-send version had a 30 s floor, and its median age of
50 s against a 60 s gate is the same measurement seen from the other side.

### The trap: two posters look exactly like a slow poster

The first devnet run of this measurement read **~44 s per pass**, and the honest first
hypothesis was contention with the VPS poster on the shared Helius key. It was not. **Two
poster processes were running on this machine**, one an orphan of a launch whose log redirect
had failed — the launch reported a pid, so it looked like it had not started when it had.

Both posted to the same six accounts, doubling the send rate against a 1/s limit. Killing the
orphan took the cadence from ~44 s to ~12.5 s with no code change. Check before measuring:

```bash
ps -eo pid,etime,cmd | grep '[d]ebug/price-poster'
```

Note also that `pkill -f "$POSTER"` **kills the shell running it**, because the pattern matches
that shell's own command line — the trap `run-devnet-stack.sh` already documents. Kill by pid.

### Weekend prices are carried forward, and that is documented

At 07:53 UTC on a Sunday the five FX/metals feeds all read `publish_time = Fri 2026-09-11
20:59:59 UTC` — one second before the documented Friday 21:00 close — while BTC/USD read 36 s.
That reads exactly like a broken relayer, and is not one. Two independent checks:

- **`posted_slot` advances on all six accounts** (+206 to +272 slots over 40 s), so the poster
  is writing every account every pass. `publish_time` is the price tick's own timestamp;
  `posted_slot` is when it was posted. Only the latter is evidence about the poster.
- **Pyth documents the behaviour.** From the Pyth Pro FAQ, via the Solana MCP: *"Starting March
  23, 2026, when markets are closed and a fresh aggregate cannot be produced, Pyth Pro will
  carry forward the most recent available price rather than omitting it."*

So a closed market yields a successful post of a stale price, the 60 s gate rejects it, and the
market stays `Halted`. Every layer is behaving correctly. **The consequence for testing: at the
weekend only BTC/USD is exercisable**, and any FX or metals result before Sunday 21:00 UTC is a
statement about the session calendar, not about the protocol.

---

## Task 6 — the extensibility test

**Status: the config half is DONE and proven. ETH/USD and SOL/USD are listed, activated, and
priced, with the program bytes byte-identical either side.** The live-position half is still
owed, and the listing exposed a real operational defect — see *What listing them proved* below.

### Listed 2026-09-13 08:30 UTC — two markets, zero program change

| | |
|---|---|
| ETH/USD | index **9**, feed `ff61491a…fd0ace` |
| SOL/USD | index **10**, feed `ef0d8b6f…80b56d` |
| Program SHA-256 before | `5df771bf773d029dc3cbb641492234218d69fa519ae55ef3e7d483f7ecd78f6b` |
| Program SHA-256 after | `5df771bf773d029dc3cbb641492234218d69fa519ae55ef3e7d483f7ecd78f6b` |
| Program size | 1,107,816 bytes, unchanged |

Both feed ids were resolved from Hermes and cross-checked against Pyth's own sponsored-feed
table via the Solana MCP; both are entitled on the current plan (ETH $2,511.51, SOL $100.50 at
12 s). `solana program dump` either side of the listing gives the **same hash**, which is the
§12.5 claim stated as a measurement: *a market is added by configuration, and the financial core
is not touched.*

The poster picked both up from `deployment.json` on restart and reports **8 posted, 0 failed**,
all eight at `VerificationLevel::Full` with matching feed ids. **One VAA still serves all eight**
(`8fehnHDjHCRFdeXNAC7EYLtnJzH7pjKQK3C9K3MyKGXW`, 8 of 8 `post_update` transactions), so Task 0's
saving does not decay as markets are added — a pass is `3 + n + 1` sends, not `5n`.

### What listing them proved — the map, again

**Within minutes both new markets were `Halted`, with 37-second-fresh prices on chain.** So were
BTC/USD and every other market. This is not a new bug; it is [`solana.md` §5's address
trap](../.claude/rules/solana.md) for the third time:

- This machine's poster writes **this machine's** price accounts, and they are fresh.
- The VPS keeper resolves feeds through **the VPS's** `price-accounts.json`, which has six
  entries and no ETH/USD or SOL/USD at all.
- A keeper that cannot find a live price sets `feed_live` false, and `crank_market_session`
  halts the market. Correctly.

So a market is only tradeable when **one** poster and **one** keeper share **one** map. Adding
a market to the chain is therefore two thirds of the job; the remaining third is operational,
and it is the part that fails silently. **What the VPS needs, in this order:**

1. Regenerate its `deployment.json` for eight markets — the verification pass (no `--activate`)
   is enough, and signs nothing. Note `deployment.json` is gitignored, so `git pull` will
   **not** deliver the new market list.
2. Restart the poster, so it creates the two new price accounts and rewrites
   `price-accounts.json` with eight entries.
3. **Then** restart the keeper, so it loads that eight-entry map. In this order, or the keeper
   reads a six-entry map and halts the two new markets exactly as it does now.

### Two traps in the tooling, found by using it

- **A verification pass rewrites `deployment.json` with only the requested set.** Running
  `--markets ETH/USD,SOL/USD` would have cut the file from six markets to two, and the poster
  and keeper both read it — so the venue would have silently shrunk. Request the **whole**
  intended set, not just the additions. The command used here names all eight.
- **`USD/CNH` is in `STARTER` and not in `ALL`.** Neither table is a superset, so a resolver
  that searches one refuses a symbol the tool can demonstrably list. `select_markets` searches
  both, and an unknown symbol is an error rather than a skip: a typo that listed nothing would
  read as a successful run.

### The earlier reading, and why it still matters

#### Re-measured 2026-09-13 07:42 UTC, after Task 0 was proven

| | |
|---|---|
| BTC/USD #5 (continuous) | **`Active`** |
| The other eight | `Halted` — five of them correctly, FX and metals reopen Sunday 21:00 UTC |

So `feed_live` went true for the continuous market the moment a working poster published to it.
**That is the prediction in this section coming true**, and it settles the causal chain: the
poster was the cause, the keeper was never at fault, and no program change was involved.

What is still owed, and when it can be done:

- **Listing ETH/USD and SOL/USD is doable now** — both are continuous, so neither depends on
  the session calendar. They need the admin wallet, which only this machine holds.
- **The carried position and the browser trade must use BTC/USD** until Sunday 21:00 UTC. It is
  the only exercisable market at the weekend, for the documented reason in Task 0.
- **Task 0's own acceptance test — "one hour with zero `lag_secs >= 60`" — cannot pass before
  Sunday 21:00 UTC**, and this is a defect in the criterion rather than in the poster. Five of
  the six feeds are legitimately stale at the weekend, so the watchdog will report them stale
  no matter how well the poster runs. Either run it after the FX open, or scope it to the
  continuous markets and say so.

### The original diagnosis, 2026-09-13 07:25 UTC, against `api.devnet.solana.com`

| | |
|---|---|
| All nine markets | **`Halted`** |
| Keeper | **alive** — market accounts written seconds before the check, `CrankMarketSession` succeeding |
| BTC/USD #5 (`FeedKind::Crypto`, continuous) | **`Halted`**, which no calendar can explain |

`crank_market_session` leaves `Halted` only when **both** conditions hold
([session.rs:157](../programs/solfx-core/src/instructions/keeper/session.rs#L157)):

```rust
if current == MarketStatus::Halted && !(feed_live && calendar_open) {
    return MarketStatus::Halted;
}
```

For the five FX and metals markets on a Sunday morning, `calendar_open` is correctly false —
they reopen Sunday 21:00 UTC. But **BTC/USD is continuous**, so `calendar_open` is true and
`feed_live` must therefore be false. `crank_market_session` takes no price account (verified
from the accounts in its transaction), so `feed_live` comes from market state that
`crank_market_price` refreshes, and that instruction does need a fresh price. A continuous
market held `Halted` by a healthy keeper therefore points at the **poster**, and only the
VPS's own logs can confirm which.

**What I could not measure from here, and why it matters:** the local `price-accounts.json`
points at accounts last written **2026-09-04 15:41 UTC** — nine days ago, abandoned. So the
"all six feeds are 747,814 s stale" figure those accounts produce describes *my stale map*,
not the venue. The VPS has its own map, and the poster and keeper there must be reading the
same one. I nearly reported that number as the venue's; it would have been true about the
wrong accounts.

**Consequence for Task 6.** It requires a live position carried throughout and a browser trade,
and on a Sunday the only market that could provide either is BTC/USD — which is `Halted`.
Listing ETH/USD and SOL/USD now would also add two markets nothing publishes, reproducing the
"three markets can never price" condition already recorded in `CLAUDE-SESSION.md`. **The
poster has to be working first**, which makes Task 0 a prerequisite for Task 6 rather than a
parallel task.

---


## Task 1 — the baseline

**Status: complete.** Recorded in [`CONTEXT.md`](CONTEXT.md); commit `docs: the real test and
lint baseline`. 555 Rust tests, 198 SDK, 15 app, clippy clean after seven fixes, `npm run ci`
green in `app/` for the first time.

---

## Task 3 — fuzzing

**Status: 24-hour run in progress.** Harness at [`fuzz/solfx_core/src/main.rs`](../fuzz/solfx_core/src/main.rs),
commit `test: a Crucible fuzz harness that asserts I1-I8, not just "no panic"`.

**Tool: `anchor fuzz` (Crucible), not Trident.** The brief asked me to check first, and the
answer changed: Anchor 1.1.2 ships coverage-guided fuzzing with stateful invariant testing,
crash minimisation and LCOV output that Task 4 needs, scaffolded into `fuzz/<program>/` —
outside `programs/`, which this repo forbids touching. Trident is not installed.

```
anchor fuzz run solfx_core invariant_test --release --timeout 86400 -j 4 \
  --corpus-out ~/solfx-fuzz-corpus
```

Started **2026-09-12 17:34:46 UTC**, log at `~/solfx-fuzz-24h.log`, crashes at
`fuzz/solfx_core/crashes/invariant_test/`.

45-second trial before the long run: **0 crashes, 9/9 actions discovered, corpus 145 → 156,
edges 12.3%, branches 23.4%, ~315 exec/sec single-core, 39% of instruction attempts accepted.**
At 4 workers: ~600 exec/sec, 257 MiB. The acceptance figure is the 39%: near 0% would mean a
blocker stopping the fuzzer at the door, near 100% would mean it never reaches a boundary.

The harness asserts **I1, I2, I4, I5 and I8 after every action**, ported from
`tests/common/mod.rs` rather than linked — the fuzz workspace is standalone by design, because
Crucible's dependency graph spans Solana 3.x and 4.x deliberately, so it cannot depend on the
program crate. A port can drift from its original; that is the cost of the isolation, and the
mitigation is that each assertion names the invariant it carries.

Five things had to be measured rather than assumed. Each would have produced a harness that ran
happily and tested nothing:

| | |
|---|---|
| Pyth receiver id | `rec5EKMG…`, taken from the owner of the EUR/USD price account the deployed program reads on devnet. Crucible's mock defaults to a different id. It does produce `VerificationLevel::Full`, which the protocol requires. |
| `FeedKind::Crypto` | A `SpotFx` market is gated by the session calendar and would reject most sequences with `MarketClosedForOpens` before reaching any arithmetic. |
| `base_oi_*` are signed `i128` | Funding moves them in both directions. |
| `#[invariant_test]` hardcodes `fixture` | It splices the body into a generated function with that binding name, so any other parameter name fails to compile against code you did write. |
| Program accounts are not struct fields | Crucible fills anything with a fixed address in the IDL — passing `token_program` is a compile error, not a requirement. |

**Coverage caveat, stated rather than buried:** 12.3% of edges is the whole instrumented
binary, and the harness drives one market through the trader and LP paths. Admin, referral,
trigger and session-crank instructions are not in the action set, so a clean 24 hours is
evidence about the money paths and not about the program as a whole. Task 4's per-crate figures
are what will say where the real gaps are.

---

## Task 2 — the keeper's stale-book guard

**Status: complete, and deployed to the VPS.** Commit `38c845a`.

`book_is_fresh` stands the keeper down whenever the book is older than `max_book_age_secs`,
and only the **slow** loop resets that age — `refresh_prices` updates marks without touching
it. So `refresh_secs >= max_book_age_secs` means the book is stale for part of every cycle by
arithmetic, on a healthy machine with a healthy RPC. Worse than the gap is its regularity: the
crank tick and the refresh are both whole seconds, so once a tick lands inside the stale window
it lands there every cycle rather than drifting out.

Two changes:

- **The defaults were incoherent** — `refresh_secs = 30` against `max_book_age_secs = 20`, so a
  keeper started with *no flags at all* was broken. Now **15 against 45**, the pair the VPS has
  run since 2026-09-09: survives one missed refresh, not two.
- **`Config::validate` refuses an incoherent pair outright**, naming both numbers and two ways
  out. Same reasoning `main` already applies to a missing protocol account — a keeper that
  silently repairs its configuration is a keeper running settings its operator does not know
  about. It can now only fire on values someone passed deliberately.

Five tests; the load-bearing one is `the_defaults_are_coherent`, because the defaults are what
shipped broken. Verified by running the binary rather than only the tests:

```
$ solfx-keeper --refresh-secs 30 --max-book-age-secs 20
Error: --refresh-secs 30 is not shorter than --max-book-age-secs 20, so the book would be
stale for part of every cycle and the keeper would stand down without ever saying why. Give
the tolerance room for at least one missed refresh: --max-book-age-secs 60 or higher, or
--refresh-secs 19 or lower.                                                        exit 1
```

`clippy -p solfx-keeper --all-targets` clean — the test module needed the crate's existing
`#[allow(clippy::expect_used, …)]` block, since the workspace denies `expect_used` and
`expect_err` counts.

---

## Task 9 — replace the hand-written referral client

**Status: blocked on the VPS. Nothing was half-done, because a partial change here breaks
`npm run generate` for everyone.**

What was settled from here:

- **Neither program has an IDL account on chain.** Re-checked 2026-09-13 at the canonical
  Anchor address (`sha256(base ‖ "anchor:idl" ‖ program_id)`): `solfx_referral` →
  `CCXwvvvx…f4SzR`, `solfx_core` → `H94jNXdH…Xb1nho`, **both absent**. So `anchor idl fetch`
  has nothing to return, and `CONTEXT.md`'s "IDL current at 38 instructions" describes the
  local `target/idl/` file rather than anything published. Publishing it is a separate
  decision — `scripts/publish-idl.sh` exists.
- The hand-written client is **still green**: 198 SDK tests, discriminators re-derived.

What blocks it here: **no `anchor` CLI on the VPS and no `target/idl/`**, so the referral IDL
cannot be produced.

**The trap to avoid when you do it locally.** `codama.json` names a single `idl`, and its
renderer args carry `deleteFolderBeforeRendering: true` pointed at
`clients/js/src/generated`. Adding the referral program to that same config, or rendering it
into that same folder, **deletes the core client**. It needs its own config and its own output
directory:

```jsonc
// codama.referral.json
{
  "idl": "target/idl/solfx_referral.json",
  "before": [],
  "scripts": { "js": { "from": "@codama/renderers-js",
    "args": ["clients/js/src/generated-referral",
             { "deleteFolderBeforeRendering": true, "formatCode": true }] } }
}
```

```jsonc
// package.json — a second invocation, not a merged one
"generate": "codama run js && codama run js -c codama.referral.json && npm --prefix clients/js run generate:events"
```

Then delete `clients/js/src/referral/` **in the same commit**, and move its discriminator
re-derivation test onto the generated output — that test is the only thing that has been
guarding those constants, and it should outlive the module it was written for.

---

## Task 3 (scenarios) — complete

**Status: both replays written and passing.** Commit `fa29808`. Nine scenarios now, up from
seven. Full workspace: **562 passed, 0 failed**, clippy clean.

`cargo-build-sbf` is installed on the VPS even though `anchor` is not, so the `.so` for both
programs can be built there and the LiteSVM suites actually **run** rather than being written
blind. Worth knowing — it changes what the VPS can verify.

### COVID, March 2020 — sustained wide confidence

Not a second CHF depeg. That replay tests one dislocation; this tests **persistence**. Twelve
ticks over three weeks, EUR/USD swinging 1.064–1.150 with ~40 bps of confidence throughout —
every price real, none of them certain. The failure mode is the opposite one: not "did the
breaker fire" but "did the venue spend three weeks refusing everything".

Asserts: new risk refused on **every** tick; the existing position untouched by the refusal;
and — the one that matters — the venue **recovers by itself** when confidence narrows, with no
admin action.

**Two things nearly made it a test that proved nothing**, and both are worth recording:

1. It originally asserted only that the opens *failed*. On tick 1 they were failing with
   `OracleDeviationTooLarge`, not the confidence gate the test is named after. It now asserts
   the specific error code on every tick.
2. At the stock `max_deviation_bps = 300` the deviation breaker fires before confidence can
   ever bind, so the market is configured at 1,500 with the reason written next to it. The
   deviation breaker already has its own replay in the GBP flash crash.

### EM devaluation, 2013 taper tantrum

Answers § 12.4's actual question: *whether 10–20x survives a managed-float break.* USD/INR
walks 55 → 68.8 as a **staircase, not a gap** — that is what a devaluation is, and it means
liquidations land rather than all arriving after the fact.

Asserts conservation across the conversion (PnL lands in rupees, so an error there is a units
error), that insurance is drawn before LP capital, and that **bad debt was actually produced** —
a 25% move against EM leverage that produced none would mean the scenario was too gentle.

Three honest failures on the way, all in the test rather than the protocol, which is the
pattern this project keeps finding:

| Failure | What it was |
|---|---|
| `MissingQuoteConversionPriceUpdate` | USD/INR is not USD-quoted; every instruction pricing it needs a conversion account (C-3). The protocol refusing to guess. |
| I1 off by exactly 2,000 USDC | `open_ix` + `send` does not register a position the way `open` does, so the invariant sweep read an untracked one as collateral that left the vault and went nowhere. |
| A patch that changed nothing | The replacement did not match because `cargo fmt` had already reflowed the call it was matching, and the script did not assert on the match. |
