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
  SOLFX_CORE_PROGRAM_ADDRESS,
  TRIGGER_ORDER_DISCRIMINATOR,
  TriggerKind,
  findMarketPda,
  findProtocolPda,
  findTriggerOrderPda,
  findUserAccountPda,
  getCancelTriggerOrderInstruction,
  getPlaceTriggerOrderInstruction,
  getPositionDecoder,
  getTriggerOrderDecoder,
} from "@solfx/client";
import { getBase58Decoder } from "@solana/kit";
import type {
  Address,
  Instruction,
  Rpc,
  SolanaRpcApi,
  TransactionSigner,
} from "@solana/kit";

import { READ_COMMITMENT } from "@/lib/commitment";

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
  spot: bigint
): boolean {
  const firesWhenSpotRises =
    (kind === TriggerKind.TakeProfit && direction === Direction.Long) ||
    (kind === TriggerKind.StopLoss && direction === Direction.Short);
  return firesWhenSpotRises ? triggerPrice > spot : triggerPrice < spot;
}

/** Which side of the current price this trigger must sit on, for the UI to say so plainly. */
export function requiredSide(
  kind: TriggerKind,
  direction: Direction
): "above" | "below" {
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
  /** Needed to tell an order apart from one left by a previous holder of the same PDA. */
  readonly createdAt: bigint;
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
  limit = 8
): Promise<RestingTrigger[]> {
  const pdas = await Promise.all(
    Array.from({ length: limit }, (_, orderId) =>
      findTriggerOrderPda({ position, orderId }).then(([a]) => a)
    )
  );
  const { value } = await rpc
    .getMultipleAccounts(pdas, {
      commitment: READ_COMMITMENT,
      encoding: "base64",
    })
    .send();

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
      createdAt: order.createdAt,
    });
  }
  return out;
}

/** The lowest id with no account yet. Reusing an occupied one fails "account already in use". */
export async function firstFreeOrderId(
  rpc: Rpc<SolanaRpcApi>,
  position: Address,
  limit = 8
): Promise<number | undefined> {
  const scan = Math.min(limit, MAX_ORDER_ID);
  const pdas = await Promise.all(
    Array.from({ length: scan }, (_, orderId) =>
      findTriggerOrderPda({ position, orderId }).then(([a]) => a)
    )
  );
  const { value } = await rpc
    .getMultipleAccounts(pdas, {
      commitment: READ_COMMITMENT,
      encoding: "base64",
    })
    .send();
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
  const [userAccount] = await findUserAccountPda({
    authority: p.signer.address,
  });
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
/**
 * A resting order that no longer belongs to the position it points at.
 *
 * # The bug this exists to contain
 *
 * A position's PDA is `["position", user_account, market_index, nonce]`, and `firstFreeNonce`
 * picks the **lowest free** nonce. Close a position and open another on the same market and
 * the new one lands at the same address. `close_position` does not cancel outstanding
 * triggers, and `TriggerOrder` records no position-instance identity — no `opened_at`, no
 * generation counter — so a stop left behind by the previous position silently re-attaches to
 * the new one, carrying the old direction's semantics, the old trigger price and the old size.
 *
 * Observed on devnet 2026-09-13: a take-profit at 2500 left over from an ETH **short** was
 * inherited by a fresh ETH **long** entered at 2509.61. A take-profit on a long fires when the
 * price rises past it, so it was already met at the moment the position existed, and the
 * keeper closed the position 19 seconds after it opened. Every layer behaved correctly.
 *
 * The real fix is in the program and cannot be made from here. What the client can do is
 * refuse to treat these as live orders: never draw them on the chart, and give the trader a
 * way to cancel them and reclaim the rent — which the position-row UI cannot, because an
 * orphan has no row.
 */
export type StaleTrigger = RestingTrigger & {
  readonly position: Address;
  readonly marketIndex: number;
  /** `orphaned`: the position is gone. `predates`: the PDA was reused under it. */
  readonly reason: "orphaned" | "predates";
};

/** Byte offset of `authority` in `TriggerOrder`: 8 discriminator + position + user_account. */
const TRIGGER_AUTHORITY_OFFSET = 72n;

/**
 * Every trigger this wallet owns that is not attached to the position currently living at
 * its address.
 *
 * Found by `getProgramAccounts` on the authority field rather than by scanning PDAs, because
 * an orphan's position may be long gone and there is nothing left to scan from.
 */
export async function loadStaleTriggers(
  rpc: Rpc<SolanaRpcApi>,
  owner: Address
): Promise<StaleTrigger[]> {
  // `memcmp.bytes` is a branded base58 string in kit, and `getProgramAccounts` is typed as a
  // union over `withContext`. Neither distinction survives the filter shape below, so both
  // are asserted once here rather than fought at every use.
  type RawAccount = { pubkey: Address; account: { data: [string, string] } };
  const accounts = (await rpc
    .getProgramAccounts(SOLFX_CORE_PROGRAM_ADDRESS, {
      commitment: READ_COMMITMENT,
      encoding: "base64",
      filters: [
        {
          memcmp: {
            offset: 0n,
            bytes: getBase58Decoder().decode(
              TRIGGER_ORDER_DISCRIMINATOR
            ) as never,
            encoding: "base58",
          },
        },
        {
          memcmp: {
            offset: TRIGGER_AUTHORITY_OFFSET,
            bytes: owner as never,
            encoding: "base58",
          },
        },
      ],
    })
    .send()) as unknown as RawAccount[];
  if (accounts.length === 0) return [];

  const decoded = accounts.map((a) => ({
    address: a.pubkey,
    order: getTriggerOrderDecoder().decode(
      Uint8Array.from(atob(a.account.data[0]), (c) => c.charCodeAt(0))
    ),
  }));

  // One read for the positions they point at. Duplicates collapse: a bracket puts two
  // orders on one position.
  const positions: Address[] = [
    ...new Set(decoded.map((d) => d.order.position)),
  ];
  const { value } = await rpc
    .getMultipleAccounts(positions, {
      commitment: READ_COMMITMENT,
      encoding: "base64",
    })
    .send();

  const live = new Map<Address, bigint>();
  positions.forEach((address, i) => {
    const raw = value[i];
    if (!raw) return;
    const p = getPositionDecoder().decode(
      Uint8Array.from(atob(raw.data[0]), (c) => c.charCodeAt(0))
    );
    if (p.sizeBase === 0n) return;
    live.set(address, p.openedAt);
  });

  const out: StaleTrigger[] = [];
  for (const { address, order } of decoded) {
    const openedAt = live.get(order.position);
    // A trigger created before the position it points at was opened cannot belong to it.
    // Equal timestamps are treated as fine: a bracket placed in the same second as the open
    // is the normal case, and the program stamps both from the same clock.
    const reason =
      openedAt === undefined
        ? ("orphaned" as const)
        : openedAt > order.createdAt
          ? ("predates" as const)
          : undefined;
    if (!reason) continue;
    out.push({
      address,
      orderId: order.orderId,
      kind: order.kind,
      triggerPrice: order.triggerPrice,
      sizeBase: order.sizeBase,
      createdAt: order.createdAt,
      position: order.position,
      marketIndex: order.marketIndex,
      reason,
    });
  }
  return out;
}

export function buildCancelTrigger(
  signer: TransactionSigner,
  triggerOrder: Address
): Instruction {
  return getCancelTriggerOrderInstruction({ authority: signer, triggerOrder });
}
