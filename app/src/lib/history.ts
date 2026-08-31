/**
 * Price history for the chart, from the same oracle the program trades on.
 *
 * There is deliberately no third-party feed here. A chart drawn from an exchange API would
 * disagree with the price the program actually fills at, and a chart that disagrees with
 * execution is worse than no chart. These are Pyth's own numbers — the identical feed the
 * price poster publishes on chain.
 *
 * Pyth Benchmarks' TradingView shim (benchmarks.pyth.network/v1/shims/tradingview) now 404s,
 * so history comes from Hermes' `/v2/updates/price/{publish_time}`, one request per point.
 * That is the constraint on resolution: bars cost requests, and Hermes rate-limits.
 */
const PRICE_PRECISION = 1_000_000_000n;

export type Candle = {
  /** Unix seconds. */
  readonly time: number;
  readonly value: number;
};

export type HistoryConfig = {
  readonly hermesUrl: string;
  readonly token?: string | undefined;
};

function normalise(mantissa: bigint, exponent: number): bigint {
  if (exponent <= 0) return (mantissa * PRICE_PRECISION) / 10n ** BigInt(-exponent);
  return mantissa * PRICE_PRECISION * 10n ** BigInt(exponent);
}

/** A price at PRICE_PRECISION as a float, for charting only — never for sizing. */
export function toDisplay(price: bigint): number {
  return Number(price) / Number(PRICE_PRECISION);
}

async function priceAt(
  cfg: HistoryConfig,
  feedIdHex: string,
  at: number,
): Promise<Candle | undefined> {
  const url = `${cfg.hermesUrl.replace(/\/$/, "")}/v2/updates/price/${at}?ids[]=${feedIdHex}&parsed=true`;
  const res = await fetch(url, {
    headers: cfg.token ? { Authorization: `Bearer ${cfg.token}` } : {},
  });
  if (!res.ok) return undefined;
  const json = (await res.json()) as {
    parsed?: { price: { price: string; expo: number; publish_time: number } }[];
  };
  const p = json.parsed?.[0]?.price;
  if (!p) return undefined;
  return {
    time: p.publish_time,
    value: toDisplay(normalise(BigInt(p.price), p.expo)),
  };
}

/**
 * Fetch `points` samples ending now, spaced `stepSeconds` apart.
 *
 * Requests are issued in small batches rather than all at once: Hermes rate-limits, and a
 * burst of sixty gets a 429 that looks like a broken chart. Points that fail are dropped
 * rather than retried — a chart with a gap is better than a chart that never appears.
 */
export async function fetchHistory(
  cfg: HistoryConfig,
  feedIdHex: string,
  points = 40,
  stepSeconds = 60,
  batch = 5,
): Promise<Candle[]> {
  const now = Math.floor(Date.now() / 1000);
  const stamps = Array.from({ length: points }, (_, i) => now - (points - 1 - i) * stepSeconds);

  const out: Candle[] = [];
  for (let i = 0; i < stamps.length; i += batch) {
    const slice = stamps.slice(i, i + batch);
    const got = await Promise.all(slice.map((t) => priceAt(cfg, feedIdHex, t)));
    for (const c of got) if (c) out.push(c);
  }

  // Hermes returns the first update at or after the requested time, so two adjacent requests
  // can land on the same publish. Charting libraries reject duplicate or unordered stamps.
  const seen = new Set<number>();
  return out
    .sort((a, b) => a.time - b.time)
    .filter((c) => (seen.has(c.time) ? false : (seen.add(c.time), true)));
}

export type Bar = {
  readonly time: number;
  readonly open: number;
  readonly high: number;
  readonly low: number;
  readonly close: number;
};

/**
 * OHLC bars, built by sampling faster than the bar interval and bucketing.
 *
 * Hermes returns a single price per timestamp, not candles — the Benchmarks TradingView shim
 * that did serve candles now 404s. So a bar is assembled from `samplesPerBar` prices inside
 * its window: first is the open, last the close, and the extremes are the high and low. With
 * three samples a bar the wicks are real but coarse; they are the true extremes *of what was
 * sampled*, not of every tick Pyth published.
 *
 * Cost is one request per sample, so bars are not free: 20 bars at 3 samples is 60 requests.
 */
export async function fetchBars(
  cfg: HistoryConfig,
  feedIdHex: string,
  bars = 24,
  barSeconds = 60,
  samplesPerBar = 3,
  batch = 6,
): Promise<Bar[]> {
  const step = Math.max(1, Math.floor(barSeconds / samplesPerBar));
  const points = bars * samplesPerBar;
  const samples = await fetchHistory(cfg, feedIdHex, points, step, batch);
  if (samples.length === 0) return [];

  // Bucket by wall-clock window so bars line up on the interval rather than on sample index,
  // which matters because Hermes may return the same publish for adjacent requests.
  const buckets = new Map<number, number[]>();
  for (const s of samples) {
    const key = Math.floor(s.time / barSeconds) * barSeconds;
    const arr = buckets.get(key);
    if (arr) arr.push(s.value);
    else buckets.set(key, [s.value]);
  }

  return [...buckets.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([time, vs]) => ({
      time,
      open: vs[0] as number,
      high: Math.max(...vs),
      low: Math.min(...vs),
      close: vs[vs.length - 1] as number,
    }));
}
