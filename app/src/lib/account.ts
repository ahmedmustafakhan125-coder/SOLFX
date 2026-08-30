/**
 * Target 1: the bootstrap a fresh wallet needs before it can trade.
 *
 * Three things have to exist, and none of them do for a wallet that has never used SolFX:
 * a `UserAccount` PDA, an associated token account for the protocol's collateral mint, and
 * a collateral balance in the vault. This module reports which are missing and builds the
 * instructions to fix each.
 */
import {
  fetchProtocol,
  findCollateralVaultPda,
  findProtocolPda,
  findUserAccountPda,
  getDepositCollateralInstruction,
  getInitializeUserAccountInstruction,
} from "@solfx/client";
import {
  TOKEN_PROGRAM_ADDRESS,
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstructionAsync,
} from "@solana-program/token";
import type { Address, Instruction, Rpc, SolanaRpcApi, TransactionSigner } from "@solana/kit";

/** No referrer. The program binds this once at init and never reassigns it. */
const NO_REFERRER = "11111111111111111111111111111111" as Address;

export type AccountStatus = {
  readonly userAccountPda: Address;
  readonly ata: Address;
  readonly usdcMint: Address;
  /** Null until `initialize_user_account` has run. */
  readonly hasUserAccount: boolean;
  /** Free collateral inside the protocol, at QUOTE_PRECISION. */
  readonly freeCollateral: bigint;
  /** Balance of the wallet's own token account, at QUOTE_PRECISION. */
  readonly walletUsdc: bigint;
  readonly hasAta: boolean;
};

export async function readAccountStatus(
  rpc: Rpc<SolanaRpcApi>,
  owner: Address,
): Promise<AccountStatus> {
  const [protocolPda] = await findProtocolPda();
  const protocol = await fetchProtocol(rpc, protocolPda);
  const usdcMint = protocol.data.usdcMint;

  const [userAccountPda] = await findUserAccountPda({ authority: owner });
  const [ata] = await findAssociatedTokenPda({
    mint: usdcMint,
    owner,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });

  // Both may legitimately not exist, so read raw rather than through a decoder that throws.
  const { value: accounts } = await rpc
    .getMultipleAccounts([userAccountPda, ata], { encoding: "base64" })
    .send();

  const userRaw = accounts[0];
  const ataRaw = accounts[1];

  let freeCollateral = 0n;
  if (userRaw) {
    const { getUserAccountDecoder } = await import("@solfx/client");
    const bytes = base64ToBytes(userRaw.data[0]);
    freeCollateral = getUserAccountDecoder().decode(bytes).freeCollateral;
  }

  let walletUsdc = 0n;
  if (ataRaw) {
    // SPL token account: amount is a u64 at offset 64.
    const bytes = base64ToBytes(ataRaw.data[0]);
    walletUsdc = readU64LE(bytes, 64);
  }

  return {
    userAccountPda,
    ata,
    usdcMint,
    hasUserAccount: userRaw !== null,
    freeCollateral,
    walletUsdc,
    hasAta: ataRaw !== null,
  };
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function readU64LE(bytes: Uint8Array, offset: number): bigint {
  let v = 0n;
  for (let i = 7; i >= 0; i--) v = (v << 8n) | BigInt(bytes[offset + i] ?? 0);
  return v;
}

/** `initialize_user_account`, with the referrer left unset. */
export async function buildInitUserAccount(
  signer: TransactionSigner,
): Promise<Instruction> {
  const [protocolPda] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({ authority: signer.address });
  return getInitializeUserAccountInstruction({
    authority: signer,
    protocol: protocolPda,
    userAccount,
    referrer: NO_REFERRER,
  });
}

/**
 * Everything needed to move `amount` of collateral in, including creating the token account
 * if the wallet has none. The ATA instruction is the idempotent variant, so re-running is
 * harmless.
 */
export async function buildDeposit(
  signer: TransactionSigner,
  usdcMint: Address,
  amount: bigint,
  needsAta: boolean,
): Promise<Instruction[]> {
  const [protocolPda] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({ authority: signer.address });
  const [collateralVault] = await findCollateralVaultPda();
  const [ata] = await findAssociatedTokenPda({
    mint: usdcMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });

  const ixs: Instruction[] = [];
  if (needsAta) {
    ixs.push(
      await getCreateAssociatedTokenIdempotentInstructionAsync({
        payer: signer,
        mint: usdcMint,
        owner: signer.address,
      }),
    );
  }
  ixs.push(
    getDepositCollateralInstruction({
      authority: signer,
      protocol: protocolPda,
      userAccount,
      collateralMint: usdcMint,
      collateralVault,
      userTokenAccount: ata,
      amount,
    }),
  );
  return ixs;
}
