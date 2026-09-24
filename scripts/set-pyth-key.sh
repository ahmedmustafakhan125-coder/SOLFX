#!/usr/bin/env bash
#
# Swap the Pyth API key and bring the price feed back up.
#
# Every Hermes endpoint requires a key — measured 2026-09-11 against
# hermes.pyth.network, hermes-beta.pyth.network and pyth.dourolabs.app/hermes, all 401
# without one. So when the key expires the poster stops, every feed goes stale, and the
# session crank correctly halts every market within a minute. Recovery is this script plus
# the six minutes the Halted -> GapWindow -> Active path takes.
#
# The order matters and is the reason this is a script rather than three commands:
#
#   1. Probe the new key BEFORE writing anything. A bad key that is written and rolled out
#      leaves the venue in exactly the state it was already in, having also destroyed the
#      evidence of what the old one was.
#   2. Back up .env, then write.
#   3. Restart all three consumers. The web container reads .env through `env_file`, so it
#      needs `docker compose up -d`, not a signal — an scp'd .env writes a new inode.
#   4. Verify a real pass, rather than reporting success because systemd says "active".
#      A poster that is up and posting nothing is still active; that is the failure mode
#      this whole project keeps rediscovering.
#
# Usage:  ./scripts/set-pyth-key.sh                 # prompts for the key, hidden
#         ./scripts/set-pyth-key.sh <new-api-key>   # works, but lands in shell history
#         ./scripts/set-pyth-key.sh --probe [key]   # check a key, change nothing
#
# A working key is necessary and not sufficient. On 2026-09-23 the key was replaced and prices
# stayed 38 hours stale, because the poster's fee payer held 0.00065 SOL and every transaction
# was silently dropped. So this also checks the payer, and says which of the two is missing.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$ROOT/.env"
COMPOSE_DIR="/docker/solfx"

# BTC/USD — the most basic feed there is. If this 401s, the key is not live.
PROBE_FEED="e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43"
HERMES="${SOLFX_HERMES_URL:-https://pyth.dourolabs.app/hermes}"

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  sed -n '3,29p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi

probe_only=false
if [[ "${1:-}" == "--probe" ]]; then
  probe_only=true
  shift
fi

KEY="${1:-}"
if [[ -z "$KEY" ]]; then
  # Read without echo, so the key never lands in the terminal or in ~/.bash_history.
  read -rsp "new Pyth API key (hidden): " KEY
  echo
fi
if [[ -z "$KEY" || "$KEY" == -* ]]; then
  # An option is never a key. `--help` used to be sent to Hermes as one (2026-09-24): refused,
  # harmlessly, but the script should have said what it takes instead.
  echo "usage: $0 [--probe] [new-pyth-api-key]   (run with no key to be prompted, hidden)" >&2
  exit 64
fi

# The poster's fee payer, by public key, from its own startup banner. Checked before anything
# is changed: a new key cannot bring prices back if the transactions carrying them cannot pay.
SOL_FLOOR="${SOLFX_MIN_SOL:-0.5}"
PAYER="${SOLFX_OPERATOR_PUBKEY:-$(journalctl -u solfx-price-poster.service --no-pager -o cat 2>/dev/null \
  | grep -oE 'payer +[1-9A-HJ-NP-Za-km-z]{32,44}' | tail -1 | awk '{print $2}')}"
check_payer() {
  [[ -n "$PAYER" ]] || { echo "  (could not find the poster's fee payer — skipping the balance check)"; return 0; }
  local rpc bal
  rpc="$(grep -E '^SOLFX_RPC_URL=' "$ENV_FILE" | cut -d= -f2- | tr -d '"' || true)"
  bal="$(solana balance "$PAYER" --url "${rpc:-https://api.devnet.solana.com}" 2>/dev/null | awk '{print $1}')"
  if [[ -z "$bal" ]]; then
    echo "  (could not read the fee payer's balance)"
  elif awk -v b="$bal" -v f="$SOL_FLOOR" 'BEGIN{exit !(b<f)}'; then
    echo "WARNING: the poster's fee payer $PAYER holds $bal SOL (floor $SOL_FLOOR)." >&2
    echo "         Its transactions will be dropped silently and prices will stay stale. Fund it:" >&2
    echo "           solana transfer $PAYER 5 --url devnet" >&2
    return 1
  else
    echo "  fee payer $PAYER holds $bal SOL"
  fi
}

# --- 1. probe -------------------------------------------------------------------------------
echo "probing $HERMES with the supplied key…"
code="$(curl -s -o /tmp/pyth-probe.out -w '%{http_code}' -m 30 \
  -H "Authorization: Bearer $KEY" \
  "$HERMES/v2/updates/price/latest?ids[]=$PROBE_FEED&encoding=hex" || echo 000)"

if [[ "$code" != "200" ]]; then
  echo "REFUSED: the key did not work (HTTP $code). Nothing was changed." >&2
  echo "  $(head -c 200 /tmp/pyth-probe.out)" >&2
  exit 1
fi
echo "  the key works (HTTP 200, $(wc -c </tmp/pyth-probe.out) bytes of update data)"

check_payer || true
if $probe_only; then
  echo "--probe given; .env untouched and nothing restarted."
  exit 0
fi

# --- 2. write -------------------------------------------------------------------------------
backup="$ENV_FILE.bak.$(date +%s)"
cp "$ENV_FILE" "$backup"
# Rewritten in place with the same permissions rather than recreated: the file is read by
# systemd units and bind-referenced by the compose project.
if grep -q '^PYTH_API_KEY=' "$ENV_FILE"; then
  # `|` as the delimiter — a key may contain a slash.
  sed -i "s|^PYTH_API_KEY=.*|PYTH_API_KEY=$KEY|" "$ENV_FILE"
else
  printf '\nPYTH_API_KEY=%s\n' "$KEY" >>"$ENV_FILE"
fi
echo "  .env updated (previous copy at $backup)"

# --- 3. restart the three consumers ----------------------------------------------------------
echo "restarting the poster and the keeper…"
systemctl restart solfx-price-poster solfx-keeper

if [[ -f "$COMPOSE_DIR/docker-compose.yml" ]]; then
  echo "restarting the web tier (env_file is re-read on up, not on a signal)…"
  ( cd "$COMPOSE_DIR" && docker compose up -d >/dev/null )
fi

# --- 4. verify a real pass --------------------------------------------------------------------
echo "waiting for a clean pass (a unit being active is not evidence)…"
deadline=$(( $(date +%s) + 180 ))
while (( $(date +%s) < deadline )); do
  if journalctl -u solfx-price-poster --since "60 seconds ago" --no-pager 2>/dev/null \
      | grep -q "posted, 0 failed"; then
    echo
    journalctl -u solfx-price-poster --since "60 seconds ago" --no-pager | grep "pass complete" | tail -2
    echo
    echo "The feed is back. The markets are still Halted and will stay that way until the"
    echo "keeper cranks them: crank_market_session moves Halted -> GapWindow, then GapWindow"
    echo "-> Active after GAP_WINDOW_SECONDS (5 min). Expect Active in roughly six minutes."
    echo "Watch it with:  ./scripts/vps-health.sh"
    exit 0
  fi
  sleep 5
done

echo "The key works but no clean pass landed within 180s." >&2
check_payer >&2 || echo "  ^ that is the likely cause." >&2
echo "The poster's own log:" >&2
journalctl -u solfx-price-poster --since "2 minutes ago" --no-pager | tail -8 >&2
exit 1
