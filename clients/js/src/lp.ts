/**
 * Liquidity-pool share arithmetic the pool page needs to *preview*, ported from
 * `crates/solfx-math/src/lp.rs` and `programs/solfx-core/src/instructions/lp.rs`.
 *
 * # Why this is a port and not a reimplementation
 *
 * An LP decides to withdraw on the number the page shows them. If that number is optimistic
 * by even one unit, `remove_liquidity` returns `SlippageExceeded` against the `min_usdc_out`
 * the page computed from it — a rejection the LP cannot explain, on a transaction that looked
 * fine. So every function here mirrors a named Rust function, keeps its rounding direction,
 * and is checked in the tests against values copied from the Rust tests rather than
 * recomputed.
 *
 * Everything is `bigint`. No floats, for the same reason `solfx-math` has none: a `number`
 * cannot hold a u64 exactly, and this is money.
 *
 * # The rounding table, which is the whole design
 *
 * | Quantity | Direction | Who it favours |
 * |---|---|---|
 * | Shares minted for a deposit | **down** | the existing pool |
 * | USDC paid for a redemption | **down** | the remaining pool |
 * | Exit fee | **up** | the remaining pool |
 * | Performance fee | **up** | the treasury |
 *
 * Every one rounds against the party initiating the action.
 */

/** USDC and `slpUSD` both have 6 decimals, so one unit of each is 1e-6. */
const QUOTE_PRECISION = 1_000_000n;
const BPS_PRECISION = 10_000n;

/** The u64 ceiling. `bigint` has none, the chain does, and the port must agree. */
const U64_MAX = 18_446_744_073_709_551_615n;

/**
 * Mirrors `fixed::to_u64`, which **errors** rather than wrapping.
 *
 * Without this the port would happily display a number the chain cannot represent — the one
 * failure mode a `bigint` port has that the Rust does not.
 */
function toU64(v: bigint): bigint {
  if (v < 0n || v > U64_MAX) {
    throw new RangeError(`value does not fit in u64: ${v}`);
  }
  return v;
}

/** `a * b / d` rounded **down**. Mirrors `fixed::mul_div_floor`. */
function mulDivFloor(a: bigint, b: bigint, d: bigint): bigint {
  return (a * b) / d;
}

/** `a * b / d` rounded **up**. Mirrors `fixed::mul_div_ceil`; used for anything charged. */
function mulDivCeil(a: bigint, b: bigint, d: bigint): bigint {
  const n = a * b;
  return n / d + (n % d === 0n ? 0n : 1n);
}

/**
 * NAV per share at `QUOTE_PRECISION`, i.e. `1_000_000n` == $1.00 per share.
 *
 * An empty pool is worth par by definition — the first depositor sets the price. Mirrors
 * `lp::nav_per_share`.
 */
export function navPerShare(aum: bigint, supply: bigint): bigint {
  if (supply === 0n) return QUOTE_PRECISION;
  return toU64(mulDivFloor(aum, QUOTE_PRECISION, supply));
}

/**
 * Shares minted for a USDC deposit, rounded **down**. Mirrors `lp::shares_for_deposit`.
 *
 * An empty pool mints 1:1 rather than at the last NAV. A pool can reach `supply == 0` with
 * `aum > 0` when every LP exits while it holds retained fees; minting 1:1 hands those to the
 * depositor as a higher starting NAV instead of letting the last exiter time the giveaway.
 */
export function sharesForDeposit(
  amount: bigint,
  aum: bigint,
  supply: bigint,
): bigint {
  if (amount === 0n) return 0n;
  if (supply === 0n || aum === 0n) return amount;
  return toU64(mulDivFloor(amount, supply, aum));
}

/** USDC returned for burning shares, rounded **down**. Mirrors `lp::usdc_for_shares`. */
export function usdcForShares(
  shares: bigint,
  aum: bigint,
  supply: bigint,
): bigint {
  if (shares === 0n || supply === 0n) return 0n;
  if (shares > supply) {
    throw new Error("usdcForShares: shares exceed supply");
  }
  return toU64(mulDivFloor(shares, aum, supply));
}

/** The exit fee, rounded **up**. Mirrors `lp::exit_fee`. Stays in the pool, not the treasury. */
export function exitFee(gross: bigint, feeBps: number): bigint {
  if (gross === 0n || feeBps === 0) return 0n;
  return toU64(mulDivCeil(gross, BigInt(feeBps), BPS_PRECISION));
}

/**
 * The performance fee on a redemption, rounded **up**. Mirrors `lp::performance_fee`.
 *
 * Charged only on the gain above the pool's high-water mark, and nothing at all below it.
 * One mark for the whole pool, so an LP who joins after a drawdown pays nothing until the
 * pool recovers its previous peak.
 */
export function performanceFee(
  shares: bigint,
  navNowPerShare: bigint,
  highWaterMarkPerShare: bigint,
  feeBps: number,
): bigint {
  if (shares === 0n || feeBps === 0 || navNowPerShare <= highWaterMarkPerShare) {
    return 0n;
  }
  const gainPerShare = navNowPerShare - highWaterMarkPerShare;
  const gain = mulDivFloor(shares, gainPerShare, QUOTE_PRECISION);
  return toU64(mulDivCeil(gain, BigInt(feeBps), BPS_PRECISION));
}

/** The pool state a preview reads. Field names match `LpPool` so a caller can spread it. */
export type PoolState = {
  readonly aum: bigint;
  readonly lpTokenSupply: bigint;
  readonly exitFeeBps: number;
  readonly performanceFeeBps: number;
  readonly highWaterMarkPerShare: bigint;
};

export type DepositPreview = {
  /** Shares the deposit mints. */
  readonly shares: bigint;
  /** NAV per share the deposit buys at, at `QUOTE_PRECISION`. */
  readonly navPerShare: bigint;
};

/** What `add_liquidity` will mint for `amount`, before slippage. */
export function previewDeposit(
  amount: bigint,
  pool: Pick<PoolState, "aum" | "lpTokenSupply">,
): DepositPreview {
  return {
    shares: sharesForDeposit(amount, pool.aum, pool.lpTokenSupply),
    navPerShare: navPerShare(pool.aum, pool.lpTokenSupply),
  };
}

export type WithdrawalPreview = {
  /** Value of the shares before any fee. */
  readonly gross: bigint;
  readonly performanceFee: bigint;
  readonly exitFee: bigint;
  /** What actually reaches the LP's token account. */
  readonly payout: bigint;
  /** True when this redemption empties the pool, which waives the exit fee. */
  readonly drainsThePool: boolean;
};

/**
 * What `remove_liquidity` will pay for `shares`.
 *
 * Composed in the same order as the instruction, which matters: the performance fee is
 * charged on NAV **before** the redemption, and the exit fee is waived when the redemption
 * empties the pool — there is nobody left to compensate, and leaving the fee behind would
 * strand USDC against zero shares (invariant I8).
 */
export function previewWithdrawal(
  shares: bigint,
  pool: PoolState,
): WithdrawalPreview {
  const gross = usdcForShares(shares, pool.aum, pool.lpTokenSupply);
  const navBefore = navPerShare(pool.aum, pool.lpTokenSupply);
  const perf = performanceFee(
    shares,
    navBefore,
    pool.highWaterMarkPerShare,
    pool.performanceFeeBps,
  );
  const drainsThePool = shares === pool.lpTokenSupply && shares > 0n;
  const exit = drainsThePool ? 0n : exitFee(gross, pool.exitFeeBps);
  return {
    gross,
    performanceFee: perf,
    exitFee: exit,
    payout: gross - perf - exit,
    drainsThePool,
  };
}

/**
 * Seconds still to wait before `remove_liquidity` will settle, or `0` when it is ready.
 *
 * `unlockAt` and `now` are both unix seconds. Returned rather than formatted because a
 * countdown is a presentation decision and this module makes none.
 */
export function secondsUntilUnlock(unlockAt: bigint, now: bigint): bigint {
  return unlockAt > now ? unlockAt - now : 0n;
}
