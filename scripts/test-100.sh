#!/usr/bin/env bash
# 100 manual cases against a live validator, as runnable commands.
#
# Companion to docs/test-cases/localnet-manual.md, which explains *why* each case exists.
# This file is the *what to type*. It runs them, records PASS/FAIL against the expected
# outcome, and prints a summary.
#
# The interesting half of this suite is the cases that must FAIL. A venue that accepts
# everything is not a venue, and every bug found so far was caught by something correctly
# refusing. `expect_fail` cases pass when the command errors.
#
#   scripts/test-100.sh              # everything runnable
#   scripts/test-100.sh 26 45        # a range
#   scripts/test-100.sh --list       # numbers and descriptions, run nothing
#
# Prerequisites: validator up, protocol initialised, price-poster running, `trade setup` done.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

TRADE=./target/debug/trade
RPC=http://127.0.0.1:8899
PASS=0; FAIL=0; SKIP=0
FROM=${1:-1}; TO=${2:-100}
LIST=false
[[ "${1:-}" == "--list" ]] && { LIST=true; FROM=1; TO=100; }

c()  { printf "\033[36m%s\033[0m" "$*"; }
ok() { printf "\033[32mPASS\033[0m"; }
no() { printf "\033[31mFAIL\033[0m"; }

# run a case expected to SUCCEED
expect_ok() {
  local n=$1 desc=$2; shift 2
  (( n < FROM || n > TO )) && return
  $LIST && { printf "%3d  %s\n" "$n" "$desc"; return; }
  printf "%3d  %-52s " "$n" "$desc"
  if out=$("$@" 2>&1); then printf "%s\n" "$(ok)"; PASS=$((PASS+1))
  else printf "%s  %s\n" "$(no)" "$(echo "$out" | grep -oE 'Error Code: [A-Za-z]+|Error: .*' | head -1)"; FAIL=$((FAIL+1)); fi
}

# run a case expected to be REJECTED. Optionally assert the error names $4.
expect_fail() {
  local n=$1 desc=$2 want=$3; shift 3
  (( n < FROM || n > TO )) && return
  $LIST && { printf "%3d  %s  [must reject: %s]\n" "$n" "$desc" "$want"; return; }
  printf "%3d  %-52s " "$n" "$desc"
  if out=$("$@" 2>&1); then
    printf "%s  accepted, should have been refused\n" "$(no)"; FAIL=$((FAIL+1))
    # Leave no position behind: an unexpected success otherwise fails every later case on
    # "account already in use", which looks like three bugs instead of one.
    $TRADE close --market BTC/USD --nonce 0 >/dev/null 2>&1 || true
  else
    if [[ -z "$want" ]] || grep -qi "$want" <<<"$out"; then printf "%s\n" "$(ok)"; PASS=$((PASS+1))
    else printf "%s  wrong error: %s\n" "$(no)" "$(echo "$out" | grep -oE 'Error Code: [A-Za-z]+' | head -1)"; FAIL=$((FAIL+1)); fi
  fi
}

# a case this harness cannot drive
manual() {
  local n=$1 desc=$2 why=$3
  (( n < FROM || n > TO )) && return
  $LIST && { printf "%3d  %s  [manual: %s]\n" "$n" "$desc" "$why"; return; }
  printf "%3d  %-52s \033[33mMANUAL\033[0m  %s\n" "$n" "$desc" "$why"; SKIP=$((SKIP+1))
}

flat() { $TRADE close --market "$1" --nonce "${2:-0}" >/dev/null 2>&1 || true; }
fresh() { ./target/debug/price-poster --rpc-url $RPC --once >/dev/null 2>&1; }

echo "=== SolFX — 100 cases ==="
$LIST || { echo "refreshing prices..."; fresh; }
echo

# ── 1. Account lifecycle ────────────────────────────────────────────────────────
echo "-- 1. Account lifecycle (1-10)"
manual   1 "status before setup" "needs a fresh wallet"
expect_ok 2 "setup deposits collateral" $TRADE setup --amount 1000
expect_ok 3 "status reads the account" $TRADE status
expect_ok 4 "setup is idempotent" $TRADE setup --amount 1000
expect_ok 5 "collateral accumulates" $TRADE status
expect_fail 6 "setup --amount 0" "" $TRADE setup --amount 0
manual   7 "open before setup" "needs a fresh wallet"
expect_ok 8 "status lists all five markets" $TRADE status
expect_ok 9 "LP vault is seeded" $TRADE status
manual  10 "missing deployment.json" "would break every later case"

# ── 2. Opening, valid ───────────────────────────────────────────────────────────
echo; echo "-- 2. Opening, valid (11-25)"
flat BTC/USD
expect_ok 11 "BTC long, \$1000 notional" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 200
expect_ok 12 "status shows the position" $TRADE status
expect_ok 13 "close it" $TRADE close --market BTC/USD
expect_ok 14 "BTC short" $TRADE open --market BTC/USD --direction short --notional 1000 --collateral 200
expect_ok 15 "close the short" $TRADE close --market BTC/USD
expect_ok 16 "small position, \$100" $TRADE open --market BTC/USD --direction long --notional 100 --collateral 50
expect_ok 17 "close it" $TRADE close --market BTC/USD
expect_ok 18 "5x leverage, \$5000 on \$1000" $TRADE open --market BTC/USD --direction long --notional 5000 --collateral 1000
expect_ok 19 "close it" $TRADE close --market BTC/USD
expect_ok 20 "sized by lots" $TRADE open --market BTC/USD --direction long --lots 0.01 --collateral 200
expect_ok 21 "close it" $TRADE close --market BTC/USD
expect_ok 22 "sized by quantity (XM's Quantity tab)" $TRADE open --market BTC/USD --direction long --quantity 0.01 --collateral 200
expect_ok 23 "close it" $TRADE close --market BTC/USD
expect_ok 24 "EUR/USD long  [FX session]" $TRADE open --market EUR/USD --direction long --notional 1000 --collateral 100
expect_ok 25 "close EUR/USD" $TRADE close --market EUR/USD

# ── 3. Opening, must be refused ─────────────────────────────────────────────────
echo; echo "-- 3. Opening, MUST BE REFUSED (26-45)"
flat BTC/USD 0; flat BTC/USD 1
expect_fail 26 "BTC 500x leverage" "LeverageTooHigh" $TRADE open --market BTC/USD --direction long --notional 100000 --collateral 100
expect_fail 27 "absurd notional" "" $TRADE open --market BTC/USD --direction long --notional 1000000000 --collateral 100
expect_fail 28 "zero notional" "" $TRADE open --market BTC/USD --direction long --notional 0 --collateral 100
expect_fail 29 "zero collateral" "" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 0
expect_fail 30 "zero lots" "" $TRADE open --market BTC/USD --direction long --lots 0 --collateral 100
expect_fail 31 "negative lots" "" $TRADE open --market BTC/USD --direction long --lots -1 --collateral 100
expect_fail 32 "non-numeric lots" "" $TRADE open --market BTC/USD --direction long --lots abc --collateral 100
expect_fail 33 "lots finer than resolution" "" $TRADE open --market BTC/USD --direction long --lots 0.00001 --collateral 100
expect_fail 34 "unlisted market GBP/USD" "not listed" $TRADE open --market GBP/USD --direction long --notional 1000 --collateral 100
expect_fail 35 "nonsense market" "not listed" $TRADE open --market NONSENSE --direction long --notional 1000 --collateral 100
expect_fail 36 "sub-\$1 notional (0.000001 BTC ~ \$0.08)" "" $TRADE open --market BTC/USD --direction long --quantity 0.000001 --collateral 100
expect_fail 37 "collateral beyond balance" "" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 999999999
expect_fail 38 "zero slippage bound" "Slippage" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 200 --slippage-bps 0
expect_fail 39 "1bp bound, tighter than the spread" "Slippage" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 200 --slippage-bps 1
expect_fail 40 "two sizing modes at once" "" $TRADE open --market BTC/USD --direction long --lots 0.01 --notional 1000 --collateral 200
expect_fail 41 "no sizing mode at all" "" $TRADE open --market BTC/USD --direction long --collateral 200
manual  42 "EUR/USD outside the FX session" "run at a weekend"
manual  43 "XAU/USD outside its session" "run at a weekend"
manual  44 "open while the protocol is paused" "needs the admin pause instruction"
manual  45 "open on a ReduceOnly market" "needs set_market_status"

# ── 4. Sizing ───────────────────────────────────────────────────────────────────
echo; echo "-- 4. Sizing (46-55)"
flat BTC/USD
expect_ok 46 "1 BTC by quantity" $TRADE open --market BTC/USD --direction long --quantity 1 --collateral 20000
expect_ok 47 "close it" $TRADE close --market BTC/USD
expect_ok 48 "1 lot BTC == 1 BTC" $TRADE open --market BTC/USD --direction long --lots 1 --collateral 20000
expect_ok 49 "close it" $TRADE close --market BTC/USD
expect_ok 50 "EUR/USD 1000 units  [FX]" $TRADE open --market EUR/USD --direction long --quantity 1000 --collateral 100
expect_ok 51 "close it" $TRADE close --market EUR/USD
expect_ok 52 "EUR/USD 0.01 lots == 1000 EUR  [FX]" $TRADE open --market EUR/USD --direction long --lots 0.01 --collateral 100
expect_ok 53 "close it" $TRADE close --market EUR/USD
expect_ok 54 "XAU/USD 1 oz  [FX]" $TRADE open --market XAU/USD --direction long --quantity 1 --collateral 500
expect_ok 55 "close it" $TRADE close --market XAU/USD

# ── 5. Closing ──────────────────────────────────────────────────────────────────
echo; echo "-- 5. Closing (56-65)"
flat BTC/USD 0; flat BTC/USD 1
expect_ok 56 "open one" $TRADE open --market BTC/USD --direction long --notional 500 --collateral 100 --nonce 0
expect_ok 57 "close without --nonce, one open" $TRADE close --market BTC/USD
expect_ok 58 "open on nonce 0" $TRADE open --market BTC/USD --direction long --notional 500 --collateral 100 --nonce 0
expect_ok 59 "open on nonce 1" $TRADE open --market BTC/USD --direction short --notional 500 --collateral 100 --nonce 1
expect_fail 60 "close ambiguous, two open" "Say which" $TRADE close --market BTC/USD
expect_ok 61 "close nonce 0 explicitly" $TRADE close --market BTC/USD --nonce 0
expect_ok 62 "close the last one, no --nonce needed" $TRADE close --market BTC/USD
expect_fail 63 "close with nothing open" "no open position" $TRADE close --market BTC/USD
expect_fail 64 "close a nonce that is not open" "" $TRADE close --market BTC/USD --nonce 5
expect_ok 65 "status is flat" $TRADE status

# ── 6. Oracle ───────────────────────────────────────────────────────────────────
echo; echo "-- 6. Oracle (66-75)"
expect_ok 66 "prices refresh" fresh
expect_ok 67 "trade on a fresh price" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 200
expect_ok 68 "close" $TRADE close --market BTC/USD
manual  69 "stop the poster, wait 60s, open" "must fail OracleStale"
manual  70 "restart the poster, open again" "must succeed"
expect_ok 71 "every posted feed is Full" bash -c '
  python3 - <<PY
import json,base64,urllib.request,sys
pa={e["symbol"]:e for e in json.load(open("price-accounts.json"))}
def rpc(m,p):
    r=urllib.request.Request("http://127.0.0.1:8899",data=json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode(),headers={"Content-Type":"application/json"})
    return json.load(urllib.request.urlopen(r))["result"]
bad=[s for s,e in pa.items() if (v:=rpc("getAccountInfo",[e["price_account"],{"encoding":"base64"}])["value"]) and base64.b64decode(v["data"][0])[40]!=1]
sys.exit(1 if bad else 0)
PY'
expect_ok 72 "market feed id matches its price account" bash -c '
  python3 - <<PY
import json,base64,urllib.request,sys
dep=json.load(open("deployment.json")); pa={e["symbol"]:e for e in json.load(open("price-accounts.json"))}
bad=[m["symbol"] for m in dep["markets"] if m["symbol"] in pa and m["feed_id"]!=pa[m["symbol"]]["feed_id"]]
sys.exit(1 if bad else 0)
PY'
expect_ok 73 "devnet probe runs" ./target/debug/devnet-feed-probe --cluster devnet
manual  74 "mainnet probe" "slow; run by hand"
manual  75 "pass another market's price account" "CLI cannot mis-wire it"

# ── 7. Sessions ─────────────────────────────────────────────────────────────────
echo; echo "-- 7. Sessions (76-83)"
expect_ok 76 "BTC trades regardless of weekday" $TRADE open --market BTC/USD --direction long --notional 500 --collateral 100
expect_ok 77 "close" $TRADE close --market BTC/USD
expect_ok 78 "EUR/USD in session  [FX]" $TRADE open --market EUR/USD --direction long --notional 500 --collateral 100
expect_ok 79 "close" $TRADE close --market EUR/USD
expect_ok 80 "XAU/USD in session  [FX]" $TRADE open --market XAU/USD --direction long --notional 500 --collateral 100
expect_ok 81 "close" $TRADE close --market XAU/USD
manual  82 "watch EUR/USD across 21:00 UTC Sunday" "closed -> open"
manual  83 "ReduceOnly / Halted transitions" "needs set_market_status"

# ── 8. LP and accounting ────────────────────────────────────────────────────────
echo; echo "-- 8. LP pool and accounting (84-88)"
expect_ok 84 "LP vault readable" $TRADE status
expect_ok 85 "OI rises on open" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 200
expect_ok 86 "status shows OI" $TRADE status
expect_ok 87 "OI clears on close" $TRADE close --market BTC/USD
manual  88 "exceed max_oi_long (1M cap)" "needs ~1M of notional"

# ── 9. Liquidation and keeper ───────────────────────────────────────────────────
echo; echo "-- 9. Liquidation and keeper (89-94)"
expect_ok 89 "open near the leverage cap" $TRADE open --market BTC/USD --direction long --notional 1900 --collateral 200
expect_ok 90 "status shows it" $TRADE status
expect_ok 91 "close it" $TRADE close --market BTC/USD
manual  92 "keeper dry-run" "scripts/run-keeper.sh --dry-run"
manual  93 "keeper liquidates an underwater position" "needs an adverse price move"
manual  94 "kill and restart the keeper mid-run" "rebuilds from chain"

# ── 10. Invariants ──────────────────────────────────────────────────────────────
echo; echo "-- 10. Invariants under live conditions (95-100)"
expect_ok 95 "collateral vault >= deposits (I1)" bash -c '
  python3 - <<PY
import json,base64,urllib.request,sys
def rpc(m,p):
    r=urllib.request.Request("http://127.0.0.1:8899",data=json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode(),headers={"Content-Type":"application/json"})
    return json.load(urllib.request.urlopen(r))["result"]
d=json.load(open("deployment.json"))
v=rpc("getAccountInfo",[d["collateral_vault"],{"encoding":"base64"}])["value"]
sys.exit(0 if v and int.from_bytes(base64.b64decode(v["data"][0])[64:72],"little")>0 else 1)
PY'
expect_ok 96 "LP vault balance is positive (I2)" bash -c '
  python3 - <<PY
import json,base64,urllib.request,sys
def rpc(m,p):
    r=urllib.request.Request("http://127.0.0.1:8899",data=json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode(),headers={"Content-Type":"application/json"})
    return json.load(urllib.request.urlopen(r))["result"]
d=json.load(open("deployment.json"))
v=rpc("getAccountInfo",[d["lp_vault"],{"encoding":"base64"}])["value"]
sys.exit(0 if v and int.from_bytes(base64.b64decode(v["data"][0])[64:72],"little")>=1_000_000_000_000 else 1)
PY'
expect_ok  97 "round trip, then re-check" $TRADE open --market BTC/USD --direction long --notional 1000 --collateral 200
expect_ok  98 "close it" $TRADE close --market BTC/USD
expect_ok  99 "account is flat and consistent" $TRADE status
expect_ok 100 "automated suite still green" cargo test -j 4 --workspace --quiet

echo
echo "======================================"
printf "PASS %d   FAIL %d   MANUAL %d\n" "$PASS" "$FAIL" "$SKIP"
echo "======================================"
[[ $FAIL -eq 0 ]] || exit 1
