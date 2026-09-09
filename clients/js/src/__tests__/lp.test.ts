import { describe, expect, it } from "vitest";
import {
  exitFee,
  navPerShare,
  performanceFee,
  previewDeposit,
  previewWithdrawal,
  secondsUntilUnlock,
  sharesForDeposit,
  usdcForShares,
  type PoolState,
} from "../lp.js";

/** $1.00 per share, at `QUOTE_PRECISION`. Named as `lp.rs` names it. */
const PAR = 1_000_000n;
const U64_MAX = 18_446_744_073_709_551_615n;

/**
 * Parity with `crates/solfx-math/src/lp.rs`.
 *
 * Every expected value is the assertion from the Rust test of the same name, copied rather
 * than recomputed — the same discipline `margin.test.ts` applies to liquidation prices, and
 * for the same reason. A rounding direction that drifts here shows up as an LP being quoted
 * a payout the chain then refuses on `SlippageExceeded`.
 */
describe("lp share arithmetic matches solfx-math", () => {
  // lp.rs: an_empty_pool_is_worth_par
  it("prices an empty pool at par", () => {
    expect(navPerShare(0n, 0n)).toBe(PAR);
  });

  // lp.rs: the_first_deposit_mints_one_to_one
  it("mints the first deposit one to one", () => {
    expect(sharesForDeposit(1_000_000n, 0n, 0n)).toBe(1_000_000n);
  });

  // lp.rs: a_deposit_into_a_profitable_pool_buys_fewer_shares
  it("buys fewer shares in a profitable pool", () => {
    const aum = 1_000_000_000n;
    const supply = 500_000_000n;
    expect(navPerShare(aum, supply)).toBe(2_000_000n); // $2.00
    expect(sharesForDeposit(100_000_000n, aum, supply)).toBe(50_000_000n);
  });

  // lp.rs: redemption_returns_the_share_of_aum
  it("returns the share of AUM on redemption", () => {
    expect(usdcForShares(50_000_000n, 1_000_000_000n, 500_000_000n)).toBe(
      100_000_000n,
    );
  });

  // lp.rs: redeeming_the_whole_supply_returns_the_whole_pool
  it("returns the whole pool when the whole supply is redeemed", () => {
    const aum = 1_234_567_890n;
    const supply = 999_999_999n;
    expect(usdcForShares(supply, aum, supply)).toBe(aum);
  });

  // lp.rs: redeeming_more_than_exists_is_rejected
  it("rejects redeeming more than exists", () => {
    expect(() => usdcForShares(101n, 1_000n, 100n)).toThrow();
  });

  // lp.rs: the_exit_fee_rounds_up
  it("rounds the exit fee up", () => {
    expect(exitFee(100_010_000n, 5)).toBe(50_005n);
    // A fee that would floor to zero still charges one unit.
    expect(exitFee(1n, 5)).toBe(1n);
    expect(exitFee(0n, 5)).toBe(0n);
    expect(exitFee(1_000_000n, 0)).toBe(0n);
  });

  // lp.rs: no_performance_fee_below_the_high_water_mark
  it("charges no performance fee below the high-water mark", () => {
    expect(performanceFee(1_000_000_000n, 900_000n, PAR, 1_000)).toBe(0n);
    // Exactly at the mark is not a gain.
    expect(performanceFee(1_000_000_000n, PAR, PAR, 1_000)).toBe(0n);
  });

  // lp.rs: the_performance_fee_is_ten_percent_of_the_gain_above_the_mark
  it("takes ten percent of the gain above the mark", () => {
    expect(performanceFee(1_000_000_000n, 1_200_000n, PAR, 1_000)).toBe(
      20_000_000n,
    );
  });

  // lp.rs: an_lp_who_joined_after_a_drawdown_pays_nothing_until_the_peak_returns
  it("charges a post-drawdown joiner nothing until the peak returns", () => {
    const hwm = 2_000_000n; // the pool peaked at $2.00
    expect(performanceFee(1_000_000_000n, 1_500_000n, hwm, 1_000)).toBe(0n);
  });

  // lp.rs: degenerate_inputs_do_not_panic
  it("handles degenerate inputs the way the engine does", () => {
    expect(sharesForDeposit(0n, 100n, 100n)).toBe(0n);
    expect(usdcForShares(0n, 100n, 100n)).toBe(0n);
    expect(usdcForShares(10n, 100n, 0n)).toBe(0n);
    // `u64::MAX x 1e6` for a single share does not fit in a u64. The Rust errors; so must
    // the port, or it would display a number the chain cannot represent.
    expect(() => navPerShare(U64_MAX, 1n)).toThrow(RangeError);
    expect(navPerShare(U64_MAX, 1_000_000n)).toBe(U64_MAX);
    expect(() => sharesForDeposit(U64_MAX, 1n, U64_MAX)).toThrow(RangeError);
  });
});

/**
 * The properties `lp.rs` states as its reason for existing. Asserted at values chosen to
 * force rounding, exactly as the Rust does — a pool at round numbers cannot fail these.
 */
describe("the anti-dilution properties hold", () => {
  const AWKWARD_AUM = 1_000_000_007n;
  const AWKWARD_SUPPLY = 333_333_331n;

  // lp.rs: a_deposit_never_raises_nav_per_share_for_the_depositor
  it("never lets a deposit dilute the existing pool", () => {
    const before = navPerShare(AWKWARD_AUM, AWKWARD_SUPPLY);
    const deposit = 7_777_777n;
    const shares = sharesForDeposit(deposit, AWKWARD_AUM, AWKWARD_SUPPLY);
    const after = navPerShare(AWKWARD_AUM + deposit, AWKWARD_SUPPLY + shares);
    expect(after).toBeGreaterThanOrEqual(before);
  });

  // lp.rs: a_withdrawal_never_lowers_nav_per_share_for_those_who_stay
  it("never lets a withdrawal dilute those who stay", () => {
    const before = navPerShare(AWKWARD_AUM, AWKWARD_SUPPLY);
    const shares = 111_111_111n;
    const paid = usdcForShares(shares, AWKWARD_AUM, AWKWARD_SUPPLY);
    const after = navPerShare(AWKWARD_AUM - paid, AWKWARD_SUPPLY - shares);
    expect(after).toBeGreaterThanOrEqual(before);
  });

  /**
   * A round trip must lose. The LP-side statement of
   * `round_trip_at_the_same_price_always_loses` — deposit then immediately redeem every
   * share the deposit minted, at an unmoved NAV, and you get back less than you put in.
   *
   * Not in the Rust unit tests, because there it is a property test over the whole space.
   * Asserted here at the awkward pool so the port cannot claim a profitable round trip.
   */
  it("makes an immediate round trip lose", () => {
    const pool: PoolState = {
      aum: AWKWARD_AUM,
      lpTokenSupply: AWKWARD_SUPPLY,
      exitFeeBps: 5,
      performanceFeeBps: 1_000,
      highWaterMarkPerShare: navPerShare(AWKWARD_AUM, AWKWARD_SUPPLY),
    };
    const deposit = 100_000_000n; // $100
    const { shares } = previewDeposit(deposit, pool);

    const after: PoolState = {
      ...pool,
      aum: pool.aum + deposit,
      lpTokenSupply: pool.lpTokenSupply + shares,
    };
    const { payout } = previewWithdrawal(shares, after);
    expect(payout).toBeLessThan(deposit);
  });
});

/**
 * The composed preview, which is what the page actually calls. Its order has to match
 * `remove_liquidity` — performance fee on NAV *before* the redemption, exit fee waived when
 * the pool is emptied.
 */
describe("previewWithdrawal composes the way remove_liquidity does", () => {
  const pool: PoolState = {
    aum: 1_000_000_000n, // $1,000
    lpTokenSupply: 500_000_000n, // 500 shares -> $2.00 NAV
    exitFeeBps: 5,
    performanceFeeBps: 1_000,
    highWaterMarkPerShare: PAR, // the mark is still $1.00, so all of the gain is taxable
  };

  it("charges both fees and nets them off the gross", () => {
    const p = previewWithdrawal(50_000_000n, pool);
    expect(p.gross).toBe(100_000_000n); // 50 shares x $2.00
    // 50 shares of gain from $1.00 to $2.00 is $50; 10% of that is $5.
    expect(p.performanceFee).toBe(5_000_000n);
    // 5 bps of $100.
    expect(p.exitFee).toBe(50_000n);
    expect(p.payout).toBe(100_000_000n - 5_000_000n - 50_000n);
    expect(p.drainsThePool).toBe(false);
  });

  it("waives the exit fee when the redemption empties the pool", () => {
    const p = previewWithdrawal(pool.lpTokenSupply, pool);
    expect(p.drainsThePool).toBe(true);
    expect(p.exitFee).toBe(0n);
    expect(p.gross).toBe(pool.aum);
    // The performance fee is still charged; only the exit fee is waived.
    expect(p.performanceFee).toBe(50_000_000n);
  });

  it("charges nothing but the exit fee when the pool is below its mark", () => {
    const belowMark: PoolState = { ...pool, highWaterMarkPerShare: 3_000_000n };
    const p = previewWithdrawal(50_000_000n, belowMark);
    expect(p.performanceFee).toBe(0n);
    expect(p.exitFee).toBe(50_000n);
  });

  it("previews nothing for zero shares", () => {
    const p = previewWithdrawal(0n, pool);
    expect(p).toMatchObject({
      gross: 0n,
      performanceFee: 0n,
      exitFee: 0n,
      payout: 0n,
      drainsThePool: false,
    });
  });
});

describe("secondsUntilUnlock", () => {
  it("counts down and then reports ready", () => {
    expect(secondsUntilUnlock(1_000n, 400n)).toBe(600n);
    expect(secondsUntilUnlock(1_000n, 1_000n)).toBe(0n);
    // A request whose cooldown expired long ago is ready, not negative.
    expect(secondsUntilUnlock(1_000n, 9_999n)).toBe(0n);
  });
});
