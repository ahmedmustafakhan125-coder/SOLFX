/**
 * What a partial close does to a position, ported from
 * `programs/solfx-core/src/instructions/trader/close_position.rs`.
 *
 * # Why the client needs this at all
 *
 * A trader reducing a position is deciding on two numbers the row does not show: how much
 * collateral comes back, and what is left behind. Both are **floored proportions** of the
 * position, and flooring means the remainder keeps the odd unit rather than the trader —
 * so a preview that divides in floating point and rounds to the nearest cent can promise a
 * unit that never arrives.
 *
 * # The boundary that decides which instruction to send
 *
 * `decrease_position` requires `size_delta < position.size_base`, **strictly**. A "100%
 * partial close" is not a partial close: it is `ReductionExceedsSize`. `close_position` is
 * the instruction for the whole thing, and it also reclaims the position account's rent.
 * [`reduceIsFullClose`] is that boundary, named so a caller cannot forget it.
 */

/** Base units per unit of notional. Mirrors `NOTIONAL_DIVISOR`. */
const NOTIONAL_DIVISOR = 1_000_000_000_000n;

/** The position fields a reduction reads. Field names match `Position`. */
export type ReduciblePosition = {
  readonly sizeBase: bigint;
  readonly collateral: bigint;
  readonly entryNotional: bigint;
};

export type ReducePreview = {
  /** Size being closed, in base units. */
  readonly sizeDelta: bigint;
  /** Size the position keeps. */
  readonly remainingSize: bigint;
  /**
   * Collateral released to free collateral, floored.
   *
   * `collateral × size_delta / size_base`. Floored, so a partial close never releases more
   * than its share — the remainder stays with the surviving position.
   */
  readonly releasedCollateral: bigint;
  /** Collateral the position keeps. */
  readonly remainingCollateral: bigint;
  /**
   * Entry notional attributed to the closed portion, floored. Open interest is removed at
   * the notional recorded when this size entered the book, never at today's price.
   */
  readonly closedEntryNotional: bigint;
  readonly remainingEntryNotional: bigint;
  /** True when this reduction closes the whole position. */
  readonly isFullClose: boolean;
};

/** `a * b / d`, floored. The program's `checked_mul` then `checked_div`. */
function mulDivFloor(a: bigint, b: bigint, d: bigint): bigint {
  return (a * b) / d;
}

/**
 * Whether `sizeDelta` closes the position outright.
 *
 * The caller must send `close_position` when this is true and `decrease_position` when it is
 * false. Sending the wrong one fails: `decrease_position` rejects an equal size with
 * `ReductionExceedsSize`, and there is no partial form of `close_position`.
 */
export function reduceIsFullClose(
  sizeDelta: bigint,
  position: Pick<ReduciblePosition, "sizeBase">,
): boolean {
  return sizeDelta >= position.sizeBase;
}

/**
 * Split a position at `sizeDelta`, the way `reduce` does.
 *
 * Throws on a delta of zero or one larger than the position — the same two conditions the
 * program rejects as `ZeroAmount` and `ReductionExceedsSize`. A preview that quietly clamps
 * would hide the error until the wallet had already signed.
 */
export function previewReduce(
  position: ReduciblePosition,
  sizeDelta: bigint,
): ReducePreview {
  if (sizeDelta <= 0n) {
    throw new Error("previewReduce: size must be positive (ZeroAmount)");
  }
  if (sizeDelta > position.sizeBase) {
    throw new Error(
      "previewReduce: size exceeds the position (ReductionExceedsSize)",
    );
  }

  const isFullClose = sizeDelta === position.sizeBase;

  const releasedCollateral = isFullClose
    ? position.collateral
    : mulDivFloor(position.collateral, sizeDelta, position.sizeBase);

  const closedEntryNotional = isFullClose
    ? position.entryNotional
    : mulDivFloor(position.entryNotional, sizeDelta, position.sizeBase);

  return {
    sizeDelta,
    remainingSize: position.sizeBase - sizeDelta,
    releasedCollateral,
    remainingCollateral: position.collateral - releasedCollateral,
    closedEntryNotional,
    remainingEntryNotional: position.entryNotional - closedEntryNotional,
    isFullClose,
  };
}

/**
 * The size that closes `percent` of a position, floored.
 *
 * 100 returns the whole size exactly, rather than a floored approximation of it — otherwise
 * the "Close all" preset would leave a dust position behind and route to the wrong
 * instruction. Every other percentage floors, which is the direction that leaves the
 * position slightly larger rather than overshooting it.
 */
export function sizeForPercent(
  position: Pick<ReduciblePosition, "sizeBase">,
  percent: number,
): bigint {
  if (percent >= 100) return position.sizeBase;
  if (percent <= 0) return 0n;
  return mulDivFloor(position.sizeBase, BigInt(Math.trunc(percent)), 100n);
}

/**
 * Unrealised P&L on the portion being closed, at `markPrice`, **before fees and carry**.
 *
 * Deliberately not called "realised": the fill goes through the spread at an execution price
 * that is adverse to the trader, and the close fee and accrued carry come off after. This is
 * the same figure the position row already shows as unrealised, scaled to the closed
 * portion, and callers must label it the same way.
 *
 * `isLong` rather than the `Direction` enum so this module stays free of generated imports.
 */
export function pnlOnClosedPortion(
  preview: Pick<ReducePreview, "sizeDelta" | "closedEntryNotional">,
  markPrice: bigint,
  isLong: boolean,
): bigint {
  const notionalNow = mulDivFloor(preview.sizeDelta, markPrice, NOTIONAL_DIVISOR);
  return isLong
    ? notionalNow - preview.closedEntryNotional
    : preview.closedEntryNotional - notionalNow;
}
