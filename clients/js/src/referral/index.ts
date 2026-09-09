/**
 * A hand-written client for `solfx-referral`.
 *
 * # Why this is not generated
 *
 * Everything under `../generated/` is Codama output from `target/idl/solfx_core.json`.
 * `codama.json` names that one IDL, so the referral programme — a separate program at
 * `J7dwkNcyPjHtRyqkpnqq3wgE6XmYCaENhYZozX2MHsyt` — has no generated client, and neither
 * program currently publishes an IDL account on chain to fetch one from.
 *
 * So this is written out longhand: three accounts, four instructions, three PDAs. It is
 * small enough to be worth writing and small enough to be worth checking, and
 * `__tests__/referral.test.ts` re-derives every discriminator from
 * `sha256("global:<name>")` and `sha256("account:<Name>")` rather than trusting the
 * constants below — the repo rule for hardcoded discriminators, and the only thing standing
 * between a renamed instruction and a devnet transaction that fails for no visible reason.
 *
 * **When `solfx_referral` is added to `codama.json`, delete this directory.** Generated code
 * cannot drift from the program; this can.
 */
import {
  fixDecoderSize,
  getAddressDecoder,
  getAddressEncoder,
  getBytesDecoder,
  getI64Decoder,
  getProgramDerivedAddress,
  getStructDecoder,
  getU16Decoder,
  getU32Decoder,
  getU64Decoder,
  getU8Decoder,
  type Address,
  type Decoder,
  type ProgramDerivedAddress,
} from "@solana/kit";

export const SOLFX_REFERRAL_PROGRAM_ADDRESS =
  "J7dwkNcyPjHtRyqkpnqq3wgE6XmYCaENhYZozX2MHsyt" as Address<"J7dwkNcyPjHtRyqkpnqq3wgE6XmYCaENhYZozX2MHsyt">;

/** Seeds, copied from `programs/solfx-referral/src/lib.rs`. Never retyped elsewhere. */
export const CONFIG_SEED = "referral_config";
export const IB_SEED = "ib";
export const LINK_SEED = "link";

// --- PDAs -----------------------------------------------------------------------------------

const utf8 = new TextEncoder();

function seedBytes(s: string): Uint8Array {
  return utf8.encode(s);
}

/** `["referral_config"]`. The singleton the whole programme hangs off. */
export async function findReferralConfigPda(
  config: { programAddress?: Address } = {},
): Promise<ProgramDerivedAddress> {
  return await getProgramDerivedAddress({
    programAddress: config.programAddress ?? SOLFX_REFERRAL_PROGRAM_ADDRESS,
    seeds: [seedBytes(CONFIG_SEED)],
  });
}

/** `["ib", authority]`. One per introducing broker. */
export async function findIbAccountPda(
  seeds: { authority: Address },
  config: { programAddress?: Address } = {},
): Promise<ProgramDerivedAddress> {
  return await getProgramDerivedAddress({
    programAddress: config.programAddress ?? SOLFX_REFERRAL_PROGRAM_ADDRESS,
    seeds: [seedBytes(IB_SEED), getAddressEncoder().encode(seeds.authority)],
  });
}

/**
 * `["link", ib, user_account]`. The watermark that makes `sync_trader` idempotent.
 *
 * Note both seeds are *account addresses*, not authorities: the IB's PDA and the trader's
 * `UserAccount` PDA. Seeding it with the authorities instead derives an address the program
 * will not accept, and the failure names neither seed.
 */
export async function findTraderLinkPda(
  seeds: { ibAccount: Address; userAccount: Address },
  config: { programAddress?: Address } = {},
): Promise<ProgramDerivedAddress> {
  const enc = getAddressEncoder();
  return await getProgramDerivedAddress({
    programAddress: config.programAddress ?? SOLFX_REFERRAL_PROGRAM_ADDRESS,
    seeds: [
      seedBytes(LINK_SEED),
      enc.encode(seeds.ibAccount),
      enc.encode(seeds.userAccount),
    ],
  });
}

// --- accounts -------------------------------------------------------------------------------

/**
 * Anchor account discriminators: `sha256("account:<StructName>")[..8]`.
 *
 * Re-derived in the tests. A struct rename that changes one of these is a decode that
 * silently reads the wrong fields, which is worse than an error.
 */
export const REFERRAL_CONFIG_DISCRIMINATOR = new Uint8Array([
  102, 148, 171, 235, 148, 83, 250, 140,
]);
export const IB_ACCOUNT_DISCRIMINATOR = new Uint8Array([
  177, 58, 94, 107, 59, 73, 103, 201,
]);
export const TRADER_LINK_DISCRIMINATOR = new Uint8Array([
  71, 170, 112, 75, 243, 80, 192, 173,
]);

export type ReferralConfig = {
  readonly admin: Address;
  /** The `solfx-core` protocol account this programme draws from. */
  readonly protocol: Address;
  /** Parent's share of a sub-broker's earnings, in bps. */
  readonly overrideBps: number;
  /** Mirror of `Protocol.fee_split_referral_bps`, refreshed by `sync_split`. */
  readonly poolSplitBps: number;
  readonly totalAccrued: bigint;
  readonly totalClaimed: bigint;
  readonly ibCount: number;
  readonly bump: number;
};

export function getReferralConfigDecoder(): Decoder<ReferralConfig> {
  return getStructDecoder([
    ["discriminator", fixDecoderSize(getBytesDecoder(), 8)],
    ["admin", getAddressDecoder()],
    ["protocol", getAddressDecoder()],
    ["overrideBps", getU16Decoder()],
    ["poolSplitBps", getU16Decoder()],
    ["totalAccrued", getU64Decoder()],
    ["totalClaimed", getU64Decoder()],
    ["ibCount", getU32Decoder()],
    ["bump", getU8Decoder()],
    ["reserved", fixDecoderSize(getBytesDecoder(), 64)],
  ]) as unknown as Decoder<ReferralConfig>;
}

export type IbAccount = {
  readonly authority: Address;
  /** The IB who recruited this one, or the system program when there is none. */
  readonly parent: Address;
  /** Earned and not yet claimed, in USDC. */
  readonly unclaimed: bigint;
  readonly lifetimeEarned: bigint;
  readonly lifetimeClaimed: bigint;
  /** Referred trading volume driving the tier. */
  readonly referredVolume: bigint;
  /** Referral-pool money this IB's traders generated, before the tier is applied. */
  readonly referredPoolShare: bigint;
  readonly referredTraderCount: number;
  readonly createdAt: bigint;
  readonly bump: number;
};

export function getIbAccountDecoder(): Decoder<IbAccount> {
  return getStructDecoder([
    ["discriminator", fixDecoderSize(getBytesDecoder(), 8)],
    ["authority", getAddressDecoder()],
    ["parent", getAddressDecoder()],
    ["unclaimed", getU64Decoder()],
    ["lifetimeEarned", getU64Decoder()],
    ["lifetimeClaimed", getU64Decoder()],
    ["referredVolume", getU64Decoder()],
    ["referredPoolShare", getU64Decoder()],
    ["referredTraderCount", getU32Decoder()],
    ["createdAt", getI64Decoder()],
    ["bump", getU8Decoder()],
    ["reserved", fixDecoderSize(getBytesDecoder(), 64)],
  ]) as unknown as Decoder<IbAccount>;
}

export type TraderLink = {
  readonly ib: Address;
  readonly userAccount: Address;
  /** Watermark on `UserAccount.referral_fees_generated`. */
  readonly creditedPoolShare: bigint;
  /** Watermark on `UserAccount.lifetime_volume`. */
  readonly creditedVolume: bigint;
  readonly bump: number;
};

export function getTraderLinkDecoder(): Decoder<TraderLink> {
  return getStructDecoder([
    ["discriminator", fixDecoderSize(getBytesDecoder(), 8)],
    ["ib", getAddressDecoder()],
    ["userAccount", getAddressDecoder()],
    ["creditedPoolShare", getU64Decoder()],
    ["creditedVolume", getU64Decoder()],
    ["bump", getU8Decoder()],
    ["reserved", fixDecoderSize(getBytesDecoder(), 32)],
  ]) as unknown as Decoder<TraderLink>;
}

export * from "./tiers.js";
export * from "./instructions.js";
