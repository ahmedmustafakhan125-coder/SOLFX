#!/usr/bin/env bash
# Bring the whole devnet stack up, or take it down.
#
#   ./scripts/run-devnet-stack.sh          start everything and health-check it
#   ./scripts/run-devnet-stack.sh --stop   stop everything
#   ./scripts/run-devnet-stack.sh --status what is running right now
#
# Three processes have to be up for the terminal to work, and each fails in its own way when
# it is not:
#
#   price-poster   publishes Pyth prices. Without it every trade fails on `OracleStale` after
#                  60 seconds — the single most common cause of "it stopped working".
#   solfx-keeper   liquidates and fires stop-loss / take-profit orders. Without it those
#                  orders rest on chain and nothing ever executes them.
#   vite           the terminal itself.
#
# ---------------------------------------------------------------------------------------
# The RPC budget, which is why the --max-rps numbers are not arbitrary
#
# All three share one endpoint, and a free tier allows roughly ten calls a second between
# them. Measured on this cluster: the poster alone ran 11 consecutive clean passes, and lost
# 491 feeds across 371 passes with an unthrottled keeper beside it. The poster is the one that
# suffers, because its calls are the only ones carrying a 60-second deadline.
#
# So the budget is split deliberately — poster 5, keeper 4, and the browser polls every 8s
# with what is left. Raise all three together on a paid endpoint; do not raise one alone.
# ---------------------------------------------------------------------------------------
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LOGS="$ROOT/.stack-logs"
POSTER="$ROOT/target/debug/price-poster"
KEEPER="$ROOT/target/debug/solfx-keeper"
APP_PORT=5173

# Matching on the absolute binary path is what makes the stop path safe. `pkill -f price-poster`
# also matches the shell that is running this script when the string appears in its own command
# line, and kills it mid-run — a trap this repo has fallen into more than once.
stop_all() {
  pkill -f "$POSTER" 2>/dev/null || true
  pkill -f "$KEEPER" 2>/dev/null || true
  fuser -k "${APP_PORT}/tcp" 2>/dev/null || true
  sleep 1
}

running() { pgrep -f "$1" >/dev/null 2>&1 && echo yes || echo no; }

status() {
  printf "  %-14s %s\n" "price-poster" "$(running "$POSTER")"
  printf "  %-14s %s\n" "solfx-keeper" "$(running "$KEEPER")"
  local code
  code=$(curl -s -o /dev/null -m 4 -w '%{http_code}' "http://localhost:${APP_PORT}/" 2>/dev/null || echo 000)
  printf "  %-14s %s\n" "terminal" "$([ "$code" = 200 ] && echo "yes  http://localhost:${APP_PORT}" || echo "no")"
}

case "${1:-start}" in
  --stop)   echo "==> stopping"; stop_all; status; exit 0 ;;
  --status) status; exit 0 ;;
  start|"") ;;
  *) echo "usage: $0 [--stop|--status]" >&2; exit 1 ;;
esac

# --- preconditions -----------------------------------------------------------------------
if [[ -z "${SOLFX_RPC_URL:-}" ]]; then
  echo "error: SOLFX_RPC_URL is not set. Run:" >&2
  echo "         set -a && . ./.env && set +a" >&2
  exit 1
fi

for bin in "$POSTER" "$KEEPER"; do
  if [[ ! -x "$bin" ]]; then
    echo "error: $(basename "$bin") is not built." >&2
    # Never build with a validator up: 12 rustc jobs beside one OOM-killed WSL2 on this machine.
    if pgrep -f solana-test-validator >/dev/null 2>&1; then
      echo "       a validator is running — stop it before building." >&2
    else
      echo "       cargo build -p solfx-keeper" >&2
    fi
    exit 1
  fi
done

REWARD_ATA="${SOLFX_REWARD_TOKEN_ACCOUNT:-}"
if [[ -z "$REWARD_ATA" ]]; then
  # The liquidator refuses to start without somewhere to receive its fee, and says so rather
  # than running as a keeper that can never actually liquidate.
  echo "note: SOLFX_REWARD_TOKEN_ACCOUNT is not set — the liquidator will not start." >&2
  echo "      export it, or add it to .env, to enable liquidations." >&2
fi

mkdir -p "$LOGS"
echo "==> stopping anything already running"
stop_all

# --- start -------------------------------------------------------------------------------
echo "==> price-poster   (--max-rps 5)"
nohup "$POSTER" --rpc-url "$SOLFX_RPC_URL" \
  --interval-secs 2 --concurrency 8 --max-rps 5 \
  > "$LOGS/poster.log" 2>&1 &

echo "==> solfx-keeper   (--max-rps 4)"
nohup "$KEEPER" --rpc-url "$SOLFX_RPC_URL" \
  --keypair "${SOLFX_KEYPAIR:-$HOME/.config/solana/id.json}" \
  --price-accounts "$ROOT/price-accounts.json" \
  ${REWARD_ATA:+--reward-token-account "$REWARD_ATA"} \
  --max-rps 4 \
  > "$LOGS/keeper.log" 2>&1 &

echo "==> terminal       (vite on :$APP_PORT)"
( cd "$ROOT/app" && nohup npx vite --port "$APP_PORT" > "$LOGS/vite.log" 2>&1 & )

# --- wait for the first poster pass -------------------------------------------------------
# The stack is not usable until prices are on chain, so wait for evidence rather than a fixed
# sleep. A pass is roughly 25 s; 90 s is generous without being an unbounded hang.
echo "==> waiting for the first poster pass (up to 90s)"
for _ in $(seq 1 90); do
  grep -q "pass complete" "$LOGS/poster.log" 2>/dev/null && break
  sleep 1
done

echo
echo "==> status"
status
echo
if grep -q "pass complete" "$LOGS/poster.log" 2>/dev/null; then
  echo "  poster: $(grep 'pass complete' "$LOGS/poster.log" | tail -1)"
else
  echo "  poster: no pass yet — check $LOGS/poster.log"
fi
if grep -q "429" "$LOGS/poster.log" 2>/dev/null; then
  echo "  WARNING: 429s in the poster log — the RPC budget is over. Lower --max-rps."
fi
echo
echo "  logs   $LOGS/{poster,keeper,vite}.log"
echo "  stop   ./scripts/run-devnet-stack.sh --stop"
