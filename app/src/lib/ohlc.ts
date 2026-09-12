/**
 * Real OHLC candles from Pyth Pro's History API.
 *
 * This replaces the earlier approach of sampling Hermes once per point and bucketing the
 * results: that cost one request per sample, capped resolution at what the rate limit
 * allowed, and produced wicks that were only the extremes *of what was sampled*. These are
 * Pyth's own candles, one request per chart, at any resolution TradingView understands.
 *
 * Two details that are easy to get wrong, both learned by hitting them:
 *
 *  - **Channel.** Every feed declares a `min_channel` and supports that channel *and slower
 *    ones*. XAU, XAG and USD/CNH declare `fixed_rate@200ms`, so asking for `real_time`
 *    returns 404 "symbol not found" even though `/v1/symbols` lists them as stable.
 *    `fixed_rate@200ms` is supported by all six of our markets.
 *  - **Symbols are fully qualified.** `FX.EUR/USD`, `Metal.XAU/USD`, `Crypto.BTC/USD` — the
 *    bare pair is not accepted, and the prefix differs by asset class.
 */
const PRICE_PRECISION = 1_000_000_000n;

/** A `PRICE_PRECISION` price as a float, for charting only — never for sizing. */
export function toDisplay(price: bigint): number {
  return Number(price) / Number(PRICE_PRECISION);
}

export type Ohlc = {
  readonly time: number;
  readonly open: number;
  readonly high: number;
  readonly low: number;
  readonly close: number;
};

/** Resolutions the History API accepts, with how far back each is worth asking for. */
export const TIMEFRAMES = [
  { label: "15m", resolution: "15", seconds: 900, span: 900 * 200 },
  { label: "30m", resolution: "30", seconds: 1_800, span: 1_800 * 200 },
  { label: "1H", resolution: "60", seconds: 3_600, span: 3_600 * 200 },
  { label: "4H", resolution: "240", seconds: 14_400, span: 14_400 * 200 },
  { label: "1D", resolution: "D", seconds: 86_400, span: 86_400 * 200 },
] as const;

export type Timeframe = (typeof TIMEFRAMES)[number];

/** The channel every one of our feeds supports. See the note above. */
const CHANNEL = "fixed_rate@200ms";

/**
 * Pyth's fully-qualified symbol for one of our market symbols.
 *
 * The prefix is per asset class and the bare pair is never accepted — `EUR/USD` is a 404,
 * `FX.EUR/USD` is the feed. The short name is not unique either: `FX.Index.EUR/USD` exists
 * and is a different instrument.
 */
const METALS = new Set(["XAU", "XAG", "XPT", "XPD"]);
const CRYPTO = new Set(["BTC", "ETH", "SOL"]);

export function pythSymbol(symbol: string): string {
  const s = symbol.toUpperCase();
  const base = s.split("/")[0] ?? s;
  if (METALS.has(base)) return `Metal.${s}`;
  if (CRYPTO.has(base)) return `Crypto.${s}`;
  return `FX.${s}`;
}

/** UDF response. `s` is "ok", "no_data" or "error". */
type Udf = {
  s: string;
  t?: number[];
  o?: number[];
  h?: number[];
  l?: number[];
  c?: number[];
  errmsg?: string;
};

export async function fetchOhlc(
  baseUrl: string,
  symbol: string,
  tf: Timeframe,
  now = Math.floor(Date.now() / 1000)
): Promise<Ohlc[]> {
  const url =
    `${baseUrl.replace(/\/$/, "")}/${CHANNEL}/history` +
    `?symbol=${encodeURIComponent(pythSymbol(symbol))}` +
    `&resolution=${tf.resolution}&from=${now - tf.span}&to=${now}`;

  const res = await fetch(url);
  if (!res.ok) return [];
  const d = (await res.json()) as Udf;
  if (d.s !== "ok" || !d.t?.length) return [];

  const out: Ohlc[] = [];
  for (let i = 0; i < d.t.length; i++) {
    const time = d.t[i];
    const open = d.o?.[i];
    const high = d.h?.[i];
    const low = d.l?.[i];
    const close = d.c?.[i];
    if (
      time === undefined ||
      open === undefined ||
      high === undefined ||
      low === undefined ||
      close === undefined
    ) {
      continue;
    }
    out.push({ time, open, high, low, close });
  }
  return out;
}
