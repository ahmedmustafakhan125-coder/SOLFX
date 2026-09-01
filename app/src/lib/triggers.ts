/**
 * Stop-loss and take-profit orders.
 *
 * The feature every retail FX trader assumes exists, and the one place where being explicit
 * about what is *not* promised matters most.
 *
 * **A trigger is not a guaranteed fill price.** It fires when the *oracle* crosses the trigger
 * price, and then closes at the execution price — which includes the adverse spread, and in a
 * gap may be far past the trigger. That is how a stop works at any broker, and the program's
 * own documentation ([`trigger_order.rs`]) says the UI must state it, because the alternative
 * is a trader who believes they were promised something and finds out otherwise during the one
 * event where it matters.
 *
 * The order rests on chain rather than in our keeper, which is the point: anyone can execute
 * it, the condition is public, and the trader can verify it exists without trusting us.
 */
import {
  Direction,
  TriggerKind,
  findMarketPda,
  findProtocolPda,
  findTriggerOrderPda,
  findUserAccountPda,
  getCancelTriggerOrderInstruction,
  getPlaceTriggerOrderInstruction,
  getTriggerOrderDecoder,
} from "@solfx/client";
import type { Address, Instruction, Rpc, SolanaRpcApi, TransactionSigner } from "@solana/kit";

/** Placing one reads the oracle to check the side, so it costs more than a bare write. */
export const PLACE_TRIGGER_CU = 90_000;
export const CANCEL_TRIGGER_CU = 30_000;

/** `order_id` is a u8 in both the seed and the instruction data, so 256 per position. */
const MAX_ORDER_ID = 256;

/**
 * Is `triggerPrice` on the side the program will accept, given the position and the oracle?
 *
 * Mirrors `TriggerKind::is_placeable`. A take-profit below the market on a long is already
 * met, and so is a stop-loss above it — either would fire on the next keeper pass, which is
 * not an order, it is a delayed market close with extra steps. The program rejects both with
 * `TriggerAlreadyMet`; checking here turns that into a sentence the trader reads before
 * signing rather than a failed transaction afterwards.
 *
 * Deliberately the *only* rule duplicated from the program, and it is duplicated because it
 * is a UI affordance rather than a safety check — the program re-checks it regardless.
 */
export function isPlaceable(
  kind: TriggerKind,
  direction: Direction,
  triggerPrice: bigint,
  spot: bigint,
): boolean {
  const firesWhenSpotRises =
    (kind === TriggerKind.TakeProfit && direction === Direction.Long) ||
    (kind === TriggerKind.StopLoss && direction === Direction.Short);
  return firesWhenSpotRises ? triggerPrice > spot : triggerPrice < spot;
}

/** Which side of the current price this trigger must sit on, for the UI to say so plainly. */
export function requiredSide(kind: TriggerKind, direction: Direction): "above" | "below" {
  const firesWhenSpotRises =
    (kind === TriggerKind.TakeProfit && direction === Direction.Long) ||
    (kind === TriggerKind.StopLoss && direction === Direction.Short);
  return firesWhenSpotRises ? "above" : "below";
}

export type RestingTrigger = {
  readonly address: Address;
  readonly orderId: number;
  readonly kind: TriggerKind;
  readonly triggerPrice: bigint;
  readonly sizeBase: bigint;
};

/**
 * The orders resting on one position.
 *
 * Scanned rather than enumerated, for the same reason `firstFreeNonce` scans: nothing on
 * chain lists a position's orders, the id is only a PDA seed. `limit` bounds the scan; a
 * trader with more than a handful of orders on one position is not a case worth a slower page.
 */
export async function readTriggers(
  rpc: Rpc<SolanaRpcApi>,
  position: Address,
  limit = 8,
): Promise<RestingTrigger[]> {
  const pdas = await Promise.all(
    Array.from({ length: limit }, (_, orderId) =>
      findTriggerOrderPda({ position, orderId }).then(([a]) => a),
    ),
  );
  const { value } = await rpc.getMultipleAccounts(pdas, { encoding: "base64" }).send();

  const out: RestingTrigger[] = [];
  const decoder = getTriggerOrderDecoder();
  for (let orderId = 0; orderId < pdas.length; orderId++) {
    const raw = value[orderId];
    const address = pdas[orderId];
    if (!raw || !address) continue;
    const bytes = Uint8Array.from(atob(raw.data[0]), (c) => c.charCodeAt(0));
    const order = decoder.decode(bytes);
    out.push({
      address,
      orderId,
      kind: order.kind,
      triggerPrice: order.triggerPrice,
      sizeBase: order.sizeBase,
    });
  }
  return out;
}

/** The lowest id with no account yet. Reusing an occupied one fails "account already in use". */
export async function firstFreeOrderId(
  rpc: Rpc<SolanaRpcApi>,
  position: Address,
  limit = 8,
): Promise<number | undefined> {
  const scan = Math.min(limit, MAX_ORDER_ID);
  const pdas = await Promise.all(
    Array.from({ length: scan }, (_, orderId) =>
      findTriggerOrderPda({ position, orderId }).then(([a]) => a),
    ),
  );
  const { value } = await rpc.getMultipleAccounts(pdas, { encoding: "base64" }).send();
  const free = value.findIndex((a) => a === null);
  return free === -1 ? undefined : free;
}

export type PlaceParams = {
  readonly signer: TransactionSigner;
  readonly marketIndex: number;
  readonly position: Address;
  readonly orderId: number;
  readonly kind: TriggerKind;
  /** At PRICE_PRECISION. */
  readonly triggerPrice: bigint;
  /** Base units to close. May be less than the position, for scaling out. */
  readonly sizeBase: bigint;
  readonly priceUpdate: Address;
};

export async function buildPlaceTrigger(p: PlaceParams): Promise<Instruction> {
  const [protocol] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({ authority: p.signer.address });
  const [market] = await findMarketPda({ marketIndex: p.marketIndex });
  const [triggerOrder] = await findTriggerOrderPda({
    position: p.position,
    orderId: p.orderId,
  });

  return getPlaceTriggerOrderInstruction({
    authority: p.signer,
    protocol,
    userAccount,
    market,
    position: p.position,
    triggerOrder,
    priceUpdate: p.priceUpdate,
    // secondaryPriceUpdate and quoteConversionPriceUpdate omitted: the listed markets are
    // direct, so the program expects the program-id sentinel in both slots.
    orderId: p.orderId,
    kind: p.kind,
    triggerPrice: p.triggerPrice,
    sizeBase: p.sizeBase,
  });
}

/**
 * Cancel takes two accounts and no oracle: the authority stored on the order is the only one
 * that may cancel it, and that is the whole check.
 */
export function buildCancelTrigger(
  signer: TransactionSigner,
  triggerOrder: Address,
): Instruction {
  return getCancelTriggerOrderInstruction({ authority: signer, triggerOrder });
}
