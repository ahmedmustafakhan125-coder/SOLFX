/**
 * Margin arithmetic the terminal needs to *display*, ported from `crates/solfx-math`.
 *
 * # Why this is a port and not a reimplementation
 *
 * The liquidation price is the number a trader actually watches, so it has to agree with the
 * chain exactly — a display that is one unit optimistic is a display that says "safe" on a
 * position the keeper is about to close. Every function here mirrors a named Rust function,
 * keeps its rounding direction, and is checked against Rust-derived values in the tests.
 *
 * Everything is `bigint`. No floats appear anywhere, for the same reason they appear nowhere
 * in `solfx-math`: a `number` cannot hold a u64 exactly, and a rounding step in the wrong
 * direction here is a rounding step in the wrong direction on a liquidation price.
 *
 * This computes *nothing the program relies on*. The chain re-derives all of it. A drift
 * between the two is a display bug, never a safety one — but a display bug on this particular
 * number is still the worst kind of display bug.
 */
import { Direction } from "./generated/index.js";

const BPS_PRECISION = 10_000n;
const NOTIONAL_DIVISOR = 1_000_000_000_000n;

/** `< $100k → 1.0x`, `< $1M → 1.5x`, `< $5M → 2.5x`, else `4.0x`. Upper bounds exclusive. */
const MMR_TIERS: readonly (readonly [bigint, bigint])[] = [
  [100_000_000_000n, 10_000n],
  [1_000_000_000_000n, 15_000n],
  [5_000_000_000_000n, 25_000n],
];

/** Mirrors `margin::mmr_multiplier_bps`. `10_000` = 1.0x. */
export function mmrMultiplierBps(notional: bigint): bigint {
  for (const [bound, multiplier] of MMR_TIERS) {
    if (notional < bound) return multiplier;
  }
  return 40_000n;
}

/** `a * b / d` rounded **up**. Mirrors `fixed::mul_div_ceil`; used for anything charged. */
function mulDivCeil(a: bigint, b: bigint, d: bigint): bigint {
  const n = a * b;
  return n / d + (n % d === 0n ? 0n : 1n);
}

/**
 * `a * b / d` rounded toward **−∞**. Mirrors `fixed::mul_div_floor_signed`.
 *
 * JavaScript's `BigInt` division truncates toward zero exactly as Rust's does, so `-7n / 2n`
 * is `-3n` and the correction below is as necessary here as it is there.
 */
function mulDivFloorSigned(a: bigint, b: bigint, d: bigint): bigint {
  const n = a * b;
  const q = n / d;
  return n % d < 0n ? q - 1n : q;
}

/** Mirrors `margin::maintenance_margin`. Rounded **up**, twice. */
export function maintenanceMargin(notional: bigint, mmrBps: number): bigint {
  const base = mulDivCeil(notional, BigInt(mmrBps), BPS_PRECISION);
  return mulDivCeil(base, mmrMultiplierBps(notional), BPS_PRECISION);
}

/** Costs already accrued against the position, mirroring `margin::AccruedCosts`. */
export type AccruedCosts = {
  /** Carry, in USDC. Always a charge. */
  readonly carry: bigint;
  /** Funding owed, in USDC. Signed — the light side of the book *receives*. */
  readonly funding: bigint;
  /** Estimated cost to close, in USDC. */
  readonly closeFee: bigint;
};

/**
 * The price at which the position becomes liquidatable.
 *
 * Mirrors `margin::liquidation_price`: solves `equity(price) == maintenance_margin` for price,
 * holding size and costs fixed.
 *
 * **Only exact for USD-quoted markets.** For a market quoted in something else — USD/JPY and
 * USD/CNH among the listed ones — the liquidation price also moves with the conversion rate,
 * and this holds that rate constant. The Rust function carries the same restriction and says
 * the caller must solve that case itself. Callers should label the result accordingly rather
 * than presenting it as exact; see `liquidationPriceIsExact`.
 *
 * Returns `1n` for a position already past liquidation, matching the Rust clamp, and
 * `undefined` where the Rust returns an error rather than a price.
 */
export function liquidationPrice(args: {
  readonly sizeBase: bigint;
  readonly entryPrice: bigint;
  readonly collateral: bigint;
  readonly maintenanceMargin: bigint;
  readonly costs: AccruedCosts;
  readonly direction: Direction;
}): bigint | undefined {
  if (args.sizeBase === 0n) return undefined;
  if (args.entryPrice <= 0n) return undefined;

  // How much PnL the position can lose before equity reaches the maintenance margin.
  const budget =
    args.collateral -
    args.costs.carry -
    args.costs.funding -
    args.costs.closeFee -
    args.maintenanceMargin;

  // That quote-unit budget converted back into a price move.
  const moveMagnitude = mulDivFloorSigned(budget, NOTIONAL_DIVISOR, args.sizeBase);

  // A long is liquidated by a fall, a short by a rise.
  const sign = args.direction === Direction.Long ? 1n : -1n;
  const liq = args.entryPrice - moveMagnitude * sign;

  return liq <= 0n ? 1n : liq;
}

/**
 * Is a liquidation price for this market exact, or does it assume a frozen conversion rate?
 *
 * True only when the market is quoted in USD. `EURUSD` is (the quote currency is the second
 * half); `USDJPY` is not.
 */
export function liquidationPriceIsExact(symbol: string): boolean {
  return symbol.toUpperCase().replace("/", "").endsWith("USD");
}
