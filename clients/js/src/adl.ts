/**
 * Auto-deleveraging, from the trader's side.
 *
 * # Why this is in the client at all
 *
 * ADL takes money from traders who did nothing wrong. When a position gaps through its
 * liquidation price the pool is owed more than the collateral covers, and the waterfall is:
 * insurance fund first, then **the most profitable opposing positions are force-closed and
 * part of their profit withheld**, then LP NAV absorbs the rest.
 *
 * `ARCHITECTURE.md` § 6.9 requires telling traders that in plain language *before* they open
 * a position — every venue that hid it and then used it was destroyed on social media. A
 * disclosure is necessary but not sufficient: a trader should also be able to see where they
 * stand. That is what this module computes.
 *
 * # What it cannot do
 *
 * There is no opting out, and no fee that buys immunity. The only lever a trader has is the
 * ranking itself: ADL takes the most profitable positions first, measured as unrealised P&L
 * against collateral, so taking profit lowers your exposure to it. Anything that implied more
 * control than that would be a lie.
 */

const BPS_PRECISION = 10_000n;

/**
 * A position's place in the ADL queue: unrealised P&L as a proportion of collateral, in bps.
 *
 * § 6.9 ranks candidates by `uPnL % of collateral` **descending** — the most profitable
 * first. Higher is closer to the front of the queue.
 *
 * Zero for a position with no collateral rather than a division by zero: a position cannot
 * exist without collateral (invariant I5), so this is a defensive branch, not a case.
 */
export function adlRankBps(unrealised: bigint, collateral: bigint): bigint {
  if (collateral <= 0n) return 0n;
  return (unrealised * BPS_PRECISION) / collateral;
}

/**
 * Whether a position could be deleveraged at all.
 *
 * The program enforces exactly one condition beyond the shortfall existing: `health.upnl > 0`
 * (`NotAdlEligible`). **A position that is not in profit cannot be deleveraged**, so a losing
 * or flat position is never a candidate however the queue is ordered.
 */
export function isAdlEligible(unrealised: bigint): boolean {
  return unrealised > 0n;
}

/** How much of the waterfall is left before ADL can happen at all. */
export type AdlExposure = {
  /** Shortfall already recorded and awaiting deleveraging, across the markets given. */
  readonly pendingDebt: bigint;
  /** The insurance fund, which absorbs a shortfall before any trader does. */
  readonly insuranceBalance: bigint;
  /** True when a shortfall is outstanding right now — ADL is not hypothetical. */
  readonly active: boolean;
};

export function adlExposure(input: {
  pendingDebt: bigint;
  insuranceBalance: bigint;
}): AdlExposure {
  return {
    pendingDebt: input.pendingDebt,
    insuranceBalance: input.insuranceBalance,
    active: input.pendingDebt > 0n,
  };
}

/**
 * What a position would keep if it were deleveraged now.
 *
 * ADL withholds **up to the whole profit, never more, and never any principal** — the trader
 * gets their collateral back in full plus whatever profit survives. So the worst case is
 * being returned to flat, and the figure worth showing is the profit at risk, not the
 * position's value.
 *
 * `shortfall` is what remains to be covered; the withholding stops there, and the remainder
 * carries to the next position in the queue.
 */
export function adlWithholding(
  unrealised: bigint,
  shortfall: bigint,
): { readonly withheld: bigint; readonly profitKept: bigint } {
  if (unrealised <= 0n) return { withheld: 0n, profitKept: 0n };
  const withheld = shortfall < unrealised ? shortfall : unrealised;
  return { withheld, profitKept: unrealised - withheld };
}
