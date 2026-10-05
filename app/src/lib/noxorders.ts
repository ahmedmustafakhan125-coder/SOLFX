/**
 * Orders beyond the market open: targets and tighter stops, resting entries, partial closes.
 *
 * Every builder here sends to an instruction that re-checks everything on chain; the pure
 * helpers at the top only let the page name a refusal before a transaction is signed. They
 * mirror the program's rules in its own units:
 *
 * | Here | On chain |
 * |---|---|
 * | `isTighter` | `eval_orders::is_tighter` — toward the price, strictly |
 * | `entryMet` | `EntryKind::is_met`, on the oracle, inclusive |
 * | `defaultPriceLimit` | `funded_orders::price_limit_fits` — a limit never fills worse than its price |
 * | `holdLeftSecs` | `EVAL_MIN_HOLD_SECS`; a target or a stop move waits for it |
 */
import { findTriggerOrderPda, nox, noxPdas, Direction } from "@solfx/client";
import type { Address, Instruction, TransactionSigner } from "@solana/kit";

import { EVAL, solfxAccounts, type SolfxLegs } from "@/lib/nox";

type Dir = nox.DirectionArgs | Direction;
const isLong = (d: Dir) => Number(d) === Number(Direction.Long);

export const EntryKind = nox.EntryKind;
export type EntryKindArgs = nox.EntryKindArgs;

// --- the rules, mirrored ---------------------------------------------------------------------

/** Can a stop at `next` replace one at `current` — strictly toward the price? */
export function isTighter(
  direction: Dir,
  current: bigint,
  next: bigint
): boolean {
  return isLong(direction) ? next > current : next < current;
}

/** Has the oracle reached an entry's trigger? Inclusive, as the program judges it. */
export function entryMet(
  kind: EntryKindArgs,
  direction: Dir,
  trigger: bigint,
  price: bigint
): boolean {
  const limit = Number(kind) === Number(nox.EntryKind.Limit);
  // A buy limit and a sell stop fill at or below; a sell limit and a buy stop at or above.
  const below = limit === isLong(direction);
  return below ? price <= trigger : price >= trigger;
}

/**
 * The fill bound a funded entry order carries into `open_position`.
 *
 * A **limit** is bounded at its own price: it never fills worse. A **stop** buys a breakout, so
 * its bound sits `slippageBps` beyond the trigger — there is no unbounded order on this venue.
 */
export function defaultPriceLimit(
  kind: EntryKindArgs,
  direction: Dir,
  trigger: bigint,
  slippageBps: number
): bigint {
  if (Number(kind) === Number(nox.EntryKind.Limit)) return trigger;
  const delta =
    (trigger * BigInt(Math.max(0, Math.trunc(slippageBps)))) / 10_000n;
  return isLong(direction) ? trigger + delta : trigger - delta;
}

/** Seconds left on the evaluation's ten-minute hold; zero once it has passed. */
export function holdLeftSecs(openedAt: bigint, now: bigint): number {
  const left = BigInt(EVAL.minHoldSecs) - (now - openedAt);
  return left > 0n ? Number(left) : 0;
}

/** The lowest id not in `used`, up to 255. `undefined` when every id is taken. */
export function nextFreeId(used: readonly number[]): number | undefined {
  for (let id = 0; id <= 255; id++) if (!used.includes(id)) return id;
  return undefined;
}

/**
 * The first reason `*_place_entry_order` would refuse this bracket, or `null`. What can be
 * judged without a price: the program checks the same at placement and everything again at fill.
 */
export function entryBracketRefusal(args: {
  direction: Dir;
  trigger: bigint;
  stop: bigint;
  takeProfit: bigint;
  sizeBase: bigint;
}): string | null {
  const { direction, trigger, stop, takeProfit, sizeBase } = args;
  if (trigger <= 0n) return "enter a trigger price";
  if (sizeBase <= 0n) return "enter a size";
  if (stop <= 0n) return "a stop is mandatory";
  const long = isLong(direction);
  if (long ? stop >= trigger : stop <= trigger)
    return `the stop must be ${long ? "below" : "above"} the trigger`;
  if (
    takeProfit !== 0n &&
    (long ? takeProfit <= trigger : takeProfit >= trigger)
  )
    return `the target must be ${long ? "above" : "below"} the trigger`;
  return null;
}

// --- evaluation builders ---------------------------------------------------------------------

export function evalSetTakeProfitIx(args: {
  signer: TransactionSigner;
  evaluation: Address;
  virtualPosition: Address;
  market: Address;
  legs: SolfxLegs;
  /** Zero clears it. */
  triggerPrice: bigint;
}): Instruction {
  return nox.getEvalSetTakeProfitInstruction({
    trader: args.signer,
    evaluation: args.evaluation,
    virtualPosition: args.virtualPosition,
    market: args.market,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    triggerPrice: args.triggerPrice,
  });
}

export function evalMoveStopIx(args: {
  signer: TransactionSigner;
  evaluation: Address;
  virtualPosition: Address;
  market: Address;
  legs: SolfxLegs;
  newStop: bigint;
}): Instruction {
  return nox.getEvalMoveStopInstruction({
    trader: args.signer,
    evaluation: args.evaluation,
    virtualPosition: args.virtualPosition,
    market: args.market,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    newStop: args.newStop,
  });
}

export async function evalPlaceEntryOrderIx(args: {
  signer: TransactionSigner;
  evaluation: Address;
  orderId: number;
  market: Address;
  marketIndex: number;
  nonce: number;
  direction: nox.DirectionArgs;
  kind: EntryKindArgs;
  triggerPrice: bigint;
  sizeBase: bigint;
  stopLossPrice: bigint;
  takeProfitPrice: bigint;
  expiresAt: bigint;
}): Promise<Instruction> {
  return nox.getEvalPlaceEntryOrderInstructionAsync({
    trader: args.signer,
    evaluation: args.evaluation,
    entryOrder: await noxPdas.findEvalEntryOrder(args.evaluation, args.orderId),
    market: args.market,
    // Codama flattens `EntryOrderParams` into the instruction's own arguments.
    orderId: args.orderId,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    direction: args.direction,
    kind: args.kind,
    triggerPrice: args.triggerPrice,
    sizeBase: args.sizeBase,
    stopLossPrice: args.stopLossPrice,
    takeProfitPrice: args.takeProfitPrice,
    expiresAt: args.expiresAt,
  });
}

export function evalCancelEntryOrderIx(args: {
  signer: TransactionSigner;
  trader: Address;
  evaluation: Address;
  entryOrder: Address;
}): Instruction {
  return nox.getEvalCancelEntryOrderInstruction({
    caller: args.signer,
    trader: args.trader,
    evaluation: args.evaluation,
    entryOrder: args.entryOrder,
  });
}

// --- funded builders -------------------------------------------------------------------------

/** Replace a funded stop with a tighter one: the new one placed, then the old cancelled. */
export async function fundedMoveStopIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  marketIndex: number;
  nonce: number;
  oldOrderId: number;
  newOrderId: number;
  newStop: bigint;
  legs: SolfxLegs;
}): Promise<Instruction> {
  const a = await solfxAccounts(args.mandate, args.marketIndex, args.nonce);
  const [oldStop] = await findTriggerOrderPda({
    position: a.position,
    orderId: args.oldOrderId,
  });
  const [newStop] = await findTriggerOrderPda({
    position: a.position,
    orderId: args.newOrderId,
  });
  return nox.getFundedMoveStopInstructionAsync({
    trader: args.signer,
    mandate: args.mandate,
    protocol: a.protocol,
    userAccount: a.userAccount,
    market: a.market,
    position: a.position,
    oldStop,
    newStop,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    newOrderId: args.newOrderId,
    // Codama renamed the argument: `new_stop` is also an account name on this instruction.
    newStopArg: args.newStop,
  });
}

/** Close part of a funded position. The stop stays and covers whatever is left. */
export async function fundedReduceIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  marketIndex: number;
  nonce: number;
  sizeDelta: bigint;
  priceLimit: bigint;
  legs: SolfxLegs;
}): Promise<Instruction> {
  const a = await solfxAccounts(args.mandate, args.marketIndex, args.nonce);
  return nox.getFundedReducePositionInstructionAsync({
    trader: args.signer,
    mandate: args.mandate,
    protocol: a.protocol,
    userAccount: a.userAccount,
    market: a.market,
    position: a.position,
    traderProfile: await noxPdas.findProfile(args.signer.address),
    collateralVault: a.collateralVault,
    lpPool: a.lpPool,
    lpVault: a.lpVault,
    insuranceFund: a.insuranceFund,
    insuranceVault: a.insuranceVault,
    feeVault: a.feeVault,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    sizeDelta: args.sizeDelta,
    priceLimit: args.priceLimit,
  });
}

export async function fundedPlaceEntryOrderIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  orderId: number;
  marketIndex: number;
  nonce: number;
  stopOrderId: number;
  direction: nox.DirectionArgs;
  kind: EntryKindArgs;
  triggerPrice: bigint;
  priceLimit: bigint;
  sizeBase: bigint;
  collateral: bigint;
  stopLossPrice: bigint;
  expiresAt: bigint;
}): Promise<Instruction> {
  return nox.getFundedPlaceEntryOrderInstructionAsync({
    trader: args.signer,
    mandate: args.mandate,
    entryOrder: await noxPdas.findMandateEntryOrder(args.mandate, args.orderId),
    // Codama flattens `MandateEntryParams` into the instruction's own arguments.
    orderId: args.orderId,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    stopOrderId: args.stopOrderId,
    direction: args.direction,
    kind: args.kind,
    triggerPrice: args.triggerPrice,
    priceLimit: args.priceLimit,
    sizeBase: args.sizeBase,
    collateral: args.collateral,
    stopLossPrice: args.stopLossPrice,
    expiresAt: args.expiresAt,
  });
}

export function fundedCancelEntryOrderIx(args: {
  signer: TransactionSigner;
  trader: Address;
  mandate: Address;
  entryOrder: Address;
}): Instruction {
  return nox.getFundedCancelEntryOrderInstruction({
    caller: args.signer,
    trader: args.trader,
    mandate: args.mandate,
    entryOrder: args.entryOrder,
  });
}

/** Return a settled mandate signer's SOL to its trader. Anyone may send it. */
export async function sweepMandateSignerIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  trader: Address;
}): Promise<Instruction> {
  return nox.getSweepMandateSignerInstruction({
    caller: args.signer,
    mandate: args.mandate,
    mandateSigner: await noxPdas.findMandateSigner(args.mandate),
    trader: args.trader,
  });
}

/**
 * Move margin into (`add`) or out of a funded position. Adding only lowers its risk; removing is
 * held by `solfx-core` to the initial-margin and leverage the position had to meet to exist.
 */
export async function fundedMarginIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  marketIndex: number;
  nonce: number;
  amount: bigint;
  add: boolean;
  legs: SolfxLegs;
}): Promise<Instruction> {
  const a = await solfxAccounts(args.mandate, args.marketIndex, args.nonce);
  const input = {
    trader: args.signer,
    mandate: args.mandate,
    protocol: a.protocol,
    userAccount: a.userAccount,
    market: a.market,
    position: a.position,
    collateralVault: a.collateralVault,
    lpPool: a.lpPool,
    lpVault: a.lpVault,
    insuranceFund: a.insuranceFund,
    insuranceVault: a.insuranceVault,
    feeVault: a.feeVault,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    amount: args.amount,
  };
  return args.add
    ? nox.getFundedAddMarginInstructionAsync(input)
    : nox.getFundedRemoveMarginInstructionAsync(input);
}
