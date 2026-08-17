#!/usr/bin/env bash
# Deploy both SolFX programs to a local validator, then initialise the protocol.
#
# Localhost first, devnet second (docs/DEPLOY.md). A local validator gives instant
# confirmations, free SOL, and a ledger you can throw away — so every mistake costs seconds
# instead of a devnet airdrop and a redeploy.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LEDGER="${LEDGER:-/tmp/solfx-localnet}"
RPC="http://127.0.0.1:8899"

say() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }

say "Building both programs"
anchor build

say "Starting a fresh local validator (ledger: $LEDGER)"
pkill -f solana-test-validator 2>/dev/null || true
sleep 1
rm -rf "$LEDGER"
solana-test-validator --ledger "$LEDGER" --reset --quiet &
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
echo "Next: cargo run -p solfx-cli -- init   (Phase 7/8 tooling), or drive it from the SDK."
wait $VALIDATOR_PID
