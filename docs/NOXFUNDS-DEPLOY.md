# Deploying the NOXFUNDS upgrade — step by step

The upgrade built and tested on 2026-10-03/04: entry orders, take-profits, movable stops, partial
closes, margin add/remove, two-feed and non-USD pairs in evaluations, and the verified tick.
53 instructions and 48 events, against 35 and 33 live today.

Run on your PC, in WSL, from `/mnt/e/SOLFX`. Only `noxfunds` changes; `solfx-core` and
`solfx-referral` are not touched by any of these commits.

**Numbers measured from the VPS on 2026-10-05**, against public devnet:

| | Value |
|---|---|
| Program | `9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx` |
| Upgrade authority | `7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs` |
| Authority balance | 35.19 SOL |
| Live program space | 1,198,176 bytes, last deployed in slot 503,107,132 |
| New build (VPS) | 1,352,016 bytes, so the account must grow by **153,840 bytes** |
| Cost of that growth | **0.782 SOL**, permanent (`solana rent 153840`) |
| Temporary buffer | **6.869 SOL** while the upgrade uploads, refunded when it completes (`solana rent 1352053`) |
| Open `VirtualPosition` accounts | **0**, which is what makes this upgrade safe (step 2) |

Your build may differ from the VPS build by a few bytes. Where a number below comes from the
build, use the one **your** script prints.

---

## Step 1 — get the code

```bash
cd /mnt/e/SOLFX
git branch --show-current
git status --short
git fetch origin
git merge --ff-only origin/main
git log -1 --format='%h %s'
git diff --stat ORIG_HEAD..HEAD -- programs/solfx-core programs/solfx-referral crates/solfx-math
```

| Command | Expected |
|---|---|
| `git branch --show-current` | `fix/ticket-sizing-and-read-commitment`. Never `main` — your local `main` is the old pre-public history |
| `git status --short` | empty, or only files you know about |
| `git merge --ff-only` | `Fast-forward`, then a list including `programs/noxfunds/src/instructions/eval_orders.rs` |
| `git log -1` | `docs: the NOXFUNDS deploy runbook, and how it works as diagrams` |
| `git diff --stat ... solfx-core ...` | **no output.** Any line here means the exchange changed; stop |

If the merge says `Not possible to fast-forward`, stop and send me the output — do not merge or
rebase by hand.

## Step 2 — the precondition: no open simulated position

`VirtualPosition` grew by 40 bytes. An account created by the old program would no longer load
under the new one, so there must be none on devnet when the upgrade lands.

```bash
curl -s https://api.devnet.solana.com -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,
  "method":"getProgramAccounts","params":["9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx",
  {"dataSlice":{"offset":0,"length":0},"filters":[{"memcmp":{"offset":0,"bytes":"ZyLzBLz1sY1"}}]}]}' \
  | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]))'
```

**Expected: `0`.** (`ZyLzBLz1sY1` is the first 8 bytes of `sha256("account:VirtualPosition")`,
the account's type tag. It printed `0` on 2026-10-05.)

If it prints anything else, someone opened an evaluation trade since. Close it first (the
evaluation's own trader, from `/nox/trader`), then run this again. Do not deploy over it.

Every other account changed only inside its spare `_reserved` bytes, which are zero today and
read as "nothing yet" under the new fields — no other account needs checking.

## Step 3 — the wallet

```bash
solana address
solana balance -u devnet
```

| | Expected |
|---|---|
| `solana address` | `7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs`. If not, the upgrade will be refused; set `SOLFX_KEYPAIR` to the right file |
| `solana balance` | at least **8 SOL** (0.78 kept + 6.87 temporary + fees). It was 35.19 |

## Step 4 — build

```bash
ps aux | grep -q "[s]olana-test-validator" && echo "STOP: a validator is running" || echo "ok"
anchor build
ls -l target/deploy/noxfunds.so
python3 scripts/idl-compare.py clients/js/idl/noxfunds.json target/idl/noxfunds.json
```

| | Expected |
|---|---|
| validator check | `ok`. If it says STOP, shut the validator down first — cargo next to it has OOM-killed WSL |
| `anchor build` | finishes without errors, several minutes |
| `noxfunds.so` | about **1,352,016** bytes |
| `idl-compare.py` | `  on-chain IDL matches the build: 53 instructions` — the wording says on-chain, but here it compares the committed client copy with your build |

If `idl-compare.py` prints `stale:` with a list, **stop and send me the list.** It means your
build's interface differs from the one the website was generated from. Do not regenerate the
client on WSL.

## Step 5 — ask the deploy script what to do

```bash
set -a && . ./.env && set +a
./scripts/deploy-devnet.sh noxfunds
```

Expected (the RPC URL is redacted by the script; your paths will differ):

```
==> noxfunds 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx
    cluster https://devnet.helius-rpc.com/?api-key=<redacted>
    local   1352016 bytes
    onchain 1198176 bytes allocated, authority 7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs
  on-chain account holds 1198176 bytes, smaller than the 1352016-byte build

the build is larger than the allocated account (1352016 > 1198176).
extend first — SIMD-0431 rejects anything under 10240 additional bytes:

  solana program extend "9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx" 153840 -u "$SOLFX_RPC_URL" -k "…/id.json"

deploy the current build. Run:

  solana program deploy "…/target/deploy/noxfunds.so" \
    --program-id "…/target/deploy/noxfunds-keypair.json" -u "$SOLFX_RPC_URL" -k "…/id.json"

then re-run this script — it verifies the bytecode and publishes the IDL.
```

It exits with status 1 here; that is it saying "there is work to do", not an error. If instead
it says `the client's IDL copy does not match this build` or `is older than at least one source
file`, stop — step 4 did not complete.

**Before changing anything, keep a copy of what is live** (a byte-exact record of the old
program, in case anything needs comparing later):

```bash
solana program dump 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx ~/noxfunds-before-2026-10-upgrade.so -u "$SOLFX_RPC_URL"
```

Expected: `Wrote program to …/noxfunds-before-2026-10-upgrade.so`, 1,198,176 bytes.

## Step 6 — extend, then deploy

Copy the two commands **the script printed**. If your build size differed, the extend amount it
printed is the right one, not 153840.

```bash
solana program extend "9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx" 153840 -u "$SOLFX_RPC_URL" -k ~/.config/solana/id.json
solana program show 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx -u "$SOLFX_RPC_URL"
```

Expected: `Extended Program Id 9B7qLbLk… by 153840 bytes`, then `Data Length: 1352016`.

```bash
solana program deploy target/deploy/noxfunds.so \
  --program-id target/deploy/noxfunds-keypair.json -u "$SOLFX_RPC_URL" -k ~/.config/solana/id.json
```

Expected: a progress bar while ~1,300 write transactions upload (a few minutes), then

```
Program Id: 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx

Signature: <a transaction signature>
```

If it fails partway, run the same command again. A failed upload leaves a buffer holding the
~6.87 SOL; `solana program show --buffers -u "$SOLFX_RPC_URL"` lists it and
`solana program close <BUFFER_ADDRESS> -u "$SOLFX_RPC_URL"` returns the SOL. Until the final
`Program Id:` line prints, the old program is still the live one — a failed upload changes
nothing users can see.

## Step 7 — verify, and publish the IDL

```bash
./scripts/deploy-devnet.sh noxfunds
```

Expected:

```
==> noxfunds 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx
    …
    onchain 1352016 bytes allocated, authority 7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs
  bytecode matches: sha256 <64 hex characters>
  0 bytes of zero padding
==> deployed code is the current build — no deploy needed
==> checking the on-chain IDL
  stale: <n> difference(s)
    …
  publishing
  full  …
  slim  …
  kept  53 instructions, 11 accounts, 84 types, 48 events
==> noxfunds 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx on …
==> attempt 1/5: closing any existing IDL metadata account
==> attempt 1/5: writing IDL
==> verifying it reads back
  on-chain IDL matches the build: 53 instructions
==> done
```

`bytecode matches` is the line that proves the deploy. The IDL write is known to be flaky on
devnet (about one attempt in two fails); the script retries up to five times. `failed; retrying`
lines before a success are normal.

Run it once more:

```bash
./scripts/deploy-devnet.sh noxfunds
```

Expected last line: `==> everything is current`.

## Step 8 — tell me

Send "deployed" and the output of:

```bash
solana program show 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx -u "$SOLFX_RPC_URL"
```

Expected: `Last Deployed In Slot` higher than 503107132, `Data Length: 1352016`, `Authority`
unchanged.

I then check it independently from the VPS (slot, data length, on-chain IDL) and do the server
side — none of this is needed on your PC:

1. rebuild and restart the keeper, so it fills entry orders, fires targets and sweeps settled
   mandates (`journalctl -u solfx-keeper` should show `noxfunds` lines with no decode errors);
2. `npm run build` in `app/`, which publishes the new trading desk to solfx.cloud — only now,
   because before the upgrade the site would send instructions the program does not know;
3. install the indexer and point the site's container at it ([OPERATIONS.md](OPERATIONS.md),
   "The NOXFUNDS indexer").

## Step 9 — try it in the browser

After step 8, on solfx.cloud with a devnet wallet. BTC/USD is the only market open at weekends.

| Do | Expect |
|---|---|
| `/nox/trader` → start an evaluation | $50 USDC leaves your wallet; Phase 1, balance as chosen |
| Open BTC/USD with a stop | the position appears with live P&L; a trade without a stop is refused |
| Place a **limit** buy well below the price | it shows under resting orders; nothing opens |
| Place a limit buy just above the price | within ~30 s the keeper fills it (the crank runs every 30 s) |
| Set a take-profit, then try to move the stop **away** from the price | the move is refused (`StopNotTighter`); tightening works after 10 minutes |
| Cancel a resting order | its rent comes back to you |
| `/nox/market` | traders with a passed evaluation show the verified tick |

## If something goes wrong

| Symptom | Meaning | Do |
|---|---|---|
| step 2 prints non-zero | someone has an open evaluation trade | close it first; never deploy over it |
| `idl-compare` says `stale` in step 4 | your build's interface differs from the website's | stop, send me the diff |
| `Error: … upgrade authority` | wrong wallet | check `solana address` (step 3) |
| `insufficient funds` | not enough SOL for the buffer | top up to ≥ 8 SOL; close any leftover buffer |
| deploy upload fails partway | devnet dropped writes | re-run the deploy; reclaim buffers afterwards |
| step 7 says `bytecode differs` | the upgrade did not land, or `anchor build` ran again after it | re-run step 5 and follow what it prints |
| IDL write fails all 5 attempts | devnet flakiness | re-run `./scripts/publish-idl.sh noxfunds`; the program itself is unaffected |
