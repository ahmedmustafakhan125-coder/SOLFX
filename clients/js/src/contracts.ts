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
import { BASE_PRECISION, PRICE_PRECISION } from "./constants.js";

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

/**
 * One pip, at `PRICE_PRECISION`.
 *
 * The other half of the same problem `unitsPerLot` solves: a presentation convention the
 * engine has no opinion about, which every client must nevertheless agree on. A trader reads
 * "+32 pips" and compares it against their existing broker, so getting the decade wrong is
 * not a rounding error, it is a different number entirely.
 *
 * | Class            | 1 pip  | Price units |
 * |------------------|--------|-------------|
 * | FX, 4-decimal    | 0.0001 | 100,000     |
 * | FX, JPY-quoted   | 0.01   | 10,000,000  |
 * | Gold, silver     | 0.01   | 10,000,000  |
 * | Crypto           | 1.00   | 1e9         |
 *
 * JPY pairs are the trap: quoted to three decimals rather than five, so their pip is two
 * decades larger than every other pair's, and a single table that ignores that reports a
 * USD/JPY move as a hundred times what a trader would call it.
 *
 * Crypto has no pip convention at all — brokers quote it in dollars — so one unit of the quote
 * currency is used, which makes "pips" and "dollars per coin" the same number and keeps the
 * display honest rather than inventing a scale.
 */
export function pipSize(symbol: string): bigint {
  const s = symbol.toUpperCase().replace("/", "");
  if (s.startsWith("BTC") || s.startsWith("ETH") || s.startsWith("SOL")) {
    return PRICE_PRECISION;
  }
  if (s.startsWith("XAU") || s.startsWith("XAG") || s.startsWith("XPT") || s.startsWith("XPD")) {
    return PRICE_PRECISION / 100n;
  }
  if (s.endsWith("JPY")) return PRICE_PRECISION / 100n;
  return PRICE_PRECISION / 10_000n;
}

/**
 * A price move expressed in pips, to one decimal place, as an integer of tenths.
 *
 * Returned as tenths rather than a float so nothing here touches floating point; the caller
 * formats it. Rounds toward zero, which understates the move in both directions and so cannot
 * flatter a trade in either.
 */
export function pipsTenths(symbol: string, priceMove: bigint): bigint {
  return (priceMove * 10n) / pipSize(symbol);
}
