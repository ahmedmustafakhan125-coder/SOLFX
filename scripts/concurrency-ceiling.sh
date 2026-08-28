#!/usr/bin/env bash
#
# How many concurrent transactions can SolFX actually sustain?
#
# This is NOT a load test, and that is deliberate. On Solana, concurrency is decided by
# write-lock contention, not by how many clients you point at an RPC node. Two transactions
# run in parallel only if their writable account sets do not overlap. Two transactions that
# both write the same account are serialised by the runtime no matter how many machines send
# them.
#
# So the ceiling is arithmetic, not an experiment:
#
#     max tx per block = MAX_WRITABLE_ACCOUNT_UNITS / (CU per tx)
#     max TPS          = max tx per block / slot_time
#
# A load generator against a single-node test validator cannot beat that number and cannot
# validate it either — a test validator has no leader schedule, no gossip, no fee market and
# no competing traffic. It is useful for finding failure modes, not for capacity.
#
# What this script does: run the existing compute-budget tests, which measure real CU through
# LiteSVM's `compute_units_consumed`, then turn each measurement into a concurrency ceiling.
# Every number it prints traces to a measurement; none is estimated.
#
# Usage:  ./scripts/concurrency-ceiling.sh
#
# Requires: no validator running (it builds). Takes ~30s on a warm target dir.

set -euo pipefail
cd "$(dirname "$0")/.."

# --- consensus-enforced limits ------------------------------------------------------------
#
# Verified against Solana documentation 2026-08-28:
#
#   MAX_BLOCK_UNITS            100,000,000  SIMD-0286, live on mainnet 29 Jul 2026 (epoch 1009)
#   MAX_WRITABLE_ACCOUNT_UNITS  12,000,000  UNCHANGED by SIMD-0286
#   slot time                        400 ms  SIMD-0525 (250/200ms) is still Draft
#
# The second one is the one that binds. From solana.com/upgrades/100m-cu-blocks:
#   "Max writable account units ... stays at 12M ... A congested market or popular program
#    still hits its 12M per-account ceiling at the same point."
# Raising blocks to 100M bought PARALLEL capacity across unrelated accounts. It does nothing
# for a program where every transaction writes the same account.
#
# SIMD-0306 would raise the per-account limit (title 20M; body argues 40% of block = 40M).
# Status: Review, NOT activated. If it lands, multiply every TPS below by 1.7x or 3.3x.
MAX_WRITABLE_ACCOUNT_UNITS=12000000
MAX_BLOCK_UNITS=100000000
SLOT_SECONDS=0.4

say() { printf '\n\033[1;36m%s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m%s\033[0m\n' "$*"; }

# --- the validator check that actually works ----------------------------------------------
#
# `pgrep -a solana-test-validator` silently matches nothing: the process name is over 15
# characters. `pgrep -f solana-test-validator` matches its own shell. Both produce a wrong
# answer, in opposite directions. The bracket makes the pattern not match itself.
if ps aux | grep -q "[s]olana-test-validator"; then
  warn "A solana-test-validator is running."
  warn "Building alongside it is what OOM-killed WSL2 on 2026-08-24. Stop it first."
  exit 1
fi

say "Measuring compute units (LiteSVM, no validator)"
RAW="$(mktemp)"
trap 'rm -f "$RAW"' EXIT
cargo test -p solfx-core --test compute_budget -- --nocapture 2>&1 | tee "$RAW" | grep -E "CU +\(ceiling" || true

if ! grep -qE "CU +\(ceiling" "$RAW"; then
  warn "No CU lines captured. Did the compute_budget tests run? Check the output above."
  exit 1
fi

say "Concurrency ceiling per instruction"
cat <<EOF

  Every SolFX trade writes these accounts, so the per-account limit applies to ALL of them
  simultaneously — the binding constraint is whichever is hottest, and they are all equally hot:

    protocol, lp_pool, insurance_fund,
    collateral_vault, lp_vault, insurance_vault, fee_vault      <- GLOBAL, every trade
    market                                                      <- per trading pair
    user_account, position                                      <- per user, no contention

  Because 'protocol' is written by every trade on every market, the figures below are
  PROTOCOL-WIDE, not per market. Listing more markets does not raise them.

EOF

awk -v acct="$MAX_WRITABLE_ACCOUNT_UNITS" -v blk="$MAX_BLOCK_UNITS" -v slot="$SLOT_SECONDS" '
/CU +\(ceiling/ {
  cu = ""
  for (i = 1; i <= NF; i++) if ($i == "CU") { cu = $(i-1); idx = i - 1; break }
  if (cu == "" || cu + 0 <= 0) next
  name = ""
  for (j = 1; j < idx; j++) name = name (j > 1 ? " " : "") $j
  # de-duplicate: the same instruction is measured in several phase sections
  if (seen[name]++) next
  txblock = acct / cu
  tps     = txblock / slot
  par     = blk / cu / slot
  # The headline must come from the TRADER HOT PATH only. Admin and one-time instructions
  # (initialize_protocol, initialize_market, pause/unpause) cost more than a trade but nobody
  # ever contends on them. Letting them set the headline would name an instruction that runs
  # exactly once in the lifetime of the protocol as the bottleneck.
  hot = (name ~ /position|trigger|deleverage/)
  printf "  %-36s %7d CU %s %6.0f tx/block   %8.0f TPS   (%.0f TPS if fully parallel)\n", \
         name, cu, (hot ? "*" : " "), txblock, tps, par
  if (hot && (tps < min_tps || min_tps == 0)) { min_tps = tps; min_name = name; min_cu = cu }
}
END {
  printf "\n"
  printf "  \033[1mHEADLINE\033[0m\n"
  printf "  Worst instruction on the trader hot path (*): %s at %d CU\n", min_name, min_cu
  printf "  \033[1;31m~%.0f transactions/second protocol-wide\033[0m, limited by the 12M CU\n", min_tps
  printf "  per-writable-account limit on the seven global accounts listed above.\n\n"
  printf "  The right-hand column is what the SAME instruction could reach if it touched no\n"
  printf "  global state at all (bounded only by the 100M block limit). The gap between the\n"
  printf "  two columns is what removing global state from the hot path would buy.\n"
}
' "$RAW"

cat <<'EOF'

  ------------------------------------------------------------------------------------------
  WHAT THIS DOES AND DOES NOT TELL YOU

  Does:   the hard ceiling imposed by Solana consensus on this account layout. No amount of
          client concurrency, RPC capacity or validator hardware exceeds it.

  Does not: whether the protocol behaves correctly under concurrent load. Write-lock
          contention shows up as dropped or retried transactions, not wrong answers — but
          races between liquidation and close, or deposit and withdraw, need a real validator.
          That is a separate test.

  On "how many concurrent users": the number of USERS is unbounded — user_account and position
  are per-user PDAs with no contention. What is bounded is simultaneous TRADES. Because every
  trade writes 'protocol', no two SolFX trades ever execute in parallel; they queue. Users
  beyond the ceiling are not rejected, they wait.
  ------------------------------------------------------------------------------------------
EOF
