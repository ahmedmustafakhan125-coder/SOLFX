#!/usr/bin/env bash
# Fetch a program's IDL *from the cluster* into clients/js/idl/<program>.json.
#
# The client is generated from what is deployed, not from a local build. A local IDL describes
# the code on this machine; the published one describes the program users actually call, and a
# client generated from the wrong one fails on chain rather than in CI.
#
# Read-only. Takes no keypair and sends no transaction.
#
#   set -a && . ./.env && set +a
#   ./scripts/fetch-idl.sh noxfunds
set -euo pipefail

PROGRAM="${1:?usage: fetch-idl.sh <solfx_core|noxfunds|solfx_referral>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Same pinned version publish-idl.sh writes with, so the read and the write agree on format.
PMP="@solana-program/program-metadata@0.9.1"

: "${SOLFX_RPC_URL:?set SOLFX_RPC_URL first: set -a && . ./.env && set +a}"
redact() { printf '%s' "$1" | sed -E 's#(\?|&)(api-?key|api_key|token)=[^&]*#\1\2=<redacted>#Ig'; }

# The program id comes from Anchor.toml rather than being retyped here: one source of truth, so
# a changed deployment cannot leave this script fetching the old program's interface.
ID="$(awk -v p="$PROGRAM" '
  /^\[programs\.devnet\]/ { on = 1; next }
  /^\[/                   { on = 0 }
  on && $1 == p           { gsub(/"/, "", $3); print $3 }
' "$ROOT/Anchor.toml")"
[[ -n "$ID" ]] || { echo "error: no [programs.devnet] entry for '$PROGRAM' in Anchor.toml" >&2; exit 1; }

OUT="$ROOT/clients/js/idl/$PROGRAM.json"
echo "==> $PROGRAM $ID on $(redact "$SOLFX_RPC_URL")"
npx --yes "$PMP" fetch idl "$ID" --rpc "$SOLFX_RPC_URL" --output "$OUT" >/dev/null

python3 - "$OUT" "$ID" <<'PY'
import json, sys
path, want = sys.argv[1], sys.argv[2]
d = json.load(open(path))
if d.get("address") != want:
    raise SystemExit(f"error: fetched IDL is for {d.get('address')}, expected {want}")
# Re-serialise stably, so a refresh that changes nothing produces no diff.
json.dump(d, open(path, "w"), indent=2, sort_keys=True)
open(path, "a").write("\n")
print(f"    {len(d['instructions'])} instructions, {len(d.get('accounts', []))} accounts, "
      f"{len(d.get('events', []))} events, {len(d.get('errors', []))} errors")
PY
echo "==> wrote clients/js/idl/$PROGRAM.json"
