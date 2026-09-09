/**
 * Instruction builders for `solfx-referral`.
 *
 * Account order below is copied from the `#[derive(Accounts)]` structs in
 * `programs/solfx-referral/src/lib.rs` and must stay in that order — Anchor matches accounts
 * positionally, so a swapped pair is not a type error anywhere, it is a runtime constraint
 * violation whose message names neither account.
 *
 * Optional accounts follow the same convention the generated client uses
 * (`optionalAccountStrategy: "programId"`): an absent account is passed as the program's own
 * address rather than omitted, because omitting it would shift every account after it.
 */
import {
  AccountRole,
  getAddressEncoder,
  type AccountMeta,
  type AccountSignerMeta,
  type Address,
  type Instruction,
  type InstructionWithAccounts,
  type InstructionWithData,
  type TransactionSigner,
} from "@solana/kit";

import {
  SOLFX_REFERRAL_PROGRAM_ADDRESS,
  findIbAccountPda,
  findReferralConfigPda,
  findTraderLinkPda,
} from "./index.js";

/**
 * What every builder here returns.
 *
 * The generated client spells out one exact tuple type per instruction, which is worth it
 * for code a machine writes and not for four builders a person maintains. The account list
 * is built as a plain array first and returned as `accounts`, which keeps the `signer` on
 * the metas that carry one — without it `prepare` cannot find the signer and the transaction
 * goes out unsigned — while still satisfying kit's `Instruction`.
 */
type ReferralInstruction = Instruction<typeof SOLFX_REFERRAL_PROGRAM_ADDRESS> &
  InstructionWithAccounts<readonly (AccountMeta | AccountSignerMeta)[]> &
  InstructionWithData<Uint8Array>;

const SYSTEM_PROGRAM_ADDRESS =
  "11111111111111111111111111111111" as Address<"11111111111111111111111111111111">;
const TOKEN_PROGRAM_ADDRESS =
  "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" as Address<"TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA">;

/** `sha256("global:<name>")[..8]`. Re-derived in `__tests__/referral.test.ts`. */
export const REGISTER_IB_DISCRIMINATOR = new Uint8Array([
  0, 105, 9, 65, 155, 130, 193, 120,
]);
export const SYNC_TRADER_DISCRIMINATOR = new Uint8Array([
  42, 166, 90, 146, 18, 235, 59, 253,
]);
export const CLAIM_DISCRIMINATOR = new Uint8Array([
  62, 198, 214, 193, 213, 159, 108, 210,
]);
export const SYNC_SPLIT_DISCRIMINATOR = new Uint8Array([
  81, 167, 89, 39, 246, 149, 188, 208,
]);

function withAddress(disc: Uint8Array, address: Address): Uint8Array {
  const encoded = getAddressEncoder().encode(address);
  const out = new Uint8Array(disc.length + encoded.length);
  out.set(disc, 0);
  out.set(encoded, disc.length);
  return out;
}

/**
 * Register as an introducing broker, optionally under a parent.
 *
 * Permissionless — anyone may become an IB. Earning anything still requires a trader to have
 * named them at account creation, and that binding is written once and never reassigned.
 *
 * `parent` must not equal the authority; the program rejects self-parenting.
 */
export async function getRegisterIbInstruction(input: {
  authority: TransactionSigner;
  /** The recruiting IB, or the system program for none. */
  parent?: Address;
}): Promise<ReferralInstruction> {
  const parent = input.parent ?? SYSTEM_PROGRAM_ADDRESS;
  const [config] = await findReferralConfigPda();
  const [ibAccount] = await findIbAccountPda({
    authority: input.authority.address,
  });

  const accounts: readonly (AccountMeta | AccountSignerMeta)[] = [
    {
      address: input.authority.address,
      role: AccountRole.WRITABLE_SIGNER,
      signer: input.authority,
    },
    { address: config, role: AccountRole.WRITABLE },
    { address: ibAccount, role: AccountRole.WRITABLE },
    { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
  ];
  return {
    programAddress: SOLFX_REFERRAL_PROGRAM_ADDRESS,
    accounts,
    data: withAddress(REGISTER_IB_DISCRIMINATOR, parent),
  } as ReferralInstruction;
}

/**
 * Credit an IB for everything a referred trader has generated since the last sync.
 *
 * Permissionless and idempotent: it credits only the delta above the `TraderLink` watermark
 * and then advances it, so calling it twice credits nothing the second time and never
 * calling it loses nothing. That is what removes the payout schedule an IB would otherwise
 * have to trust — anyone can push the ledger forward, including the IB themselves.
 */
export async function getSyncTraderInstruction(input: {
  payer: TransactionSigner;
  /** The IB being credited, by their wallet. */
  ibAuthority: Address;
  /** The parent IB's wallet, when the IB has one. */
  parentAuthority?: Address;
  /** The referred trader's `solfx-core` `UserAccount` PDA. */
  userAccount: Address;
}): Promise<ReferralInstruction> {
  const [config] = await findReferralConfigPda();
  const [ibAccount] = await findIbAccountPda({ authority: input.ibAuthority });
  const [link] = await findTraderLinkPda({
    ibAccount,
    userAccount: input.userAccount,
  });

  const parentIb = input.parentAuthority
    ? (await findIbAccountPda({ authority: input.parentAuthority }))[0]
    : SOLFX_REFERRAL_PROGRAM_ADDRESS;

  const accounts: readonly (AccountMeta | AccountSignerMeta)[] = [
    {
      address: input.payer.address,
      role: AccountRole.WRITABLE_SIGNER,
      signer: input.payer,
    },
    { address: config, role: AccountRole.WRITABLE },
    { address: ibAccount, role: AccountRole.WRITABLE },
    { address: parentIb, role: AccountRole.WRITABLE },
    { address: input.userAccount, role: AccountRole.READONLY },
    { address: link, role: AccountRole.WRITABLE },
    { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
  ];
  return {
    programAddress: SOLFX_REFERRAL_PROGRAM_ADDRESS,
    accounts,
    data: new Uint8Array(SYNC_TRADER_DISCRIMINATOR),
  } as ReferralInstruction;
}

/**
 * Draw everything accrued into the IB's own token account.
 *
 * No approval, no schedule, no counterparty — the IB decides when. The transfer happens
 * inside a CPI into `solfx-core`, which is why `protocol` and `feeVault` appear here as
 * plain accounts: this program does not validate them, the core does.
 */
export async function getClaimInstruction(input: {
  authority: TransactionSigner;
  /** `solfx-core`'s protocol PDA. */
  protocol: Address;
  /** `solfx-core`'s fee vault PDA, where referral money sits. */
  feeVault: Address;
  /** The IB's USDC token account. */
  destination: Address;
  /** `solfx-core`'s program address. */
  solfxCore: Address;
}): Promise<ReferralInstruction> {
  const [config] = await findReferralConfigPda();
  const [ibAccount] = await findIbAccountPda({
    authority: input.authority.address,
  });

  const accounts: readonly (AccountMeta | AccountSignerMeta)[] = [
    {
      address: input.authority.address,
      role: AccountRole.READONLY_SIGNER,
      signer: input.authority,
    },
    { address: config, role: AccountRole.WRITABLE },
    { address: ibAccount, role: AccountRole.WRITABLE },
    { address: input.protocol, role: AccountRole.WRITABLE },
    { address: input.feeVault, role: AccountRole.WRITABLE },
    { address: input.destination, role: AccountRole.WRITABLE },
    { address: input.solfxCore, role: AccountRole.READONLY },
    { address: TOKEN_PROGRAM_ADDRESS, role: AccountRole.READONLY },
  ];
  return {
    programAddress: SOLFX_REFERRAL_PROGRAM_ADDRESS,
    accounts,
    data: new Uint8Array(CLAIM_DISCRIMINATOR),
  } as ReferralInstruction;
}

/**
 * Re-read the core's referral split into the config.
 *
 * Permissionless: it copies a public number out of an owner-checked account, so there is
 * nothing to gain by calling it and something to lose by being unable to.
 */
export async function getSyncSplitInstruction(input: {
  protocol: Address;
}): Promise<ReferralInstruction> {
  const [config] = await findReferralConfigPda();
  const accounts: readonly (AccountMeta | AccountSignerMeta)[] = [
    { address: config, role: AccountRole.WRITABLE },
    { address: input.protocol, role: AccountRole.READONLY },
  ];
  return {
    programAddress: SOLFX_REFERRAL_PROGRAM_ADDRESS,
    accounts,
    data: new Uint8Array(SYNC_SPLIT_DISCRIMINATOR),
  } as ReferralInstruction;
}
