/**
 * Target 2: opening a position.
 *
 * `open_position` takes sixteen accounts, three of which are optional oracle legs. Anchor's
 * convention for an absent optional account is to pass the program id as a sentinel, and
 * Codama generates the same: `getAccountMetaFactory(programAddress, "programId")`. So the
 * two extra legs are simply omitted here, and account ordering is preserved for us.
 */
import {
  Direction,
  findCollateralVaultPda,
  findFeeVaultPda,
  findInsuranceFundPda,
  findInsuranceVaultPda,
  findLpPoolPda,
  findLpVaultPda,
  findMarketPda,
  findPositionPda,
  findProtocolPda,
  findUserAccountPda,
  getOpenPositionInstruction,
} from "@solfx/client";
import type { Address, Instruction, Rpc, SolanaRpcApi, TransactionSigner } from "@solana/kit";

const BPS = 10_000n;

/** Measured at ~59k against a 140k ceiling; the request leaves room without being wasteful. */
export const OPEN_POSITION_CU = 120_000;

export type OpenParams = {
  readonly signer: TransactionSigner;
  readonly marketIndex: number;
  readonly direction: Direction;
  readonly sizeBase: bigint;
  readonly collateral: bigint;
  /** Oracle price at PRICE_PRECISION. */
  readonly price: bigint;
  readonly slippageBps: number;
  readonly priceUpdate: Address;
  readonly nonce: number;
};

/**
 * The slippage bound the program will actually enforce.
 *
 * A **maximum** when buying, a **minimum** when selling. There is no "disabled" value:
 * `validate_slippage` always compares, so passing 0 is not a way to opt out — it is a bound
 * of zero, which a long can never satisfy.
 */
export function priceLimitFor(direction: Direction, price: bigint, slippageBps: number): bigint {
  const delta = (price * BigInt(slippageBps)) / BPS;
  return direction === Direction.Long ? price + delta : price - delta;
}

/**
 * Find a nonce with no position account yet.
 *
 * A trader may hold several positions on one market, distinguished only by nonce, and
 * nothing on chain enumerates them — so a client has to scan. Reusing an occupied nonce
 * fails with "account already in use", which reads like a protocol error and is not one.
 */
export async function firstFreeNonce(
  rpc: Rpc<SolanaRpcApi>,
  userAccount: Address,
  marketIndex: number,
  limit = 8,
): Promise<number | undefined> {
  const pdas = await Promise.all(
    Array.from({ length: limit }, (_, nonce) =>
      findPositionPda({ userAccount, marketIndex, nonce }).then(([a]) => a),
    ),
  );
  const { value } = await rpc.getMultipleAccounts(pdas, { encoding: "base64" }).send();
  const free = value.findIndex((a) => a === null);
  return free === -1 ? undefined : free;
}

export async function buildOpenPosition(p: OpenParams): Promise<Instruction> {
  const [protocol] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({ authority: p.signer.address });
  const [market] = await findMarketPda({ marketIndex: p.marketIndex });
  const [position] = await findPositionPda({
    userAccount,
    marketIndex: p.marketIndex,
    nonce: p.nonce,
  });
  const [collateralVault] = await findCollateralVaultPda();
  const [lpPool] = await findLpPoolPda();
  const [lpVault] = await findLpVaultPda();
  const [insuranceFund] = await findInsuranceFundPda();
  const [insuranceVault] = await findInsuranceVaultPda();
  const [feeVault] = await findFeeVaultPda();

  return getOpenPositionInstruction({
    authority: p.signer,
    protocol,
    userAccount,
    market,
    position,
    collateralVault,
    lpPool,
    lpVault,
    insuranceFund,
    insuranceVault,
    feeVault,
    priceUpdate: p.priceUpdate,
    // secondaryPriceUpdate and quoteConversionPriceUpdate omitted: this market is direct and
    // USD-quoted, so the program expects the program-id sentinel in both slots.
    marketIndex: p.marketIndex,
    nonce: p.nonce,
    direction: p.direction,
    sizeBase: p.sizeBase,
    collateral: p.collateral,
    priceLimit: priceLimitFor(p.direction, p.price, p.slippageBps),
  });
}
