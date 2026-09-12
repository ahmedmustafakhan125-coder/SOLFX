/**
 * Formatting for on-chain integers. No floating point in any conversion that produces a
 * number a trader acts on — the program works in fixed-point and so does this.
 */
const PRICE_PRECISION = 1_000_000_000n; // 1e9
const QUOTE_PRECISION = 1_000_000n; // 1e6
const BASE_PRECISION = 1_000_000_000n; // 1e9

/** Split a fixed-point integer into whole and fractional parts, exactly. */
function parts(value: bigint, scale: bigint, places: number): string {
  const neg = value < 0n;
  const v = neg ? -value : value;
  const whole = v / scale;
  const frac = v % scale;
  const digits = frac
    .toString()
    .padStart(scale.toString().length - 1, "0")
    .slice(0, places);
  const grouped = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${neg ? "-" : ""}${grouped}${places > 0 ? `.${digits}` : ""}`;
}

/** A price at PRICE_PRECISION. FX wants five decimals, everything else two. */
export function fmtPrice(price: bigint, places = 5): string {
  return parts(price, PRICE_PRECISION, places);
}

/** USDC at QUOTE_PRECISION. */
export function fmtUsd(amount: bigint, places = 2): string {
  return parts(amount, QUOTE_PRECISION, places);
}

/** Base units at BASE_PRECISION, as a quantity of the base asset. */
export function fmtBase(size: bigint, places = 4): string {
  return parts(size, BASE_PRECISION, places);
}

export function fmtBps(bps: number): string {
  return `${(bps / 100).toFixed(2)}%`;
}

/** Decimal places a symbol is conventionally quoted to. */
export function priceplaces(symbol: string): number {
  const s = symbol.toUpperCase();
  if (s.includes("JPY")) return 3;
  if (
    s.startsWith("BTC") ||
    s.startsWith("ETH") ||
    s.startsWith("XAU") ||
    s.startsWith("XAG")
  ) {
    return 2;
  }
  return 5;
}

/**
 * A fee rate stored at RATE_PRECISION (1e9) as basis points.
 *
 * Shown in bps rather than percent because the real figures are small — 80,000 is 0.8 bps,
 * which rounds to "0.01%" at two decimal places and stops being informative. That precision
 * matters: it is a tier in the § 8.2 schedule, not a rounding artefact.
 */
export function fmtRateBps(rate: bigint): string {
  // rate / 1e9 * 10_000 == rate / 100_000, kept as integer tenths of a bp for exactness.
  const tenths = (rate * 10n) / 100_000n;
  return `${(Number(tenths) / 10).toFixed(2)} bps`;
}
