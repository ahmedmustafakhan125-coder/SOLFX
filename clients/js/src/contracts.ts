/**
 * Contract sizes — the table the IDL cannot describe.
 *
 * `Market` stores no contract size, because the engine works in base units and does not need
 * one. Every *client* needs one, and they must agree. This is a port of
 * `crates/solfx-keeper/src/contracts.rs`, pinned against the same vectors in tests so the CLI
 * and the browser cannot drift.
 *
 * | Class  | 1 lot        | Source           |
 * |--------|--------------|------------------|
 * | FX     | 100,000 base | MT4/MT5 standard |
 * | Gold   | 100 troy oz  | XAUUSD CFD       |
 * | Silver | 5,000 oz     | common MT5 figure, not broker-verified |
 * | Crypto | 1 coin       | BTCUSD = 1 BTC at XM and Exness |
 * | Oil    | 1,000 bbl    | common MT5 figure, not broker-verified |
 *
 * Applying the FX number to Bitcoin makes `0.01 lots` mean 1,000 BTC instead of 0.01 — five
 * orders of magnitude, and it reads as a plausible order the whole way.
 */
import { BASE_PRECISION } from "./constants.js";

/** Units of the base asset in one standard lot. */
export function unitsPerLot(symbol: string): bigint {
  const s = symbol.toUpperCase();
  if (s.startsWith("XAU")) return 100n;
  if (s.startsWith("XAG")) return 5_000n;
  if (s.startsWith("XPT") || s.startsWith("XPD")) return 100n;
  if (s.startsWith("BTC") || s.startsWith("ETH") || s.startsWith("SOL")) return 1n;
  if (s.includes("OIL")) return 1_000n;
  return 100_000n;
}

/** Base units in one lot, at `BASE_PRECISION`. */
export function oneLotBase(symbol: string): bigint {
  return unitsPerLot(symbol) * BASE_PRECISION;
}

/** The largest single position, in base units: 100 lots. */
export function maxPositionBase(symbol: string): bigint {
  return oneLotBase(symbol) * 100n;
}

/**
 * The smallest single position, in base units: a millionth of a lot.
 *
 * Deliberately far below anything tradeable. A base-unit floor cannot express a *value*
 * floor — an ounce of gold and a euro differ by orders of magnitude — so the real minimum is
 * the program's $1 notional check, which applies uniformly. This only stops dust.
 */
export function minPositionBase(symbol: string): bigint {
  const millionth = oneLotBase(symbol) / 1_000_000n;
  return millionth > 0n ? millionth : 1n;
}
