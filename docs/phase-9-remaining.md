# Phase 9 — the two tasks that need your machine

Task 4 (coverage) is being done on the VPS. These two are not: Task 6 needs a browser and a
funded wallet, Task 9 needs `anchor build`. Everything below is copy-pasteable.

---

## Task 6 — the carried-position half of § 12.5

The config half is **already proven**: ETH/USD and SOL/USD were listed with the program's
SHA-256 unchanged either side (`5df771bf773d029d…`). What § 12.5 still owes is steps 3 and 4:

> 3. Trade the new market end to end.
> 4. Confirm every pre-existing position is untouched — same collateral, same entry, same
>    accrued funding.

**The point of step 4 is the one that gets skipped.** It is easy to list a market and see
nothing break. The test is whether a position opened *before* the listing is bit-for-bit
unaffected by trading on it afterwards — that is what "no redeploy, no migration" actually
claims.

### One obstacle first

ETH/USD and SOL/USD are listed but **not posted**, under the decision to return to the six
known-good pairs. A market with no price is `Halted` and cannot be traded, so one of them has
to be posting for the duration of the test. `deployment.task6.json` (BTC + ETH) is already
written on the VPS for exactly this.

### The procedure

**Step 0 — on the VPS, make ETH postable.** Say the word and this is done there; it is a unit
flag and a restart, and it reverts the same way.

```bash
# swaps --deployment to deployment.task6.json, restarts, waits for ETH to reach Active
```

**Step 1 — open the carried position, and record it exactly.** Use the CLI rather than the
browser here, because step 4 needs numbers you can diff, not a screenshot:

```bash
cd . && set -a && . ./.env && set +a
./target/debug/trade open --market BTC/USD --direction long --notional 500 --collateral 100
./target/debug/trade status          # record collateral, entry, size, funding — this is the baseline
```

**Step 2 — trade ETH/USD end to end in the browser.** solfx.cloud/trade, Phantom, open a
small position on ETH/USD and close it. This is the "browser trade" the brief asks for, and it
is the demo-facing half.

**Step 3 — re-read the carried BTC position.**

```bash
./target/debug/trade status
```

**What must be true, and what is allowed to change:**

| Field | Expectation |
|---|---|
| `collateral` | **identical** |
| `entry_notional` | **identical** |
| `size_base` | **identical** |
| accrued funding | **may differ** — funding accrues hourly by design, on every open position |

Funding moving is not a failure; funding moving *because ETH was traded* would be. If the run
straddles an hourly funding crank, note the crank in the report rather than treating the delta
as a discrepancy.

**Step 4 — close the carried position and revert.** Then the VPS goes back to
`deployment.weekend.json` (or `deployment.weekday.json` after the Sunday 21:00 UTC open).

### What to record

In `docs/phase-9-report.md` under Task 6: the two transaction signatures, the before/after
position state, and the program hash you already have. That closes § 12.5 except for the
**synthetic-cross** variant, which no listed market currently exercises — `XAU/EUR` via
`PriceSource::Synthetic { invert_quote }` is the candidate, and it is a separate piece of work.

---

## Task 9 — replace the hand-written referral client

`clients/js/src/referral/` is hand-written because `codama.json` names one IDL and no IDL
account exists on chain for either program (re-checked 2026-09-13 at the canonical Anchor
address; both absent). It works and it is tested, but generated code cannot drift from the
program and hand-written code can.

### The trap, before the commands

`codama.json` renders with **`deleteFolderBeforeRendering: true`** into
`clients/js/src/generated`. Adding the referral program to that config, or pointing it at the
same output directory, **deletes the core client**. It needs its own config and its own
directory.

### Commands

```bash
cd <repo> && git pull

# 1. The IDL. This is the step the VPS cannot do — no anchor CLI there.
ps aux | grep -q "[s]olana-test-validator" || true   # must find nothing before cargo/anchor
anchor build
ls target/idl/solfx_referral.json                    # must exist before continuing

# 2. A SEPARATE codama config. Note the different output directory.
cat > codama.referral.json <<'JSON'
{
  "idl": "target/idl/solfx_referral.json",
  "before": [],
  "scripts": {
    "js": {
      "from": "@codama/renderers-js",
      "args": [
        "clients/js/src/generated-referral",
        { "deleteFolderBeforeRendering": true, "formatCode": true }
      ]
    }
  }
}
JSON

# 3. A second invocation, not a merged one.
#    package.json "generate" becomes:
#      "codama run js && codama run js -c codama.referral.json && npm --prefix clients/js run generate:events"
npm run generate

# 4. Point the SDK at the generated client and delete the hand-written one.
#    clients/js/src/index.ts currently has:
#      export * as referral from "./referral/index.js";
#    It should re-export the generated module instead.
git rm -r clients/js/src/referral

# 5. Verify.
npm --prefix clients/js test        # 198 tests must still pass
npm --prefix clients/js run typecheck
npm --prefix app run ci
```

### The part not to lose

`clients/js/src/__tests__/referral.test.ts` re-derives **every discriminator** from
`sha256("global:<name>")` and `sha256("account:<Name>")`, and asserts the account order of each
instruction. That test is the only thing that has been guarding those constants.

**Move it onto the generated output rather than deleting it with the module it was written
for.** Generated code is derived from the IDL, and an IDL can be stale — the test is what
catches a rename that regenerates cleanly but no longer matches the deployed program. It is
worth more after this change than before it.

### Optional, and worth considering separately

Neither program publishes its IDL on chain, so `anchor idl fetch` returns nothing and
`CONTEXT.md`'s "IDL current at 38 instructions" describes the local file. `scripts/publish-idl.sh`
exists. Publishing costs rent and makes the client generatable by anyone from the chain alone,
which is a real property for a venue that asks people to trust it.
