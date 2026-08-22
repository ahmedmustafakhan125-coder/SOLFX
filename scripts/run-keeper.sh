#!/usr/bin/env bash
# Start the SolFX keepers against a running cluster.
#
# Assumes `scripts/deploy-localnet.sh` has already deployed and initialised the protocol.
# For devnet, pass --rpc-url https://api.devnet.solana.com.
#
# The keeper needs:
#   * SOL for transaction fees (and it earns lamports back from trigger-order rent)
#   * a USDC token account, for liquidation penalties
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RPC_URL="${SOLFX_RPC_URL:-http://127.0.0.1:8899}"
KEYPAIR="${SOLFX_KEYPAIR:-$ROOT/.localnet/keeper.json}"

if [[ ! -f "$KEYPAIR" ]]; then
  echo "==> creating a keeper keypair at $KEYPAIR"
  mkdir -p "$(dirname "$KEYPAIR")"
  solana-keygen new --no-bip39-passphrase --silent --outfile "$KEYPAIR"
fi

KEEPER_PUBKEY="$(solana-keygen pubkey "$KEYPAIR")"
echo "==> keeper $KEEPER_PUBKEY"

BALANCE="$(solana balance "$KEEPER_PUBKEY" --url "$RPC_URL" 2>/dev/null | awk '{print $1}' || echo 0)"
if [[ "${BALANCE%%.*}" -lt 1 ]]; then
  echo "==> funding the keeper"
  solana airdrop 10 "$KEEPER_PUBKEY" --url "$RPC_URL" >/dev/null
fi

# A liquidator with no USDC account cannot be paid its penalty, so the keeper refuses to
# start that service without one rather than looping on a failure it cannot fix. Pass
# --reward-token-account, or set SOLFX_USDC_MINT and let this create the ATA.
if [[ -z "${SOLFX_REWARD_TOKEN_ACCOUNT:-}" && -n "${SOLFX_USDC_MINT:-}" ]]; then
  echo "==> ensuring a USDC account for the keeper (mint $SOLFX_USDC_MINT)"
  spl-token create-account "$SOLFX_USDC_MINT" \
    --owner "$KEYPAIR" --fee-payer "$KEYPAIR" --url "$RPC_URL" >/dev/null 2>&1 || true
  SOLFX_REWARD_TOKEN_ACCOUNT="$(
    spl-token address --token "$SOLFX_USDC_MINT" --owner "$KEEPER_PUBKEY" --verbose \
      --url "$RPC_URL" | awk '/Associated Token Address/ {print $4}'
  )"
  export SOLFX_REWARD_TOKEN_ACCOUNT
  echo "    $SOLFX_REWARD_TOKEN_ACCOUNT"
fi

echo "==> building"
cargo build --release -p solfx-keeper

echo "==> starting keepers against $RPC_URL"
exec ./target/release/solfx-keeper \
  --rpc-url "$RPC_URL" \
  --keypair "$KEYPAIR" \
  "$@"
