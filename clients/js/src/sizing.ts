/**
 * The three ways a trader states a size, and the integer arithmetic that turns each into
 * base units. A port of the conversions in `crates/solfx-keeper/src/bin/trade.rs`.
 *
 * # No floating point, anywhere
 *
 * `0.07` is not representable in binary, so `0.07 * 10_000` is `699.9999...` and truncating
 * it sizes the trade a ten-thousandth of a lot short. Nobody would ever see it. Decimals are
 * parsed digit by digit into integer ten-thousandths and every step from there is `bigint`.
 */
import { BASE_PRECISION, NOTIONAL_DIVISOR, ONE_USDC } from "./constants.js";
import { unitsPerLot } from "./contracts.js";

/** Resolution of a decimal size. Finer than any venue quotes. */
const PLACES = 4;
const SCALE = 10_000n; // 10 ** PLACES

export class SizingError extends Error {}

/**
 * Parse a positive decimal with at most four places into integer ten-thousandths.
 * Rejects anything that is not a plain decimal — no exponents, no signs, no whitespace
 * beyond the ends.
 */
function toTenThousandths(text: string, label: string): bigint {
  const t = text.trim();
  if (t === "") throw new SizingError(`no ${label} given`);
  if (t.startsWith("-")) throw new SizingError(`${label} must be positive`);

  const dot = t.indexOf(".");
  const whole = dot === -1 ? t : t.slice(0, dot);
  const frac = dot === -1 ? "" : t.slice(dot + 1);

  const digits = (s: string) => s === "" || /^[0-9]+$/.test(s);
  if (!digits(whole) || !digits(frac)) {
    throw new SizingError(`${text} is not a decimal number`);
  }
  if (frac.length > PLACES) {
    throw new SizingError(
      `at most ${PLACES} decimal places; ${text} is finer than one ten-thousandth`,
    );
  }

  const w = whole === "" ? 0n : BigInt(whole);
  const f = frac === "" ? 0n : BigInt(frac.padEnd(PLACES, "0"));
  const ten = w * SCALE + f;
  if (ten === 0n) throw new SizingError(`${label} must be greater than zero`);
  return ten;
}

/**
 * Lots to base units. `unitsPerLot` differs per asset class, so it is passed in rather
 * than assumed to be FX.
 */
export function lotsToBase(lots: string, unitsPerLotValue: bigint): bigint {
  const ten = toTenThousandths(lots, "size");
  const base = (unitsPerLotValue * BASE_PRECISION * ten) / SCALE;
  if (base === 0n) throw new SizingError(`${lots} lots rounds to zero base units`);
  return base;
}

/**
 * Units of the base asset to base units — a straight scale by `BASE_PRECISION`.
 *
 * The one mode needing neither market data nor the contract table: 1,000 EUR is 1,000 EUR
 * regardless of price, lot convention or asset class. Implemented as a lot of unit size so
 * the two modes cannot round differently.
 */
export function quantityToBase(quantity: string): bigint {
  return lotsToBase(quantity, 1n);
}

/**
 * Whole USD of exposure to base units, inverting the notional formula.
 *
 * `notional = size_base * price / NOTIONAL_DIVISOR`, so
 * `size_base = notional * NOTIONAL_DIVISOR / price`.
 *
 * `price` is at `PRICE_PRECISION`.
 */
export function notionalToBase(notionalUsd: bigint, price: bigint): bigint {
  if (notionalUsd <= 0n) throw new SizingError("notional must be greater than zero");
  if (price <= 0n) throw new SizingError("price must be positive");
  const size = (notionalUsd * ONE_USDC * NOTIONAL_DIVISOR) / price;
  if (size === 0n) {
    throw new SizingError(`$${notionalUsd} is too small to buy one base unit at this price`);
  }
  return size;
}

export type Sizing =
  | { readonly mode: "lots"; readonly value: string }
  | { readonly mode: "quantity"; readonly value: string }
  | { readonly mode: "notional"; readonly usd: bigint };

/**
 * Resolve any of the three modes for a market. `price` is only consulted by `notional`,
 * so the other two work with no oracle read at all.
 */
export function resolveSize(symbol: string, sizing: Sizing, price?: bigint): bigint {
  switch (sizing.mode) {
    case "lots":
      return lotsToBase(sizing.value, unitsPerLot(symbol));
    case "quantity":
      return quantityToBase(sizing.value);
    case "notional":
      if (price === undefined) {
        throw new SizingError("sizing by notional needs the current price");
      }
      return notionalToBase(sizing.usd, price);
  }
}
