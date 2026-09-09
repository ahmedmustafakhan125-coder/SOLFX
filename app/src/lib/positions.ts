/**
 * Target 3: reading open positions, and closing them.
 *
 * Nothing on chain enumerates a user's positions — they are PDAs keyed by
 * (user_account, market_index, nonce) — so a client scans. That is the same reason opening
 * has to search for a free nonce.
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
  getClosePositionInstruction,
  getDecreasePositionInstruction,
  getPositionDecoder,
  type Position,
} from "@solfx/client";
import type {
  Address,
  Instruction,
  Rpc,
  SolanaRpcApi,
  TransactionSigner,
} from "@solana/kit";

const BPS = 10_000n;
const NOTIONAL_DIVISOR = 1_000_000_000_000n;

export const CLOSE_POSITION_CU = 120_000;

/**
 * `decrease_position` does the same work as a close plus the book-keeping to keep a smaller
 * position alive, so it is budgeted the same. `docs/compute-budget.md` measures the close at
 * 51,724 against a 120,000 ceiling.
 */
export const DECREASE_POSITION_CU = 120_000;

export type OpenPosition = {
  readonly address: Address;
  readonly marketIndex: number;
  readonly nonce: number;
  readonly data: Position;
  /** Notional at the current oracle, in quote units. */
  readonly notionalNow: bigint;
  /** Unrealised PnL in quote units, sign from the trader's side. */
  readonly unrealised: bigint;
};

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * Every open position for a user across the given markets.
 *
 * `maxNonce` mirrors the CLI's scan depth: a trader may hold several positions on one market
 * distinguished only by nonce, and eight is well past the practical limit while costing only
 * a handful of cheap reads.
 */
export async function loadPositions(
  rpc: Rpc<SolanaRpcApi>,
  owner: Address,
  marketIndexes: readonly number[],
  prices: Record<number, bigint | undefined>,
  maxNonce = 8
): Promise<OpenPosition[]> {
  const [userAccount] = await findUserAccountPda({ authority: owner });

  const wanted: { address: Address; marketIndex: number; nonce: number }[] = [];
  for (const marketIndex of marketIndexes) {
    for (let nonce = 0; nonce < maxNonce; nonce++) {
      const [address] = await findPositionPda({
        userAccount,
        marketIndex,
        nonce,
      });
      wanted.push({ address, marketIndex, nonce });
    }
  }

  const out: OpenPosition[] = [];
  // getMultipleAccounts caps at 100 keys per call.
  for (let i = 0; i < wanted.length; i += 100) {
    const chunk = wanted.slice(i, i + 100);
    const { value } = await rpc
      .getMultipleAccounts(
        chunk.map((w) => w.address),
        { encoding: "base64" }
      )
      .send();
    value.forEach((raw, j) => {
      const w = chunk[j];
      if (!raw || !w) return;
      const data = getPositionDecoder().decode(base64ToBytes(raw.data[0]));
      if (data.sizeBase === 0n) return;
      const price = prices[w.marketIndex];
      const notionalNow =
        price === undefined ? 0n : (data.sizeBase * price) / NOTIONAL_DIVISOR;
      const unrealised =
        price === undefined
          ? 0n
          : data.direction === Direction.Long
            ? notionalNow - data.entryNotional
            : data.entryNotional - notionalNow;
      out.push({
        address: w.address,
        marketIndex: w.marketIndex,
        nonce: w.nonce,
        data,
        notionalNow,
        unrealised,
      });
    });
  }
  return out;
}

/**
 * Re-price positions against the latest oracle, without re-reading the chain.
 *
 * `loadPositions` fixes `notionalNow` and `unrealised` at the prices that were current when
 * it ran, and it runs only when the wallet or the market set changes. Left there, unrealised
 * P&L freezes at whatever the price was on connect — the one number on the row a trader
 * watches move. Only the marks change here; the position data itself still comes from chain.
 */
export function repricePositions(
  positions: readonly OpenPosition[],
  prices: Record<number, bigint | undefined>
): OpenPosition[] {
  return positions.map((p) => {
    const price = prices[p.marketIndex];
    if (price === undefined) return p;
    const notionalNow = (p.data.sizeBase * price) / NOTIONAL_DIVISOR;
    const unrealised =
      p.data.direction === Direction.Long
        ? notionalNow - p.data.entryNotional
        : p.data.entryNotional - notionalNow;
    return { ...p, notionalNow, unrealised };
  });
}

/**
 * The bound for closing, which is the mirror of opening.
 *
 * Closing a **long** is a sell, so the limit is a **minimum**; closing a **short** is a buy,
 * so it is a **maximum**. Getting this backwards produces a bound the fill can never satisfy
 * and every close fails with SlippageExceeded.
 */
export function closePriceLimit(
  direction: Direction,
  price: bigint,
  slippageBps: number
): bigint {
  const delta = (price * BigInt(slippageBps)) / BPS;
  return direction === Direction.Long ? price - delta : price + delta;
}

export async function buildClosePosition(p: {
  signer: TransactionSigner;
  marketIndex: number;
  nonce: number;
  direction: Direction;
  price: bigint;
  slippageBps: number;
  priceUpdate: Address;
}): Promise<Instruction> {
  const [protocol] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({
    authority: p.signer.address,
  });
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

  return getClosePositionInstruction({
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
    priceLimit: closePriceLimit(p.direction, p.price, p.slippageBps),
  });
}

/**
 * Close part of a position.
 *
 * Built from the same derivations as `buildClosePosition`: the two instructions take the same
 * accounts in the same order, so two copies would only drift apart. They are not quite
 * identical, and the difference is instructive — `close_position` takes the authority
 * *writable* because it closes the position account and refunds the rent to them, while this
 * one closes nothing and leaves the authority read-only. The generated builders set that;
 * `reduce.test.ts` asserts it.
 *
 * **The caller must not send this for a full close.** `decrease_position` requires
 * `size_delta < position.size_base` strictly and rejects an equal size with
 * `ReductionExceedsSize`; `close_position` is the instruction for the whole thing and it
 * reclaims the position account's rent as well. `reduceIsFullClose` in `@solfx/client` is
 * that boundary.
 */
export async function buildDecreasePosition(p: {
  signer: TransactionSigner;
  marketIndex: number;
  nonce: number;
  direction: Direction;
  price: bigint;
  slippageBps: number;
  priceUpdate: Address;
  sizeDelta: bigint;
}): Promise<Instruction> {
  const [protocol] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({
    authority: p.signer.address,
  });
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

  return getDecreasePositionInstruction({
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
    sizeDelta: p.sizeDelta,
    priceLimit: closePriceLimit(p.direction, p.price, p.slippageBps),
  });
}
