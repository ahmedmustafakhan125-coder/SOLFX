/**
 * Carry — the FX swap, and the number brokers are least transparent about.
 *
 * Ported from `crates/solfx-math/src/funding.rs`. §10.2 asks for the swap rate shown with
 * **the markup broken out**: the true interest-rate differential on one line and SolFX's
 * charge on another. That is a concrete, verifiable improvement on XM and Exness, who quote a
 * single blended number, and it is cheap — the two components are already separate fields on
 * `Market` precisely so a UI can show them apart.
 *
 * All `bigint`, no floats, same reasons as `margin.ts`.
 */
import { Direction } from "./generated/index.js";

const HOURS_PER_YEAR = 365n * 24n;
const RATE_PRECISION = 1_000_000_000n;
const NOTIONAL_DIVISOR = 1_000_000_000_000n;

/** `a * b / d` rounded **up** for positives, toward zero for negatives — `mul_div_ceil_signed`. */
function mulDivCeilSigned(a: bigint, b: bigint, d: bigint): bigint {
  const n = a * b;
  const q = n / d;
  return n % d > 0n ? q + 1n : q;
}

export type CarryRate = {
  /** The interest-rate differential, per hour at `RATE_PRECISION`. Signed. */
  readonly differential: bigint;
  /** SolFX's markup, per hour at `RATE_PRECISION`. Always a charge, never negative. */
  readonly markup: bigint;
  /** What the trader actually pays per hour: `differential + markup`. */
  readonly total: bigint;
};

/**
 * Mirrors `funding::carry_cost_rate_per_hour`, keeping the two components separate.
 *
 * The differential flips sign for a short, because a short has the mirrored exposure. The
 * markup does not: it is charged on both directions.
 */
export function carryRatePerHour(args: {
  /** Annual rate on the base currency at `RATE_PRECISION`. */
  readonly baseAnnual: bigint;
  /** Annual rate on the quote currency at `RATE_PRECISION`. */
  readonly quoteAnnual: bigint;
  /** `Market.carryRatePerHour` — the protocol's markup only. */
  readonly markupPerHour: bigint;
  readonly direction: Direction;
}): CarryRate {
  const sign = args.direction === Direction.Long ? 1n : -1n;
  const directional = (args.quoteAnnual - args.baseAnnual) * sign;
  const differential = mulDivCeilSigned(directional, 1n, HOURS_PER_YEAR);
  return {
    differential,
    markup: args.markupPerHour,
    total: differential + args.markupPerHour,
  };
}

/**
 * Carry charged on `notional` over `hours`, in quote units.
 *
 * Clamped at zero to match `advance_carry_index`, which refuses to run the index backwards: a
 * negative differential large enough to make the total negative would mean paying a trader to
 * hold leveraged notional, which the program treats as a configuration error rather than a
 * feature. Showing a negative estimate here would promise something the chain will not pay.
 */
export function carryOver(notional: bigint, ratePerHour: bigint, hours: bigint): bigint {
  if (ratePerHour <= 0n || hours <= 0n) return 0n;
  return (notional * ratePerHour * hours) / RATE_PRECISION;
}

/** Notional in quote units from base size and price, mirroring the engine's divisor. */
export function notionalOf(sizeBase: bigint, price: bigint): bigint {
  return (sizeBase * price) / NOTIONAL_DIVISOR;
}
