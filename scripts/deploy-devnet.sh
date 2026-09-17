#!/usr/bin/env bash
# Deploy (or verify) a program on devnet.
#
#   ./scripts/deploy-devnet.sh              # solfx_core, the default
#   ./scripts/deploy-devnet.sh noxfunds
#
# Two programs share this because the verification below is the hard part, and a second copy
# of it is a second place for the bytecode comparison to be subtly wrong.
#
# Claude never runs `solana program deploy` — this script does not either. It works out what
# the cluster needs, prints the exact command for you to run, and then *verifies* the result.
# The verification is the point. The C-1 fix sat undeployed for a day because "deploy it" and
# "check it landed" were two manual steps and only the first got done.
#
# Run it as often as you like. It changes nothing unless the IDL is out of date:
#
#   ./scripts/deploy-devnet.sh          # status → either "current" or the command to run
#   <you run the printed deploy>
#   ./scripts/deploy-devnet.sh          # verifies the bytecode, then publishes the IDL
#
# ---------------------------------------------------------------------------------------
# How the bytecode comparison works, and why the obvious version is wrong
#
# `solana program show` reports "Data Length" as the **allocated** size, not the used size,
# and `solana program dump` returns exactly that many bytes — the program followed by zero
# padding. Measured here: a 1,009,680-byte .so in a 1,107,816-byte account, with all 98,136
# trailing bytes zero.
#
# The tempting comparison is to strip the dump's trailing zeros and compare lengths. That is
# wrong, and it produced a false "not deployed" verdict on a program that *was* deployed:
# solfx_core.so genuinely ends in 15 zero bytes, so stripping ate part of the program and the
# lengths disagreed by exactly 15.
#
# The correct comparison, used below: take the first `len(local)` bytes of the dump and require
# them to equal the local file, and separately require every byte past that point to be zero.
# ---------------------------------------------------------------------------------------
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROGRAM="${1:-solfx_core}"

# Which sources actually compile into each .so. The keeper is a separate binary and its churn
# must not look like a stale program build.
case "$PROGRAM" in
  solfx_core)
    SO_SOURCES=(programs/solfx-core/src crates/solfx-math/src)
    ;;
  noxfunds)
    # noxfunds links solfx-core as a library for its CPI bindings, so a change there makes
    # this build stale too.
    SO_SOURCES=(programs/noxfunds/src programs/solfx-core/src crates/solfx-math/src)
    ;;
  *)
    echo "error: unknown program '$PROGRAM' (expected solfx_core or noxfunds)" >&2
    exit 1
    ;;
esac

RPC_URL="${SOLFX_RPC_URL:-}"
KEYPAIR="${SOLFX_KEYPAIR:-$HOME/.config/solana/id.json}"
SO="$ROOT/target/deploy/$PROGRAM.so"
PROGRAM_KEYPAIR="$ROOT/target/deploy/$PROGRAM-keypair.json"
IDL="$ROOT/target/idl/$PROGRAM.json"

# SIMD-0431. An extend for fewer bytes than this is rejected outright, which is the
# `ExtendProgram requires a minimum of 10240 additional bytes` failure we hit by hand.
MINIMUM_EXTEND_PROGRAM_BYTES=10240

die() { echo "error: $*" >&2; exit 1; }

if [[ -z "$RPC_URL" ]]; then
  echo "error: SOLFX_RPC_URL is not set." >&2
  echo "       set -a && . ./.env && set +a" >&2
  exit 1
fi

[[ -f "$IDL" ]] || die "$IDL not found — run \`anchor build\` first"
[[ -f "$SO"  ]] || die "$SO not found — run \`anchor build\` first"

PROGRAM_ID="$(python3 -c "import json;print(json.load(open('$IDL'))['address'])")"

# A build older than the sources it came from is the failure this whole script exists to
# catch, one level earlier. Refuse rather than deploy something stale.
if [[ -n "$(find "${SO_SOURCES[@]}" -name '*.rs' -newer "$SO" -print -quit 2>/dev/null)" ]]; then
  echo "error: $SO is older than at least one source file it is built from." >&2
  echo "       run \`anchor build\` first (and not while a validator is running)." >&2
  find "${SO_SOURCES[@]}" -name '*.rs' -newer "$SO" -printf '         %p\n' 2>/dev/null | head -5 >&2
  exit 1
fi

echo "==> $PROGRAM $PROGRAM_ID"
echo "    cluster $RPC_URL"
echo "    local   $(wc -c < "$SO") bytes"

# --- is it deployed at all? -------------------------------------------------------------
if ! SHOW="$(solana program show "$PROGRAM_ID" -u "$RPC_URL" 2>/dev/null)"; then
  echo
  echo "not deployed on this cluster. Run:"
  echo
  echo "  solana program deploy \"$SO\" \\"
  echo "    --program-id \"$PROGRAM_KEYPAIR\" -u \"$RPC_URL\" -k \"$KEYPAIR\""
  echo
  echo "then re-run this script to verify and publish the IDL."
  exit 1
fi

ALLOCATED="$(awk '/^Data Length:/ {print $3}' <<<"$SHOW")"
AUTHORITY="$(awk '/^Authority:/ {print $2}' <<<"$SHOW")"
LOCAL_LEN="$(wc -c < "$SO")"
echo "    onchain $ALLOCATED bytes allocated, authority $AUTHORITY"

if [[ "$AUTHORITY" == "none" ]]; then
  die "this program is immutable — its upgrade authority has been revoked. It cannot be upgraded."
fi

# --- does the deployed bytecode already match the local build? --------------------------
DUMP="$(mktemp -t "$PROGRAM"-onchain-XXXXXX.so)"
trap 'rm -f "$DUMP"' EXIT
solana program dump "$PROGRAM_ID" "$DUMP" -u "$RPC_URL" >/dev/null

if python3 - "$SO" "$DUMP" <<'PY'
import hashlib, sys

local = open(sys.argv[1], "rb").read()
dump = open(sys.argv[2], "rb").read()
if len(dump) < len(local):
    print(f"  on-chain account holds {len(dump)} bytes, smaller than the {len(local)}-byte build")
    raise SystemExit(1)
head, pad = dump[: len(local)], dump[len(local) :]
if head != local:
    off = next(i for i, (a, b) in enumerate(zip(head, local)) if a != b)
    print(f"  bytecode differs, first at offset {off}")
    print(f"  local   sha256 {hashlib.sha256(local).hexdigest()}")
    print(f"  onchain sha256 {hashlib.sha256(head).hexdigest()}")
    raise SystemExit(1)
if set(pad) - {0}:
    print(f"  {len(pad)} bytes of padding are not all zero — the account holds something unexpected")
    raise SystemExit(1)
print(f"  bytecode matches: sha256 {hashlib.sha256(local).hexdigest()}")
print(f"  {len(pad)} bytes of zero padding")
PY
then
  echo "==> deployed code is the current build — no deploy needed"
else
  echo
  # An upgrade whose new code exceeds the allocated space needs an extend first. `solana
  # program deploy` will extend on its own, but it is worth printing the numbers so a failure
  # is legible rather than mysterious.
  if (( LOCAL_LEN > ALLOCATED )); then
    NEED=$(( LOCAL_LEN - ALLOCATED ))
    (( NEED < MINIMUM_EXTEND_PROGRAM_BYTES )) && NEED=$MINIMUM_EXTEND_PROGRAM_BYTES
    echo "the build is larger than the allocated account ($LOCAL_LEN > $ALLOCATED)."
    echo "extend first — SIMD-0431 rejects anything under $MINIMUM_EXTEND_PROGRAM_BYTES additional bytes:"
    echo
    echo "  solana program extend \"$PROGRAM_ID\" $NEED -u \"$RPC_URL\" -k \"$KEYPAIR\""
    echo
  fi
  echo "deploy the current build. Run:"
  echo
  echo "  solana program deploy \"$SO\" \\"
  echo "    --program-id \"$PROGRAM_KEYPAIR\" -u \"$RPC_URL\" -k \"$KEYPAIR\""
  echo
  echo "then re-run this script — it verifies the bytecode and publishes the IDL."
  exit 1
fi

# --- IDL ---------------------------------------------------------------------------------
# Only reached when the bytecode is confirmed current, so publishing here can never advertise
# an interface the deployed program does not have.
echo "==> checking the on-chain IDL"
FETCHED="$(mktemp -t "$PROGRAM"-idl-XXXXXX.json)"
trap 'rm -f "$DUMP" "$FETCHED"' EXIT

if anchor idl fetch -o "$FETCHED" "$PROGRAM_ID" --provider.cluster "$RPC_URL" >/dev/null 2>&1 \
   && python3 - "$FETCHED" "$IDL" <<'PY'
import json, sys

on_chain = {i["name"] for i in json.load(open(sys.argv[1]))["instructions"]}
local = {i["name"] for i in json.load(open(sys.argv[2]))["instructions"]}
if on_chain != local:
    print(f"  stale: missing={sorted(local - on_chain)} extra={sorted(on_chain - local)}")
    raise SystemExit(1)
print(f"  on-chain IDL matches the build: {len(local)} instructions")
PY
then
  echo "==> everything is current"
else
  echo "  publishing"
  "$ROOT/scripts/publish-idl.sh" "$PROGRAM"
fi
