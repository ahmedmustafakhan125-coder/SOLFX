/**
 * The trader's desk: the arithmetic behind the NOXFUNDS ticket, chart and record, kept out of
 * the page so every rule it mirrors can be unit-tested.
 *
 * # Mirrors, not opinions
 *
 * Each function here reproduces one measurement the program makes, in the program's units and
 * with the program's rounding, so the page can name a refusal before a transaction is signed.
 * None of them replaces the program's check: the price moves between this and the fill.
 *
 * | Here | On chain |
 * |---|---|
 * | `riskAtStop` | `pnl::notional_in_collateral(size, |spot − stop|)` — `notional_in_quote` ceils |
 * | `riskBps` | `mul_div_ceil(risk, BPS, balance)` in `eval_open_position` |
 * | `stopFromBps` | the page's own conversion; rounded adverse to the trader on both sides |
 * | `estimatePnl` | `pnl::upnl_in_quote` — floors toward −∞, so gains shrink and losses grow |
 * | `costToUsd` | `pnl::convert_cost_to_collateral` — a cost converts rounded up |
 * | `pnlToUsd` | `pnl::convert_pnl_to_collateral` — a P&L converts rounded toward −∞ |
 * | `composeSynthetic` | `oracle::compose_synthetic` — the mid, floored |
 *
 * Every figure is an integer. Prices at PRICE_PRECISION 1e9, sizes at BASE_PRECISION 1e9,
 * money at QUOTE_PRECISION 1e6, related by `notional = size × price / 1e12`.
 */
import { Direction, nox } from "@solfx/client";
import type { Address } from "@solana/kit";

import type { ChartLevel } from "@/components/PriceChart";
import { legFeeds, type LoadedMarket } from "@/lib/markets";
import type { Marketplace, Row } from "@/lib/nox";
import type { LivePrice } from "@/lib/prices";

export const NOTIONAL_DIVISOR = 1_000_000_000_000n;
const BPS = 10_000n;
const PRICE_PRECISION = 1_000_000_000n;

// --- conversion, for markets not quoted in USD ---------------------------------------------

/**
 * How a market's quote currency becomes USDC. `rate` is the conversion feed's price at
 * PRICE_PRECISION: quote units per USD for USD/JPY-style feeds, USD per quote unit for EUR/USD.
 */
export type Conversion =
  | { readonly kind: "none" }
  | { readonly kind: "quotePerUsd" | "usdPerQuote"; readonly rate: bigint };

export const NO_CONVERSION: Conversion = { kind: "none" };

/** A cost (notional, risk) in quote units → USDC, **rounded up** as the program rounds costs. */
export function costToUsd(
  quote: bigint,
  c: Conversion = NO_CONVERSION
): bigint {
  if (c.kind === "none") return quote;
  if (c.rate <= 0n) return 0n;
  return c.kind === "quotePerUsd"
    ? ceilDiv(quote * PRICE_PRECISION, c.rate)
    : ceilDiv(quote * c.rate, PRICE_PRECISION);
}

/** A P&L in quote units → USDC, **floored toward −∞** as the program converts a P&L. */
export function pnlToUsd(quote: bigint, c: Conversion = NO_CONVERSION): bigint {
  if (c.kind === "none") return quote;
  if (c.rate <= 0n) return 0n;
  return c.kind === "quotePerUsd"
    ? floorDiv(quote * PRICE_PRECISION, c.rate)
    : floorDiv(quote * c.rate, PRICE_PRECISION);
}

/** USDC → quote units, floored, for sizing: never more than the dollars asked for. */
export function usdToQuote(usd: bigint, c: Conversion = NO_CONVERSION): bigint {
  if (c.kind === "none") return usd;
  if (c.rate <= 0n) return 0n;
  return c.kind === "quotePerUsd"
    ? (usd * c.rate) / PRICE_PRECISION
    : (usd * PRICE_PRECISION) / c.rate;
}

/** A synthetic pair's mid from its two legs, floored — `compose_synthetic`. */
export function composeSynthetic(
  base: bigint,
  quote: bigint,
  invertQuote: boolean
): bigint {
  if (base <= 0n || quote <= 0n) return 0n;
  return invertQuote
    ? (base * PRICE_PRECISION) / quote
    : (base * quote) / PRICE_PRECISION;
}

/** Everything a ticket needs about one market: its price, its conversion, its price accounts. */
export type MarketQuote = {
  /** The market's price at PRICE_PRECISION — composed, for a synthetic. */
  readonly price: bigint;
  /** Any leg stale makes the whole price stale: the program refuses on any one of them. */
  readonly stale: boolean;
  readonly ageSeconds: number;
  readonly conversion: Conversion;
  readonly legs: {
    readonly priceUpdate: Address;
    readonly secondaryPriceUpdate?: Address;
    readonly quoteConversionPriceUpdate?: Address;
  };
  /** The price as the chart takes it. */
  readonly live: LivePrice;
};

/**
 * Price a market from the legs its configuration names. `undefined` when any leg has no price
 * or no account — a market priced from a missing leg is a market the program would refuse.
 */
export function marketQuote(
  m: LoadedMarket,
  prices: Readonly<Record<string, LivePrice | undefined>>,
  accounts: Readonly<Record<string, Address | undefined>>
): MarketQuote | undefined {
  const feeds = legFeeds(m.data);
  const primary = prices[feeds.primary];
  const primaryAccount = accounts[feeds.primary];
  if (!primary || !primaryAccount) return undefined;
  const used: LivePrice[] = [primary];
  let price = primary.price;
  let secondaryPriceUpdate: Address | undefined;
  if (feeds.secondary) {
    const sec = prices[feeds.secondary];
    secondaryPriceUpdate = accounts[feeds.secondary];
    if (
      !sec ||
      !secondaryPriceUpdate ||
      m.data.priceSource.__kind !== "Synthetic"
    )
      return undefined;
    price = composeSynthetic(
      primary.price,
      sec.price,
      m.data.priceSource.invertQuote
    );
    used.push(sec);
  }
  let conversion: Conversion = NO_CONVERSION;
  let quoteConversionPriceUpdate: Address | undefined;
  if (feeds.conversion) {
    const conv = prices[feeds.conversion];
    quoteConversionPriceUpdate = accounts[feeds.conversion];
    if (!conv || !quoteConversionPriceUpdate) return undefined;
    // `QuoteConversionKind`: 1 is quote-per-USD, 2 is USD-per-quote.
    conversion = {
      kind:
        Number(m.data.quoteConversionKind) === 1
          ? "quotePerUsd"
          : "usdPerQuote",
      rate: conv.price,
    };
    used.push(conv);
  }
  const stale = used.some((p) => p.stale);
  const ageSeconds = Math.max(...used.map((p) => p.ageSeconds));
  return {
    price,
    stale,
    ageSeconds,
    conversion,
    legs: {
      priceUpdate: primaryAccount,
      ...(secondaryPriceUpdate ? { secondaryPriceUpdate } : {}),
      ...(quoteConversionPriceUpdate ? { quoteConversionPriceUpdate } : {}),
    },
    live: { ...primary, price, stale, ageSeconds },
  };
}

/** Every market's quote, by market index. */
export function quotesFor(
  markets: readonly LoadedMarket[],
  prices: Readonly<Record<string, LivePrice | undefined>>,
  accounts: Readonly<Record<string, Address | undefined>>
): Record<number, MarketQuote | undefined> {
  const out: Record<number, MarketQuote | undefined> = {};
  for (const m of markets) out[m.index] = marketQuote(m, prices, accounts);
  return out;
}

type Dir = nox.DirectionArgs | Direction;

function isLong(direction: Dir): boolean {
  return Number(direction) === Number(Direction.Long);
}

function ceilDiv(n: bigint, d: bigint): bigint {
  return d === 0n ? 0n : (n + d - 1n) / d;
}

/** Floor division toward −∞, which BigInt's `/` (truncation toward zero) is not for negatives. */
function floorDiv(n: bigint, d: bigint): bigint {
  const q = n / d;
  return n % d !== 0n && n < 0n !== d < 0n ? q - 1n : q;
}

// --- the stop --------------------------------------------------------------------------------

/**
 * A stop `bps` away from `price`, on the losing side.
 *
 * Rounded adverse to the trader on both sides: a long's stop floors (a little lower, a little
 * more risk measured), a short's ceils (a little higher). The risk rule reads the distance, so
 * rounding the other way would let a ticket slip under the limit by a unit.
 */
export function stopFromBps(
  direction: Dir,
  price: bigint,
  bps: number
): bigint {
  if (price <= 0n || !Number.isInteger(bps) || bps <= 0 || bps >= 10_000)
    return 0n;
  return isLong(direction)
    ? (price * BigInt(10_000 - bps)) / BPS
    : ceilDiv(price * BigInt(10_000 + bps), BPS);
}

/** How far a stop sits from `price`, in bps, rounded up — the same way a limit is measured. */
export function bpsFromStop(price: bigint, stop: bigint): number {
  if (price <= 0n || stop <= 0n) return 0;
  const distance = price > stop ? price - stop : stop - price;
  return Number(ceilDiv(distance * BPS, price));
}

/** Whether a stop sits on the losing side of `price` — `StopOnWrongSide` otherwise. */
export function stopOnLosingSide(
  direction: Dir,
  price: bigint,
  stop: bigint
): boolean {
  if (price <= 0n || stop <= 0n) return false;
  return isLong(direction) ? stop < price : stop > price;
}

/**
 * Money lost if the stop fills exactly at its price, in USDC at 1e6.
 *
 * `notional_in_collateral(size, |price − stop|)`: `notional_in_quote` ceils, and converting a
 * cost to USDC ceils again for a market not quoted in USD.
 */
export function riskAtStop(
  sizeBase: bigint,
  price: bigint,
  stop: bigint,
  conversion: Conversion = NO_CONVERSION
): bigint {
  if (sizeBase <= 0n || price <= 0n || stop <= 0n) return 0n;
  const distance = price > stop ? price - stop : stop - price;
  return costToUsd(ceilDiv(sizeBase * distance, NOTIONAL_DIVISOR), conversion);
}

/** Risk as bps of `balance`, rounded up. `null` when there is no balance to measure against. */
export function riskBps(risk: bigint, balance: bigint): bigint | null {
  if (balance <= 0n) return null;
  return ceilDiv(risk * BPS, balance);
}

/**
 * A decimal price typed by a person, at PRICE_PRECISION 1e9. `null` for anything that is not a
 * plain positive decimal with at most nine places — a price is never guessed at.
 */
export function parsePrice(input: string): bigint | null {
  const t = input.trim().replace(/,/g, "");
  if (!/^\d+(\.\d{1,9})?$/.test(t)) return null;
  const [whole = "0", frac = ""] = t.split(".");
  const v = BigInt(whole) * 1_000_000_000n + BigInt(frac.padEnd(9, "0"));
  return v > 0n ? v : null;
}

/** What the evaluation ticket needs to judge an open, in the program's units. */
export type EvalTicket = {
  readonly direction: Dir;
  /** Oracle price at PRICE_PRECISION; 0 when there is none yet. */
  readonly price: bigint;
  readonly stale: boolean;
  readonly sizeBase: bigint;
  readonly notional: bigint;
  readonly stop: bigint;
  /** The evaluation's realised balance, floored at zero as `floor_equity` floors it. */
  readonly balance: bigint;
  readonly maxLeverage: number;
  readonly openPositions: number;
  readonly maxOpen: number;
  readonly maxRiskBps: number;
  /** How the market's quote currency becomes USDC; none for a USD-quoted market. */
  readonly conversion?: Conversion;
};

/**
 * The first reason `eval_open_position` would refuse this ticket, or `null`.
 *
 * In the order the program checks, so the sentence names the refusal that would come back. The
 * price can move between this and the fill; the program's answer is the one that counts.
 */
export function evalTicketRefusal(t: EvalTicket): string | null {
  if (t.price <= 0n) return "waiting for a price";
  if (t.stale) return "the price is stale and the program refuses it";
  if (t.openPositions >= t.maxOpen)
    return `${t.maxOpen} positions are already open`;
  if (t.sizeBase <= 0n || t.notional <= 0n) return "enter a notional";
  if (t.stop <= 0n) return "a stop is mandatory";
  if (!stopOnLosingSide(t.direction, t.price, t.stop))
    return `the stop must be ${isLong(t.direction) ? "below" : "above"} the price`;
  if (t.balance <= 0n) return "the balance is exhausted";
  const leverage = ceilDiv(t.notional, t.balance);
  if (leverage > BigInt(t.maxLeverage))
    return `${leverage}x is above this market's ${t.maxLeverage}x`;
  const r = riskBps(
    riskAtStop(t.sizeBase, t.price, t.stop, t.conversion),
    t.balance
  );
  if (r === null || r > BigInt(t.maxRiskBps))
    return `risk at the stop is ${r === null ? "unmeasurable" : `${Number(r) / 100}%`}, above the ${t.maxRiskBps / 100}% limit`;
  return null;
}

/** Size, in base units, for a USDC notional at `price`. Floors: never more than asked for. */
export function sizeFor(
  notionalUsd: bigint,
  price: bigint,
  conversion: Conversion = NO_CONVERSION
): bigint {
  if (price <= 0n) return 0n;
  return (usdToQuote(notionalUsd, conversion) * NOTIONAL_DIVISOR) / price;
}

// --- marking to the oracle -----------------------------------------------------------------

/**
 * Unrealised P&L at `price`, in USDC — `upnl_in_quote`, floored toward −∞.
 *
 * At the oracle, so it is **before** the closing spread, the close fee and carry. The program
 * prices the close at the execution price, which is always a little worse; the page says so
 * wherever it shows this.
 */
export function estimatePnl(
  direction: Dir,
  sizeBase: bigint,
  entryPrice: bigint,
  price: bigint,
  conversion: Conversion = NO_CONVERSION
): bigint {
  if (sizeBase <= 0n || entryPrice <= 0n || price <= 0n) return 0n;
  const move = isLong(direction) ? price - entryPrice : entryPrice - price;
  return pnlToUsd(floorDiv(sizeBase * move, NOTIONAL_DIVISOR), conversion);
}

export type EvalEstimate = {
  /** Balance plus every open position marked to its oracle. */
  readonly equity: bigint;
  /** Per position, keyed by its address. */
  readonly pnl: ReadonlyMap<Address, bigint>;
};

/**
 * An evaluation's equity, marked to the live oracle. `null` if any open position cannot be
 * priced — a partial sum would show a loss as smaller than it is, which is the wrong way round.
 */
export function estimateEvaluation(
  evaluation: nox.Evaluation,
  positions: readonly Row<nox.VirtualPosition>[],
  priceByIndex: Readonly<Record<number, bigint | undefined>>,
  conversionByIndex: Readonly<Record<number, Conversion | undefined>> = {}
): EvalEstimate | null {
  let equity = evaluation.balance;
  const pnl = new Map<Address, bigint>();
  for (const p of positions) {
    const px = priceByIndex[p.data.marketIndex];
    if (px === undefined || px <= 0n) return null;
    const u = estimatePnl(
      p.data.direction,
      p.data.sizeBase,
      p.data.entryPrice,
      px,
      conversionByIndex[p.data.marketIndex]
    );
    pnl.set(p.address, u);
    equity += u;
  }
  return { equity: equity > 0n ? equity : 0n, pnl };
}

// --- verification --------------------------------------------------------------------------

/**
 * Whether `trader` has passed both phases of an evaluation.
 *
 * Read from the `Evaluation` accounts themselves: only `noxfunds` can create one at its PDA, and
 * only `claim_stage_pass` can set `state = Passed`, after checking every rule of Phase 2. So the
 * mark is the program's statement, not ours. Matched on the account's own `trader` field — an
 * evaluation is only ever created with that field set from its signer.
 */
export function isVerified(m: Marketplace, trader: Address): boolean {
  return passedEvaluations(m, trader) > 0;
}

/**
 * How many evaluations `trader` has passed.
 *
 * Two sources, both written only by the program: the `Evaluation` accounts in state `Passed`,
 * and — from the upgrade that added it — the profile's own `evaluationsPassed` counter, which
 * `claim_stage_pass` increments on the Phase 2 pass. The larger is taken, so a pass recorded in
 * both is counted once, and one recorded in either is counted.
 */
export function passedEvaluations(m: Marketplace, trader: Address): number {
  let n = 0;
  for (const e of m.evaluations)
    if (e.data.trader === trader && e.data.state === 1) n += 1;
  const onProfile = m.profiles.get(trader)?.data.evaluationsPassed ?? 0;
  return Math.max(n, onProfile);
}

// --- the chart -----------------------------------------------------------------------------

/** One open position on the chart: where it came in, and where it gets out. */
export type DeskPosition = {
  readonly marketIndex: number;
  readonly direction: Dir;
  readonly entryPrice: bigint;
  readonly stopPrice?: bigint | undefined;
  readonly takeProfitPrice?: bigint | undefined;
  /** "Eval" or "Funded", so two books on one chart can be told apart. */
  readonly book: string;
};

/** The lines to draw for the positions on `marketIndex`. Zero or absent prices draw nothing. */
export function chartLevels(
  marketIndex: number,
  positions: readonly DeskPosition[]
): ChartLevel[] {
  const out: ChartLevel[] = [];
  for (const p of positions) {
    if (p.marketIndex !== marketIndex) continue;
    const long = isLong(p.direction);
    if (p.entryPrice > 0n)
      out.push({
        price: p.entryPrice,
        label: `${p.book} ${long ? "long" : "short"}`,
        kind: long ? "long" : "short",
      });
    if (p.stopPrice !== undefined && p.stopPrice > 0n)
      out.push({
        price: p.stopPrice,
        label: `${p.book} stop`,
        kind: "stop-loss",
      });
    if (p.takeProfitPrice !== undefined && p.takeProfitPrice > 0n)
      out.push({
        price: p.takeProfitPrice,
        label: `${p.book} target`,
        kind: "take-profit",
      });
  }
  return out;
}
