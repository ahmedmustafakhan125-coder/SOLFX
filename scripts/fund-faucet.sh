#!/usr/bin/env bash
# Fill the devnet faucet with test USDC and SOL. Run on YOUR machine, never on the server.
#
#   set -a && . ./.env && set +a
#   ./scripts/fund-faucet.sh <FAUCET_PUBKEY>                      # 10,000,000 USDC + 10 SOL
#   ./scripts/fund-faucet.sh <FAUCET_PUBKEY> --usdc 500000 --sol 2
#   ./scripts/fund-faucet.sh <FAUCET_PUBKEY> --simulate-as <AUTHORITY_PUBKEY>   # needs no key
#
# Signs with the collateral mint's authority: --authority <path>, else $SOLFX_MINT_AUTHORITY,
# else ~/.config/solana/id.json. That key is why this runs here: the faucet server only ever
# *transfers* from the balance this puts in its account, so the one key that can create test
# USDC never leaves the operator's machine. The faucet's public key is printed in the service's log line
# on start, and by `curl -s 172.18.0.1:8789/faucet/health` on the server.
#
# Safe to re-run: the faucet's token account is created idempotently and each run adds more.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ $# -lt 1 || "$1" == -* ]]; then
  sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
fi
FAUCET="$1"
shift

if [[ -z "${SOLFX_RPC_URL:-}" ]]; then
  echo "error: SOLFX_RPC_URL is not set." >&2
  echo "       set -a && . ./.env && set +a" >&2
  exit 1
fi

# Its own small package, so the server's web tier stays dependency-free. `npm ci` installs
# exactly the locked versions the service was tested with.
if [[ ! -d services/faucet/node_modules ]]; then
  echo "==> installing the faucet's dependencies (once)"
  npm ci --prefix services/faucet --no-fund --no-audit >/dev/null
fi

# The mint authority's keypair is resolved here, on the operator's machine, so the service code
# never names a key path. An explicit --authority (or --simulate-as, which needs none) wins.
AUTHORITY="${SOLFX_MINT_AUTHORITY:-$HOME/.config/solana/id.json}"
for a in "$@"; do [[ "$a" == "--authority" || "$a" == "--simulate-as" ]] && AUTHORITY=""; done

# The URL goes in as an argument and is never echoed: on a keyed endpoint it carries the key.
exec node services/faucet/fund.mjs --faucet "$FAUCET" --rpc "$SOLFX_RPC_URL" \
  ${AUTHORITY:+--authority "$AUTHORITY"} "$@"
