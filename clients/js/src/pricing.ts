/**
 * Execution pricing, ported from `crates/solfx-math/src/pricing.rs` and the parts of
 * `open_position.rs` that compose it.
 *
 * # Why this exists
 *
 * The terminal used to size margin against the **oracle mid**: `margin = notional / leverage`,
 * with `notional = size * mid`. The program does neither of those things. It prices the fill
 * at `execution_price` — mid widened by the spread, against the trader on both sides — takes
 * the notional of *that* with `mul_div_ceil`, and then checks
 * `effective_leverage = div_ceil(notional, collateral) <= max_leverage`.
 *
 * Three ceilings and one floor, all pointing the same way, so a ticket sized off the mid
 * always under-collateralises by a hair. At any leverage below the market's maximum the slack
 * absorbs it. At exactly the maximum it does not, and the open is refused with
 * `LeverageTooHigh` — measured on devnet against ETH/USD at 10x on a 10x market.
 *
 * So this is not a display convenience. Sizing has to agree with the chain or the ticket
 * quotes trades the venue will not accept.
 *
 * Everything is `bigint`, and every rounding direction matches the Rust function it names.
 * No floats, for the reason `crates/solfx-math` has none.
 */
import { Direction } from "./generated/index.js";
import { BPS_PRECISION, NOTIONAL_DIVISOR } from "./constants.js";

/** `pricing::MAX_TOTAL_SPREAD_BPS` — 50%. */
export const MAX_TOTAL_SPREAD_BPS = 5_000n;

/** `constants::LOT_SIZE_BASE` — 100,000 base units at 1e9. */
const LOT_SIZE_BASE = 100_000_000_000_000n; // 1e14

/** `constants::RATE_PRECISION`. */
const RATE_PRECISION = 1_000_000_000n; // 1e9

/** Which way the trade crosses the book. Mirrors `types::Side`. */
export type Side = "buy" | "sell";

/** Mirrors `Side::resolve(direction, TradeAction::Open)`. */
export function sideForOpen(direction: Direction): Side {
  return direction === Direction.Long ? "buy" : "sell";
}

/** Mirrors `Side::resolve(direction, TradeAction::Close)` — a close crosses the other way. */
export function sideForClose(direction: Direction): Side {
  return direction === Direction.Long ? "sell" : "buy";
}

/** `a * b / d` rounded **up**. Mirrors `fixed::mul_div_ceil`. */
function mulDivCeil(a: bigint, b: bigint, d: bigint): bigint {
  const n = a * b;
  return n / d + (n % d === 0n ? 0n : 1n);
}

/** `a / d` rounded **up**. Mirrors `fixed::div_ceil`. */
function divCeil(a: bigint, d: bigint): bigint {
  return a / d + (a % d === 0n ? 0n : 1n);
}

function abs(v: bigint): bigint {
  return v < 0n ? -v : v;
}

/**
 * Confidence as a fraction of price, in bps. Mirrors the `conf_bps` term of
 * `oracle::ValidatedPrice::new`, which rounds **up** — a wider band is the cautious read.
 */
export function confBps(price: bigint, conf: bigint): bigint {
  if (price <= 0n) return 0n;
  return mulDivCeil(conf, BPS_PRECISION, price);
}

/** Mirrors `pricing::confidence_spread_bps`. Rounded **up**. */
export function confidenceSpreadBps(
  confidenceBps: bigint,
  multiplierBps: number,
): bigint {
  return mulDivCeil(confidenceBps, BigInt(multiplierBps), BPS_PRECISION);
}

/**
 * How much this trade worsens the book's imbalance, in base units.
 * Mirrors `pricing::skew_delta`. Negative means the trade helps balance the book.
 */
export function skewDelta(
  baseOiLong: bigint,
  baseOiShort: bigint,
  signedSizeDelta: bigint,
): bigint {
  const before = baseOiLong - baseOiShort;
  return abs(before + signedSizeDelta) - abs(before);
}

/**
 * The inventory-risk portion of the spread. Mirrors `pricing::skew_impact_bps`.
 *
 * A trade that *improves* the balance is charged nothing rather than paid a rebate
 * (`ARCHITECTURE.md` § 6.6's `max(skew_impact, 0)`), so this never narrows a quote.
 */
export function skewImpactBps(delta: bigint, ratePerUnit: number): bigint {
  if (delta <= 0n || ratePerUnit === 0) return 0n;
  return mulDivCeil(delta, BigInt(ratePerUnit), LOT_SIZE_BASE * RATE_PRECISION);
}

/**
 * Mirrors `pricing::total_spread_bps`. Returns `undefined` past the 50% ceiling, where the
 * Rust returns an error — the caller must not fall back to a narrower quote.
 */
export function totalSpreadBps(
  baseSpreadBps: number,
  confidenceSpread: bigint,
  skewImpact: bigint,
): bigint | undefined {
  const total = BigInt(baseSpreadBps) + confidenceSpread + skewImpact;
  return total > MAX_TOTAL_SPREAD_BPS ? undefined : total;
}

/**
 * The price the trader actually fills at. Mirrors `pricing::execution_price`.
 *
 * The adjustment is ceiling-rounded and then applied *against* the trader in both
 * directions, which is what makes a same-price round trip strictly loss-making.
 */
export function executionPrice(
  oraclePrice: bigint,
  spreadBps: bigint,
  side: Side,
): bigint | undefined {
  if (spreadBps > MAX_TOTAL_SPREAD_BPS) return undefined;
  if (oraclePrice <= 0n) return undefined;
  const adjustment = mulDivCeil(oraclePrice, spreadBps, BPS_PRECISION);
  const exec =
    side === "buy" ? oraclePrice + adjustment : oraclePrice - adjustment;
  return exec <= 0n ? undefined : exec;
}

/**
 * `size_base * price / 1e12`, rounded **up**. Mirrors `pnl::notional_in_quote`.
 *
 * The ceiling matters: the terminal previously floored this, so its notional was up to one
 * micro-USDC below the program's before the spread was even considered.
 */
export function notionalInQuote(sizeBase: bigint, price: bigint): bigint {
  return mulDivCeil(sizeBase, price, NOTIONAL_DIVISOR);
}

/** Mirrors `margin::effective_leverage` — `notional / collateral`, rounded **up**. */
export function effectiveLeverage(
  notional: bigint,
  collateral: bigint,
): bigint | undefined {
  return collateral === 0n ? undefined : divCeil(notional, collateral);
}

/** Mirrors `margin::initial_margin`. Rounded **up**. */
export function initialMargin(notional: bigint, imrBps: number): bigint {
  return mulDivCeil(notional, BigInt(imrBps), BPS_PRECISION);
}

/** The market fields execution pricing depends on. A subset of the generated `Market`. */
export type SpreadParams = {
  readonly baseSpreadBps: number;
  readonly confSpreadMultiplierBps: number;
  readonly skewImpactBpsPerUnit: number;
  readonly baseOiLong: bigint;
  readonly baseOiShort: bigint;
};

export type OpenQuote = {
  /** Total spread applied, in bps: base + confidence + skew. */
  readonly spreadBps: bigint;
  /** The fill price, at PRICE_PRECISION. */
  readonly execPrice: bigint;
  /** Notional at the fill price — the figure every downstream check uses. */
  readonly notional: bigint;
};

/**
 * Reproduce `open_position`'s pricing for one side, exactly as the program will compute it.
 *
 * `confidenceBps` comes from the live price account via {@link confBps}. Pass `0n` only if
 * the confidence is genuinely unknown; understating it understates the spread.
 *
 * **Simple markets only.** A synthetic cross composes two feeds before this point, and its
 * spread is taken on the composed price. Nothing listed today is synthetic; when one is, it
 * needs the composition, not this function.
 */
export function quoteOpen(args: {
  readonly market: SpreadParams;
  readonly oraclePrice: bigint;
  readonly confidenceBps: bigint;
  readonly sizeBase: bigint;
  readonly side: Side;
}): OpenQuote | undefined {
  const { market, oraclePrice, confidenceBps, sizeBase, side } = args;

  const signedDelta = side === "buy" ? sizeBase : -sizeBase;
  const spread = totalSpreadBps(
    market.baseSpreadBps,
    confidenceSpreadBps(confidenceBps, market.confSpreadMultiplierBps),
    skewImpactBps(
      skewDelta(market.baseOiLong, market.baseOiShort, signedDelta),
      market.skewImpactBpsPerUnit,
    ),
  );
  if (spread === undefined) return undefined;

  const execPrice = executionPrice(oraclePrice, spread, side);
  if (execPrice === undefined) return undefined;

  return {
    spreadBps: spread,
    execPrice,
    notional: notionalInQuote(sizeBase, execPrice),
  };
}

/**
 * The smallest collateral the program will accept for this notional at this leverage.
 *
 * `open_position` applies two checks that "express the same constraint" but are both
 * enforced, so both are satisfied here:
 *
 *   - `div_ceil(notional, collateral) <= max_leverage`  → `LeverageTooHigh`
 *   - `collateral >= ceil(notional * imr_bps / 1e4)`    → `InsufficientMargin`
 *
 * Taking the larger is what lets a trader select the market's maximum leverage and have the
 * open actually land. The delivered leverage is then a whisker under the number on the
 * slider, which is the honest outcome — `div_ceil` means exact maximum leverage is not a
 * reachable state, not that the ticket should quote it and fail.
 */
export function minCollateralFor(args: {
  readonly notional: bigint;
  readonly leverage: number;
  readonly imrBps: number;
}): bigint {
  const byLeverage = divCeil(args.notional, BigInt(args.leverage));
  const byImr = initialMargin(args.notional, args.imrBps);
  return byLeverage > byImr ? byLeverage : byImr;
}
