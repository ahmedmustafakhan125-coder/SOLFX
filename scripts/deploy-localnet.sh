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
# `--clone` copies the live accounts in at genesis. The prices are frozen at whatever they
# were when the ledger was created, which is exactly right for testing the *plumbing* and
# exactly wrong for testing anything time-dependent: a frozen feed goes stale within
# `max_staleness_seconds` and the protocol correctly stops trading on it. That is the
# behaviour, not a bug — but it means session, funding and liquidation behaviour under real
# price movement belongs on devnet, not here.
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

Once the protocol is initialised, start the keepers with:

    scripts/run-keeper.sh --reward-token-account <the keeper's USDC account>

or, to watch what it would do without sending anything:

    scripts/run-keeper.sh --dry-run
NEXT
wait $VALIDATOR_PID
