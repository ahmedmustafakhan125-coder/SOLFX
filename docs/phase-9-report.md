# Phase 9 — report

Evidence for the tasks in [`phase-9-prompt.md`](phase-9-prompt.md). Measurements, not
restatements: where a number appears here, the command that produced it appears with it.

---

## Task 0 — the staleness margin

**Status: diagnosed, and the brief's diagnosis was wrong. Fix designed and measured; not yet
applied, because its acceptance test runs on the VPS.**

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
   further. Each adds a signature (the price update account signs) plus its merkle proof, so
   this is a packet-size question with a measurable answer.

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
