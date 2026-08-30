/** Fixed-point scales the program works in. Never mix these silently. */
export const QUOTE_PRECISION = 1_000_000n; // 1e6, USDC
export const BASE_PRECISION = 1_000_000_000n; // 1e9
export const PRICE_PRECISION = 1_000_000_000n; // 1e9
export const BPS_PRECISION = 10_000n;

/** `notional_quote = size_base * price / NOTIONAL_DIVISOR`. */
export const NOTIONAL_DIVISOR = 1_000_000_000_000n; // 1e12

/** Alias kept for readability at call sites that mean money rather than a scale. */
export const ONE_USDC = QUOTE_PRECISION;
