#!/usr/bin/env bash
# Deploy both SolFX programs to a local validator.
#
# Localhost first, devnet second (docs/DEPLOY.md). A local validator gives instant
# confirmations, free SOL, and a ledger you can throw away — so every mistake costs seconds
# instead of a devnet airdrop and a redeploy.
#
# ## Why this clones accounts from devnet
#
# A bare local validator has no Pyth on it: no receiver program, and no price feed accounts.
# Every SolFX instruction that touches a market reads one, so on a bare validator the whole
# protocol is unreachable — you can deploy it and then do nothing with it, which is a
# confusing way to discover the problem.
#
# `--clone` copies the live accounts in at genesis.
#
# ## Frozen prices, and how to unfreeze them
#
# Cloned price accounts are frozen at whatever they were when the ledger was created, so they
# go stale within `max_staleness_seconds` and the protocol correctly stops trading. That was
# once a hard limit on what localnet could test.
#
# It no longer is: `price-poster` posts live Pyth updates to any cluster, localnet included,
# so funding, sessions and liquidation under real price movement can all be exercised here.
#
#     cargo run -p solfx-keeper --bin price-poster -- --rpc-url http://127.0.0.1:8899
#
# That needs more than the Pyth *programs*, which is what this script used to clone. Posting
# an update reads the receiver's `config` and `treasury` PDAs and the Wormhole guardian set,
# and a bare validator has none of them — the failure is an opaque `AccountNotFound` on a PDA
# whose name appears nowhere. Those three addresses are derived below rather than pasted, so
# a guardian-set rotation needs no edit here.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LEDGER="${LEDGER:-/tmp/solfx-localnet}"
RPC="http://127.0.0.1:8899"

say() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }

say "Building both programs"
anchor build

# Pyth's receiver program (which owns PriceUpdateV2 accounts) and its push-oracle program
# (which owns the sponsored price-feed account PDAs the keeper reads).
PYTH_RECEIVER="rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ"
PYTH_PUSH_ORACLE="pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT"
CLONE_FROM="${CLONE_FROM:-https://api.devnet.solana.com}"

# Feed accounts to clone, derived from the feed ids the markets will be listed with. Set
# SOLFX_CLONE_FEEDS to a space-separated list of addresses to add more.
CLONE_ARGS=(--clone-upgradeable-program "$PYTH_RECEIVER" --clone-upgradeable-program "$PYTH_PUSH_ORACLE")

# The receiver's config and treasury PDAs, and the Wormhole guardian set the VAAs are
# currently signed against. Derived by the poster, which owns that logic already.
say "Deriving the Pyth accounts the poster needs"
# This FAILS the script rather than warning. It used to warn and carry on, which produced a
# validator missing the Pyth receiver config — and the failure then surfaced two commands
# later as `AccountNotFound: DaWUKXCy...` from the poster, which points nowhere near the
# cause. A half-built validator is worse than no validator: it looks like it worked.
if ! PYTH_ACCOUNTS=$(cargo run -q -p solfx-keeper --bin price-poster -- --print-clone-args 2>&1); then
  echo
  echo "  ERROR: could not derive the Pyth accounts to clone."
  echo
  echo "  $PYTH_ACCOUNTS"
  echo
  echo "  Most likely cause: Hermes needs an API key. Pyth put the public endpoint behind"
  echo "  authentication on 26 Aug 2026 at 16:00 UTC, and every request without one now"
  echo "  returns 401. Get a free key at https://pythdata.app, then:"
  echo
  echo "      echo 'PYTH_API_KEY=your_key' >> .env && set -a && . ./.env && set +a"
  echo
  echo "  Verify it before re-running this script:"
  echo
  echo "      curl -s -o /dev/null -w '%{http_code}\\n' -H \"Authorization: Bearer \$PYTH_API_KEY\" \\"
  echo "        'https://pyth.dourolabs.app/hermes/v2/price_feeds'"
  echo
  echo "  Stopping here rather than starting a validator the poster cannot use."
  exit 1
fi
# shellcheck disable=SC2206
CLONE_ARGS+=($PYTH_ACCOUNTS)
echo "  $PYTH_ACCOUNTS"

for acct in ${SOLFX_CLONE_FEEDS:-}; do
  CLONE_ARGS+=(--clone "$acct")
done

say "Starting a fresh local validator (ledger: $LEDGER)"
pkill -f solana-test-validator 2>/dev/null || true
sleep 1
rm -rf "$LEDGER"
solana-test-validator --ledger "$LEDGER" --reset --quiet \
  --url "$CLONE_FROM" "${CLONE_ARGS[@]}" &
VALIDATOR_PID=$!
trap 'kill $VALIDATOR_PID 2>/dev/null || true' EXIT

# Wait for it rather than sleeping a guessed interval.
for _ in $(seq 1 60); do
  if solana --url "$RPC" cluster-version >/dev/null 2>&1; then break; fi
  sleep 1
done
solana --url "$RPC" cluster-version

say "Funding the deployer"
solana --url "$RPC" airdrop 100 >/dev/null
solana --url "$RPC" balance

say "Deploying solfx_core"
solana --url "$RPC" program deploy \
  --program-id target/deploy/solfx_core-keypair.json \
  target/deploy/solfx_core.so

say "Deploying solfx_referral"
solana --url "$RPC" program deploy \
  --program-id target/deploy/solfx_referral-keypair.json \
  target/deploy/solfx_referral.so

say "Deployed"
CORE=$(solana address -k target/deploy/solfx_core-keypair.json)
REF=$(solana address -k target/deploy/solfx_referral-keypair.json)
echo "  solfx_core     $CORE"
echo "  solfx_referral $REF"
solana --url "$RPC" program show "$CORE" | sed 's/^/    /'

say "Validator is running on $RPC (pid $VALIDATOR_PID). Ctrl-C to stop."
cat <<'NEXT'

The programs are deployed but the protocol is NOT initialised: `initialize_protocol` needs a
USDC mint and an admin, and `initialize_market` needs feed ids and a full risk envelope.
Those are deployment decisions, not something a script should invent — see docs/DEPLOY.md.

Initialise it, then start the price poster, then the keepers:

    cargo run -p solfx-keeper --bin init-protocol
    cargo run -p solfx-keeper --bin price-poster -- --rpc-url http://127.0.0.1:8899

Without the poster every trade fails on oracle staleness: cloned feeds are frozen at genesis.

Then:

    scripts/run-keeper.sh --reward-token-account <the keeper's USDC account>

or, to watch what it would do without sending anything:

    scripts/run-keeper.sh --dry-run
NEXT
wait $VALIDATOR_PID
