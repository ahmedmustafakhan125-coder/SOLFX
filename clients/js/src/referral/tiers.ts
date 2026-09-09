/**
 * IB tier arithmetic, ported from `crates/solfx-math/src/referral.rs`.
 *
 * # Why an IB needs this in the browser
 *
 * The whole pitch of the programme is that a broker can *audit* their own rebates. Every
 * figure below is derivable from public on-chain state, so the dashboard recomputes the
 * entitlement rather than displaying whatever number the chain last wrote — and if the two
 * disagree, the IB can see that they disagree. A dashboard that only echoes
 * `IbAccount.unclaimed` would be the same ledger-you-must-trust that every FX affiliate
 * network already has.
 *
 * Rounds **down** everywhere the Rust rounds down. The protocol never over-pays a rebate on
 * a rounding boundary; the remainder stays in the pool for the next claim.
 */

const BPS_PRECISION = 10_000n;

export enum IbTier {
  Bronze = 0,
  Silver = 1,
  Gold = 2,
  Diamond = 3,
}

/**
 * Volume thresholds and shares from ARCHITECTURE § 8.5, in USDC at `QUOTE_PRECISION`.
 *
 * Boundaries are **inclusive at the bottom of the higher tier**: exactly $5M is Silver, not
 * Bronze. Stated explicitly in the Rust and repeated here because a boundary that moves
 * under an IB is precisely the dispute this design exists to prevent.
 */
const TIER_TABLE: readonly (readonly [bigint, number])[] = [
  [5_000_000_000_000n, 800], // < $5M    -> Bronze   8%
  [25_000_000_000_000n, 1_000], // < $25M   -> Silver  10%
  [100_000_000_000_000n, 1_300], // < $100M  -> Gold    13%
];

/** Mirrors `IbTier::for_volume`. */
export function tierForVolume(thirtyDayVolume: bigint): IbTier {
  if (thirtyDayVolume < TIER_TABLE[0]![0]) return IbTier.Bronze;
  if (thirtyDayVolume < TIER_TABLE[1]![0]) return IbTier.Silver;
  if (thirtyDayVolume < TIER_TABLE[2]![0]) return IbTier.Gold;
  return IbTier.Diamond;
}

/** The IB's share of trading fees, in bps. Mirrors `IbTier::share_bps`. */
export function tierShareBps(tier: IbTier): number {
  switch (tier) {
    case IbTier.Bronze:
      return TIER_TABLE[0]![1];
    case IbTier.Silver:
      return TIER_TABLE[1]![1];
    case IbTier.Gold:
      return TIER_TABLE[2]![1];
    case IbTier.Diamond:
      return 1_600;
  }
}

export const TIER_NAMES: Readonly<Record<IbTier, string>> = {
  [IbTier.Bronze]: "Bronze",
  [IbTier.Silver]: "Silver",
  [IbTier.Gold]: "Gold",
  [IbTier.Diamond]: "Diamond",
};

/** The volume at which the next tier begins, or `null` for Diamond. */
export function nextTierAt(tier: IbTier): bigint | null {
  switch (tier) {
    case IbTier.Bronze:
      return TIER_TABLE[0]![0];
    case IbTier.Silver:
      return TIER_TABLE[1]![0];
    case IbTier.Gold:
      return TIER_TABLE[2]![0];
    case IbTier.Diamond:
      return null;
  }
}

/**
 * What an IB has earned on `poolShare` of accrued referral money. Mirrors
 * `referral::entitlement`.
 *
 * ```text
 * fees_generated = pool_share × 10_000 / pool_split_bps
 * entitlement    = min(fees_generated × tier_bps / 10_000,  pool_share)
 * ```
 *
 * **The cap is the whole point.** § 8.3 sets the referral pool at 10% of fees and § 8.5 sets
 * tiers at 8/10/13/16%; those do not reconcile, so a Gold or Diamond IB is paid what the
 * pool actually holds rather than drawing on treasury money sitting in the same vault. At
 * the launch split, Bronze and Silver are paid in full and Gold and Diamond are capped.
 */
export function entitlement(
  poolShare: bigint,
  poolSplitBps: number,
  tier: IbTier,
): bigint {
  if (poolShare === 0n || poolSplitBps === 0) return 0n;
  const feesGenerated = (poolShare * BPS_PRECISION) / BigInt(poolSplitBps);
  const desired = (feesGenerated * BigInt(tierShareBps(tier))) / BPS_PRECISION;
  return desired < poolShare ? desired : poolShare;
}

/**
 * A parent IB's override on a sub-broker's earnings. Mirrors `referral::parent_override`.
 *
 * The parent earns `overrideBps` of what the child earned — **not** a second bite of the
 * trader's fees. Paid from the same pool and deducted from the same budget, so a two-level
 * chain can never cost more than a one-level one.
 */
export function parentOverride(
  childEarned: bigint,
  overrideBps: number,
): bigint {
  if (childEarned === 0n || overrideBps === 0) return 0n;
  return (childEarned * BigInt(overrideBps)) / BPS_PRECISION;
}

/**
 * Whether an entitlement was cut short by the pool's budget.
 *
 * Surfaced separately because "you earned less than your tier says" is exactly the figure an
 * IB will query, and the honest answer is that the tier is a share of fees while the pool is
 * a share of the same fees — not that they were short-changed.
 */
export function isCappedByPool(
  poolShare: bigint,
  poolSplitBps: number,
  tier: IbTier,
): boolean {
  if (poolShare === 0n || poolSplitBps === 0) return false;
  const feesGenerated = (poolShare * BPS_PRECISION) / BigInt(poolSplitBps);
  const desired = (feesGenerated * BigInt(tierShareBps(tier))) / BPS_PRECISION;
  return desired > poolShare;
}
