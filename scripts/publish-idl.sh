#!/usr/bin/env bash
# Publish the on-chain IDL for a program.
#
#   ./scripts/publish-idl.sh              # solfx_core, the default
#   ./scripts/publish-idl.sh noxfunds
#
# The program is an argument rather than a constant because a hardcoded name here once meant
# `deploy-devnet.sh noxfunds` would have closed and rewritten the *live solfx-core* IDL — a
# destructive, flaky operation — while never publishing the program it was asked about.
#
# `anchor program deploy` tries to do this itself and fails on this program. The failure is
# silent about its cause — every path (anchor idl init/upgrade/create-buffer, and the
# Program Metadata CLI directly) reports only:
#
#     [Error] The provided transaction plan failed to execute.
#
# Two things cause it, and both are handled here.
#
# 1. SIZE. Anchor 1.x stores the IDL in the Program Metadata Program, zlib-compressed. The
#    full solfx-core IDL is 216 KB on disk and 22 KB compressed, because Anchor embeds every
#    `///` doc comment and this codebase documents heavily. A 22 KB write fails; an 8 KB
#    write succeeds; a 43-byte write succeeds. Stripping `docs` is what brings it under the
#    line, and it costs nothing that matters: the on-chain IDL exists so explorers and
#    runtime clients can *decode* transactions, and the full documented IDL stays in
#    target/idl/ for anything that wants it.
#
# 2. A HALF-WRITTEN ACCOUNT CANNOT BE OVERWRITTEN. Once a write fails partway, the metadata
#    account is left allocated and corrupt — zlib refuses it outright, not merely truncated —
#    and every subsequent write to that account fails too. `anchor idl init` additionally
#    "can only be run once", so retrying the deploy can never recover. The account has to be
#    closed first, which is why this script always closes before writing.
#
# Neither is a program bug, and none of it affects whether the program runs. A client that
# ships target/idl/solfx_core.json needs nothing here; this is for explorers and for clients
# that introspect at runtime.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RPC_URL="${SOLFX_RPC_URL:-http://127.0.0.1:8899}"
KEYPAIR="${SOLFX_KEYPAIR:-$HOME/.config/solana/id.json}"
PROGRAM="${1:-solfx_core}"
case "$PROGRAM" in
  solfx_core|noxfunds) ;;
  *) echo "error: unknown program '$PROGRAM' (expected solfx_core or noxfunds)" >&2; exit 1 ;;
esac
IDL="$ROOT/target/idl/$PROGRAM.json"
# Pinned rather than `latest`: this writes the program's canonical public interface.
PMP="@solana-program/program-metadata@0.9.1"

# Never print the RPC URL: on a paid endpoint the API key is a query parameter, so a script
# that echoes it leaks the key into every screenshot, paste and CI log. `redact` shows the host
# and hides the rest; the commands printed for you to run keep the *literal* `"$SOLFX_RPC_URL"`,
# which your shell expands when you run them and which reveals nothing on screen.
redact() { printf '%s' "$1" | sed -E 's#(\?|&)(api-?key|api_key|token)=[^&]*#\1\2=<redacted>#Ig'; }
RPC_LITERAL='"$SOLFX_RPC_URL"'

if [[ ! -f "$IDL" ]]; then
  echo "error: $IDL not found — run \`anchor build\` first" >&2
  exit 1
fi

PROGRAM_ID="$(python3 -c "import json;print(json.load(open('$IDL'))['address'])")"

# Say plainly where this is going. The failure that motivated this: SOLFX_RPC_URL unset in a
# fresh shell, the default silently selected localnet, and five close-then-write cycles ran
# against a validator that was not there — which reads like a broken script rather than a
# missing variable.
if [[ -z "${SOLFX_RPC_URL:-}" ]]; then
  echo "note: SOLFX_RPC_URL is not set — defaulting to $(redact "$RPC_URL")" >&2
  echo "      for devnet:  set -a && . ./.env && set +a" >&2
fi

# Check the cluster is reachable and actually has the program *before* closing anything.
# The close is destructive and only safe when the write that follows can succeed.
if ! solana program show "$PROGRAM_ID" -u "$RPC_URL" >/dev/null 2>&1; then
  echo "error: $PROGRAM_ID is not deployed on $(redact "$RPC_URL") (or the RPC is unreachable)" >&2
  echo "       nothing was changed" >&2
  exit 1
fi
SLIM="$(mktemp -t "$PROGRAM"-idl-slim-XXXXXX.json)"
trap 'rm -f "$SLIM"' EXIT

python3 - "$IDL" "$SLIM" <<'PY'
import json, sys, zlib

src, dst = sys.argv[1], sys.argv[2]
idl = json.load(open(src))


def strip(node):
    """Drop `docs` everywhere. Nothing else is removed, so the decoding surface is intact."""
    if isinstance(node, dict):
        return {k: strip(v) for k, v in node.items() if k != "docs"}
    if isinstance(node, list):
        return [strip(v) for v in node]
    return node


lean = strip(idl)
blob = json.dumps(lean, separators=(",", ":")).encode()
open(dst, "wb").write(blob)

full = json.dumps(idl, separators=(",", ":")).encode()
print(f"  full  {len(full):>7} raw  {len(zlib.compress(full, 9)):>6} compressed")
print(f"  slim  {len(blob):>7} raw  {len(zlib.compress(blob, 9)):>6} compressed")
print(
    f"  kept  {len(lean['instructions'])} instructions, "
    f"{len(lean.get('accounts', []))} accounts, "
    f"{len(lean.get('types', []))} types, "
    f"{len(lean.get('events', []))} events"
)
PY

echo "==> $PROGRAM $PROGRAM_ID on $(redact "$RPC_URL")"

# The write is flaky — measured at roughly one failure in two against devnet, with the same
# payload that had just succeeded. The transaction plan reports no cause, so there is nothing
# to react to intelligently; retrying is the whole remedy.
#
# Each attempt closes first. That is not belt-and-braces: a failed write leaves the account
# allocated and corrupt, and every subsequent write to a corrupt account fails, so an attempt
# that did not close would poison every attempt after it.
ATTEMPTS="${SOLFX_IDL_ATTEMPTS:-5}"
for attempt in $(seq 1 "$ATTEMPTS"); do
  echo "==> attempt $attempt/$ATTEMPTS: closing any existing IDL metadata account"
  npx --yes "$PMP" close idl "$PROGRAM_ID" \
    --rpc "$RPC_URL" --keypair "$KEYPAIR" >/dev/null 2>&1 || true

  echo "==> attempt $attempt/$ATTEMPTS: writing IDL"
  if npx --yes "$PMP" write idl "$PROGRAM_ID" "$SLIM" \
      --rpc "$RPC_URL" --keypair "$KEYPAIR" 2>&1 | tail -3 | grep -q "Success"; then
    written=yes
    break
  fi
  echo "    failed; retrying"
done

if [[ "${written:-no}" != "yes" ]]; then
  echo "error: could not write the IDL in $ATTEMPTS attempts" >&2
  echo "       the metadata account is left closed, so a re-run starts clean" >&2
  exit 1
fi

echo "==> verifying it reads back"
FETCHED="$(mktemp -t "$PROGRAM"-idl-fetched-XXXXXX.json)"
trap 'rm -f "$SLIM" "$FETCHED"' EXIT
anchor idl fetch -o "$FETCHED" "$PROGRAM_ID" --provider.cluster "$RPC_URL" >/dev/null

# Verified by shape, not names: a write that landed an older interface must not pass.
python3 "$ROOT/scripts/idl-compare.py" "$FETCHED" "$IDL"

echo "==> done"
