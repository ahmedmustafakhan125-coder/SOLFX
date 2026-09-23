#!/usr/bin/env bash
# What to check when nobody is watching.
#
# The failure this exists for is the quiet one: the poster stops, nothing on the box looks
# wrong, and every trade starts failing OracleStale sixty seconds later. `systemctl is-active`
# does not catch it either — a poster that is running but making no progress is still
# "active". So this checks for evidence of a completed pass, not just a live process.
#
# Alerts go to the journal always, and to $SOLFX_ALERT_WEBHOOK as JSON if that is set (an
# n8n webhook is the obvious target on this box). No webhook configured is not an error.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[[ -f "$ROOT/.env" ]] && { set -a; . "$ROOT/.env"; set +a; }

# The operator's *public key*, not its keypair file: a balance needs only the address, and this
# script has no business opening key material. Taken from the poster's own startup banner
# ("payer    <pubkey>") unless set explicitly, so it follows whichever wallet the poster uses.
OPERATOR_PUBKEY="${SOLFX_OPERATOR_PUBKEY:-$(journalctl -u solfx-price-poster.service --no-pager -o cat 2>/dev/null \
  | grep -oE 'payer +[1-9A-HJ-NP-Za-km-z]{32,44}' | tail -1 | awk '{print $2}')}"
RPC="${SOLFX_RPC_URL:-https://api.devnet.solana.com}"
SOL_FLOOR="${SOLFX_MIN_SOL:-0.5}"
API="${SOLFX_HEALTH_URL:-http://127.0.0.1:8787/healthz}"
PUBLIC="${SOLFX_PUBLIC_URL:-https://solfx.cloud/healthz}"
SOLANA_BIN="${SOLANA_BIN:-$(command -v solana 2>/dev/null || true)}"

problems=()
note() { problems+=("$1"); }

for unit in solfx-price-poster solfx-keeper; do
  state="$(systemctl is-active "$unit.service" 2>/dev/null)"
  [[ "$state" == "active" ]] || note "$unit is $state"
done

# The web tier is a container, not a unit — Traefik discovers it over the docker socket and
# that only works for containers. Its compose project is /docker/solfx, deliberately separate
# from the n8n stack it shares a network with.
if ! docker ps --filter name=^/solfx-web$ --filter status=running --format '{{.Names}}' 2>/dev/null | grep -q solfx-web; then
  note "solfx-web container is not running"
fi

# A pass takes ~25s and the interval is 2s, so five minutes without a pass that *posted
# something* means the poster has stopped making progress even if the process is still up.
#
# It used to look for any "pass complete" line, and "pass complete: 0 posted, 6 failed" is one.
# From 2026-09-21 to 09-23 the poster completed every pass and posted nothing — its fee payer was
# empty — and this reported "all healthy" 544 times while every price sat 38 hours stale. At the
# weekend a closed feed still posts (Pyth carries the last price forward), so "posted" is the
# right test on every day of the week.
if systemctl is-active --quiet solfx-price-poster.service; then
  # Captured first, then searched. Piping journalctl straight into `grep -q` under `pipefail` is
  # a race: grep exits on its first match, journalctl can take SIGPIPE, and pipefail reports the
  # pipeline as failed — a healthy poster reported as dead, intermittently. Seen 2026-09-23.
  recent="$(journalctl -u solfx-price-poster.service --since "-5 min" --no-pager 2>/dev/null || true)"
  if ! grep -qE "pass complete: [1-9][0-9]* posted" <<<"$recent"; then
    note "price-poster has not posted a price in 5 minutes — prices are going stale (check the operator's SOL and the Pyth key)"
  fi
  if grep -q "429" <<<"$recent"; then
    note "price-poster is being rate-limited (429) — lower --max-rps or move to a paid RPC"
  fi
fi

code="$(curl -s -o /dev/null -m 5 -w '%{http_code}' "$API" 2>/dev/null)"
[[ "$code" == "200" ]] || note "api health check returned ${code:-no response}"

# Checked separately from the loopback probe because it exercises what the loopback one
# cannot: the Traefik router, the DNS record and an unexpired certificate. A cert that fails
# to renew looks exactly like a healthy box from inside.
pub="$(curl -s -o /dev/null -m 10 -w '%{http_code}' "$PUBLIC" 2>/dev/null)"
[[ "$pub" == "200" ]] || note "$PUBLIC returned ${pub:-no response} — check the Traefik router or the certificate"

# The poster pays rent and an update fee per feed per pass. An empty operator wallet stops
# everything, and it stops it gradually, which is the worst way for it to stop. This check used
# to need SOLFX_OPERATOR_KEYPAIR, which was never set, so it silently never ran.
if [[ -x "$SOLANA_BIN" && -n "$OPERATOR_PUBKEY" ]]; then
  bal="$("$SOLANA_BIN" balance "$OPERATOR_PUBKEY" --url "$RPC" 2>/dev/null | awk '{print $1}')"
  if [[ -n "$bal" ]] && awk -v b="$bal" -v f="$SOL_FLOOR" 'BEGIN{exit !(b<f)}'; then
    note "operator $OPERATOR_PUBKEY is down to $bal SOL (floor $SOL_FLOOR) — top it up"
  fi
else
  note "cannot check the operator's SOL balance (no solana CLI, or no operator pubkey found)"
fi

if ((${#problems[@]} == 0)); then
  if [[ -z "${SOLFX_ALERT_WEBHOOK:-}" ]]; then
    echo "ok: api, poster and keeper all healthy (no SOLFX_ALERT_WEBHOOK set: a failure would reach only this journal)"
  else
    echo "ok: api, poster and keeper all healthy"
  fi
  exit 0
fi

printf 'UNHEALTHY: %s\n' "${problems[@]}" >&2

if [[ -n "${SOLFX_ALERT_WEBHOOK:-}" ]]; then
  payload="$(printf '%s\n' "${problems[@]}" | python3 -c '
import json,sys
print(json.dumps({"service":"solfx","host":__import__("socket").gethostname(),
                  "problems":[l.rstrip("\n") for l in sys.stdin if l.strip()]}))')"
  curl -s -m 10 -X POST -H 'content-type: application/json' \
       -d "$payload" "$SOLFX_ALERT_WEBHOOK" >/dev/null 2>&1 \
    || echo "warning: alert webhook POST failed" >&2
fi
exit 1
