/**
 * The two PDAs Codama could not generate.
 *
 * Codama emits a helper for every account whose seeds the IDL fully determines. `Position`
 * and `TriggerOrder` mix an account key with instruction arguments, so it emits neither —
 * and these are the two a trading UI needs most.
 *
 * Seeds are transcribed from `programs/solfx-core/src/constants.rs` and the account structs
 * that use them. Note `TRIGGER_SEED` is **`"order"`**, not `"trigger"`: a client that guesses
 * the obvious string derives an address that exists nowhere.
 */
import {
  getAddressEncoder,
  getBytesEncoder,
  getProgramDerivedAddress,
  getU16Encoder,
  getU8Encoder,
  type Address,
  type ProgramDerivedAddress,
} from "@solana/kit";

import { SOLFX_CORE_PROGRAM_ADDRESS } from "./generated/programs/solfxCore.js";

const utf8 = (s: string) => getBytesEncoder().encode(new TextEncoder().encode(s));

export type PositionSeeds = {
  /** The `UserAccount` PDA, not the wallet that owns it. */
  readonly userAccount: Address;
  readonly marketIndex: number;
  /** Distinguishes several positions held by one user on one market. */
  readonly nonce: number;
};

/** `["position", user_account, market_index (u16 LE), nonce (u8)]` */
export async function findPositionPda(
  seeds: PositionSeeds,
  config: { programAddress?: Address } = {},
): Promise<ProgramDerivedAddress> {
  const { programAddress = SOLFX_CORE_PROGRAM_ADDRESS } = config;
  return await getProgramDerivedAddress({
    programAddress,
    seeds: [
      utf8("position"),
      getAddressEncoder().encode(seeds.userAccount),
      getU16Encoder().encode(seeds.marketIndex),
      getU8Encoder().encode(seeds.nonce),
    ],
  });
}

export type TriggerOrderSeeds = {
  readonly position: Address;
  readonly orderId: number;
};

/** `["order", position, order_id (u8)]` — the seed really is `order`. */
export async function findTriggerOrderPda(
  seeds: TriggerOrderSeeds,
  config: { programAddress?: Address } = {},
): Promise<ProgramDerivedAddress> {
  const { programAddress = SOLFX_CORE_PROGRAM_ADDRESS } = config;
  return await getProgramDerivedAddress({
    programAddress,
    seeds: [
      utf8("order"),
      getAddressEncoder().encode(seeds.position),
      getU8Encoder().encode(seeds.orderId),
    ],
  });
}
