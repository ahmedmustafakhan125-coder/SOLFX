import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { getAddressEncoder, getProgramDerivedAddress } from "@solana/kit";
import type { Address } from "@solana/kit";

import {
  CLAIM_DISCRIMINATOR,
  IB_ACCOUNT_DISCRIMINATOR,
  IbTier,
  REFERRAL_CONFIG_DISCRIMINATOR,
  REGISTER_IB_DISCRIMINATOR,
  SOLFX_REFERRAL_PROGRAM_ADDRESS,
  SYNC_SPLIT_DISCRIMINATOR,
  SYNC_TRADER_DISCRIMINATOR,
  TRADER_LINK_DISCRIMINATOR,
  entitlement,
  findIbAccountPda,
  findReferralConfigPda,
  findTraderLinkPda,
  getClaimInstruction,
  getIbAccountDecoder,
  getReferralConfigDecoder,
  getRegisterIbInstruction,
  getSyncTraderInstruction,
  isCappedByPool,
  nextTierAt,
  parentOverride,
  tierForVolume,
  tierShareBps,
} from "../referral/index.js";

const A = (s: string) => s as Address;
const ALICE = A("7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs");
const BOB = A("EyvqeDSh2Y4ZhobJY4bF8ueEZAjRx2V3r2GDf35ktPyo");
const SYSTEM = A("11111111111111111111111111111111");

/** A signer stub. Only `.address` is read when building an instruction. */
const signerFor = (address: Address) =>
  ({ address, signTransactions: async () => [] }) as never;

/**
 * The rule from `.claude/rules/solana.md` § 10, applied to a hand-written client:
 * hardcoded discriminators need a test that re-derives them, so a renamed instruction fails
 * the suite instead of a devnet transaction.
 *
 * This whole directory exists because `solfx_referral` has no Codama output. These
 * assertions are what make that acceptable rather than merely convenient.
 */
describe("discriminators re-derive from the program's own names", () => {
  const anchorDisc = (prefix: string, name: string) =>
    new Uint8Array(
      createHash("sha256").update(`${prefix}:${name}`).digest().subarray(0, 8),
    );

  it.each([
    ["register_ib", REGISTER_IB_DISCRIMINATOR],
    ["sync_trader", SYNC_TRADER_DISCRIMINATOR],
    ["claim", CLAIM_DISCRIMINATOR],
    ["sync_split", SYNC_SPLIT_DISCRIMINATOR],
  ])("instruction %s", (name, constant) => {
    expect(constant).toStrictEqual(anchorDisc("global", name as string));
  });

  it.each([
    ["ReferralConfig", REFERRAL_CONFIG_DISCRIMINATOR],
    ["IbAccount", IB_ACCOUNT_DISCRIMINATOR],
    ["TraderLink", TRADER_LINK_DISCRIMINATOR],
  ])("account %s", (name, constant) => {
    expect(constant).toStrictEqual(anchorDisc("account", name as string));
  });
});

/**
 * The seeds are byte strings in `lib.rs`. Re-derived here from the literal bytes rather than
 * from the client's own constants, so a typo in a seed cannot agree with itself.
 */
describe("PDAs derive from the seeds the program declares", () => {
  it('finds the config at ["referral_config"]', async () => {
    const [fromClient] = await findReferralConfigPda();
    const [expected] = await getProgramDerivedAddress({
      programAddress: SOLFX_REFERRAL_PROGRAM_ADDRESS,
      seeds: [new TextEncoder().encode("referral_config")],
    });
    expect(fromClient).toBe(expected);
  });

  it('finds an IB at ["ib", authority]', async () => {
    const [fromClient] = await findIbAccountPda({ authority: ALICE });
    const [expected] = await getProgramDerivedAddress({
      programAddress: SOLFX_REFERRAL_PROGRAM_ADDRESS,
      seeds: [
        new TextEncoder().encode("ib"),
        getAddressEncoder().encode(ALICE),
      ],
    });
    expect(fromClient).toBe(expected);
  });

  it('finds a link at ["link", ibAccount, userAccount] — accounts, not authorities', async () => {
    const [ibAccount] = await findIbAccountPda({ authority: ALICE });
    const [fromClient] = await findTraderLinkPda({
      ibAccount,
      userAccount: BOB,
    });
    const [expected] = await getProgramDerivedAddress({
      programAddress: SOLFX_REFERRAL_PROGRAM_ADDRESS,
      seeds: [
        new TextEncoder().encode("link"),
        getAddressEncoder().encode(ibAccount),
        getAddressEncoder().encode(BOB),
      ],
    });
    expect(fromClient).toBe(expected);
    // The trap this guards: seeding with the *authority* derives a different, valid-looking
    // address the program will reject with a seeds-constraint error naming neither seed.
    const [wrong] = await findTraderLinkPda({
      ibAccount: ALICE,
      userAccount: BOB,
    });
    expect(wrong).not.toBe(expected);
  });
});

/**
 * Account order is positional in Anchor, so these assertions are the only thing between a
 * swapped pair and a constraint violation on chain. Copied from the `#[derive(Accounts)]`
 * structs in `programs/solfx-referral/src/lib.rs`.
 */
describe("instructions carry the accounts the program declares, in order", () => {
  it("register_ib: authority, config, ib_account, system_program", async () => {
    const [config] = await findReferralConfigPda();
    const [ibAccount] = await findIbAccountPda({ authority: ALICE });
    const ix = await getRegisterIbInstruction({
      authority: signerFor(ALICE),
      parent: BOB,
    });

    expect(ix.programAddress).toBe(SOLFX_REFERRAL_PROGRAM_ADDRESS);
    expect(ix.accounts?.map((a) => a.address)).toStrictEqual([
      ALICE,
      config,
      ibAccount,
      SYSTEM,
    ]);
    // The parent rides in the data, not the accounts.
    expect(ix.data?.slice(0, 8)).toStrictEqual(REGISTER_IB_DISCRIMINATOR);
    expect(ix.data?.slice(8)).toStrictEqual(getAddressEncoder().encode(BOB));
  });

  it("register_ib defaults the parent to the system program", async () => {
    const ix = await getRegisterIbInstruction({ authority: signerFor(ALICE) });
    expect(ix.data?.slice(8)).toStrictEqual(getAddressEncoder().encode(SYSTEM));
  });

  it("sync_trader passes the program id for an absent parent, not a shorter list", async () => {
    const withParent = await getSyncTraderInstruction({
      payer: signerFor(BOB),
      ibAuthority: ALICE,
      parentAuthority: BOB,
      userAccount: BOB,
    });
    const without = await getSyncTraderInstruction({
      payer: signerFor(BOB),
      ibAuthority: ALICE,
      userAccount: BOB,
    });

    // Same length either way: omitting an optional account would shift every account
    // after it, which is the failure this convention exists to avoid.
    expect(without.accounts).toHaveLength(7);
    expect(withParent.accounts).toHaveLength(7);
    expect(without.accounts?.[3]?.address).toBe(SOLFX_REFERRAL_PROGRAM_ADDRESS);
    expect(withParent.accounts?.[3]?.address).not.toBe(
      SOLFX_REFERRAL_PROGRAM_ADDRESS,
    );
  });

  it("claim: authority signs read-only, the vaults are writable", async () => {
    const ix = await getClaimInstruction({
      authority: signerFor(ALICE),
      protocol: BOB,
      feeVault: BOB,
      destination: BOB,
      solfxCore: BOB,
    });
    expect(ix.accounts).toHaveLength(8);
    // AccountRole: 0 READONLY, 1 WRITABLE, 2 READONLY_SIGNER, 3 WRITABLE_SIGNER.
    expect(ix.accounts?.[0]?.role).toBe(2);
    expect(ix.accounts?.[1]?.role).toBe(1);
    expect(ix.data).toStrictEqual(CLAIM_DISCRIMINATOR);
  });
});

/**
 * Parity with `crates/solfx-math/src/referral.rs`. Values copied from the Rust tests.
 */
describe("tier arithmetic matches solfx-math", () => {
  const M = 1_000_000_000_000n; // $1M at QUOTE_PRECISION

  // referral.rs: tier boundaries
  it("places volume in the right tier", () => {
    expect(tierForVolume(0n)).toBe(IbTier.Bronze);
    expect(tierForVolume(4n * M)).toBe(IbTier.Bronze);
    expect(tierForVolume(10n * M)).toBe(IbTier.Silver);
    expect(tierForVolume(50n * M)).toBe(IbTier.Gold);
    expect(tierForVolume(200n * M)).toBe(IbTier.Diamond);
  });

  it("treats a boundary as the bottom of the higher tier", () => {
    // Exactly $5M is Silver, not Bronze — the dispute this design exists to prevent.
    expect(tierForVolume(5n * M)).toBe(IbTier.Silver);
    expect(tierForVolume(5n * M - 1n)).toBe(IbTier.Bronze);
    expect(tierForVolume(25n * M)).toBe(IbTier.Gold);
    expect(tierForVolume(100n * M)).toBe(IbTier.Diamond);
  });

  it("pays the shares § 8.5 states", () => {
    expect(tierShareBps(IbTier.Bronze)).toBe(800);
    expect(tierShareBps(IbTier.Silver)).toBe(1_000);
    expect(tierShareBps(IbTier.Gold)).toBe(1_300);
    expect(tierShareBps(IbTier.Diamond)).toBe(1_600);
  });

  it("knows where the next tier starts, and that Diamond has none", () => {
    expect(nextTierAt(IbTier.Bronze)).toBe(5n * M);
    expect(nextTierAt(IbTier.Diamond)).toBeNull();
  });

  /**
   * The cap is the point: § 8.3 funds the pool at 10% of fees, § 8.5 promises the top tiers
   * 13% and 16%. An IB can never be paid more than was earmarked for referrals.
   */
  it("caps an entitlement at what the pool actually holds", () => {
    const poolShare = 1_000_000n; // $1 of referral pool, from a 10% split
    const split = 1_000; // fee_split_referral_bps = 10%

    // Bronze: 8% of the $10 of fees behind that $1 = $0.80, inside the pool's $1.
    expect(entitlement(poolShare, split, IbTier.Bronze)).toBe(800_000n);
    expect(isCappedByPool(poolShare, split, IbTier.Bronze)).toBe(false);

    // Silver: 10% of $10 = $1.00 exactly. Paid in full, not capped.
    expect(entitlement(poolShare, split, IbTier.Silver)).toBe(1_000_000n);
    expect(isCappedByPool(poolShare, split, IbTier.Silver)).toBe(false);

    // Gold wants 13% of $10 = $1.30 from a pool holding $1. Capped.
    expect(entitlement(poolShare, split, IbTier.Gold)).toBe(poolShare);
    expect(isCappedByPool(poolShare, split, IbTier.Gold)).toBe(true);
    expect(entitlement(poolShare, split, IbTier.Diamond)).toBe(poolShare);
  });

  it("returns nothing for degenerate inputs rather than dividing by zero", () => {
    expect(entitlement(0n, 1_000, IbTier.Gold)).toBe(0n);
    expect(entitlement(1_000_000n, 0, IbTier.Gold)).toBe(0n);
    expect(isCappedByPool(0n, 1_000, IbTier.Gold)).toBe(false);
  });

  it("pays a parent a share of the child's earnings, not a second bite", () => {
    // § 8.5's two-level structure: 20% of what the sub-broker earned.
    expect(parentOverride(1_000_000n, 2_000)).toBe(200_000n);
    expect(parentOverride(0n, 2_000)).toBe(0n);
    expect(parentOverride(1_000_000n, 0)).toBe(0n);
    // Rounds down, like the Rust.
    expect(parentOverride(9n, 2_000)).toBe(1n);
  });
});

/**
 * The decoders are written by hand from the Rust structs, so a field in the wrong order
 * reads plausible-looking rubbish rather than failing. These round-trip real byte layouts.
 */
describe("account decoders match the Rust layouts", () => {
  it("decodes a ReferralConfig", () => {
    const bytes = new Uint8Array(8 + 32 + 32 + 2 + 2 + 8 + 8 + 4 + 1 + 64);
    bytes.set(REFERRAL_CONFIG_DISCRIMINATOR, 0);
    bytes.set(getAddressEncoder().encode(ALICE), 8);
    bytes.set(getAddressEncoder().encode(BOB), 40);
    const view = new DataView(bytes.buffer);
    view.setUint16(72, 2_000, true); // override_bps
    view.setUint16(74, 1_000, true); // pool_split_bps
    view.setBigUint64(76, 123_456n, true); // total_accrued
    view.setBigUint64(84, 100n, true); // total_claimed
    view.setUint32(92, 7, true); // ib_count
    bytes[96] = 254; // bump

    const cfg = getReferralConfigDecoder().decode(bytes);
    expect(cfg.admin).toBe(ALICE);
    expect(cfg.protocol).toBe(BOB);
    expect(cfg.overrideBps).toBe(2_000);
    expect(cfg.poolSplitBps).toBe(1_000);
    expect(cfg.totalAccrued).toBe(123_456n);
    expect(cfg.totalClaimed).toBe(100n);
    expect(cfg.ibCount).toBe(7);
    expect(cfg.bump).toBe(254);
  });

  it("decodes an IbAccount", () => {
    const bytes = new Uint8Array(8 + 32 + 32 + 8 * 5 + 4 + 8 + 1 + 64);
    bytes.set(IB_ACCOUNT_DISCRIMINATOR, 0);
    bytes.set(getAddressEncoder().encode(ALICE), 8);
    bytes.set(getAddressEncoder().encode(BOB), 40);
    const view = new DataView(bytes.buffer);
    view.setBigUint64(72, 5_000_000n, true); // unclaimed
    view.setBigUint64(80, 9_000_000n, true); // lifetime_earned
    view.setBigUint64(88, 4_000_000n, true); // lifetime_claimed
    view.setBigUint64(96, 30_000_000_000_000n, true); // referred_volume: $30M -> Gold
    view.setBigUint64(104, 2_500_000n, true); // referred_pool_share
    view.setUint32(112, 12, true); // referred_trader_count
    view.setBigInt64(116, 1_788_000_000n, true); // created_at
    bytes[124] = 253;

    const ib = getIbAccountDecoder().decode(bytes);
    expect(ib.authority).toBe(ALICE);
    expect(ib.parent).toBe(BOB);
    expect(ib.unclaimed).toBe(5_000_000n);
    expect(ib.referredVolume).toBe(30_000_000_000_000n);
    expect(ib.referredTraderCount).toBe(12);
    expect(ib.createdAt).toBe(1_788_000_000n);
    expect(ib.bump).toBe(253);
    // And the figure the dashboard leads with, recomputed rather than trusted.
    expect(tierForVolume(ib.referredVolume)).toBe(IbTier.Gold);
  });
});
