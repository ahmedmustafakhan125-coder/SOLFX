/**
 * Reading and moving liquidity. The counterpart to `account.ts`, for the other side of the
 * book: a trader deposits collateral to take risk, an LP deposits capital to carry it.
 *
 * # Why withdrawing is three instructions and not one
 *
 * `request_remove_liquidity` → wait out the cooldown → `remove_liquidity`, with
 * `cancel_remove_liquidity` as the way back. That is threat T6 (§ 8.1): without the delay an
 * LP deposits ahead of a known trader loss and withdraws ahead of a known gain, harvesting
 * the pool's edge without ever carrying its risk. The page therefore has a *state*, not a
 * button — and the state is the withdraw-request PDA, which either exists or does not.
 *
 * Every preview number comes from `@solfx/client`'s `lp.ts`, which is a port of the engine's
 * own arithmetic. Nothing is computed locally.
 */
import {
  fetchLpPool,
  fetchProtocol,
  findFeeVaultPda,
  findLpMintPda,
  findLpPoolPda,
  findLpVaultPda,
  findProtocolPda,
  findWithdrawRequestPda,
  getAddLiquidityInstruction,
  getCancelRemoveLiquidityInstruction,
  getLpWithdrawRequestDecoder,
  getRemoveLiquidityInstruction,
  getRequestRemoveLiquidityInstruction,
  type PoolState,
} from "@solfx/client";
import {
  TOKEN_PROGRAM_ADDRESS,
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstructionAsync,
} from "@solana-program/token";
import type {
  Address,
  Instruction,
  Rpc,
  SolanaRpcApi,
  TransactionSigner,
} from "@solana/kit";

/** An outstanding withdrawal, or `null` when the LP has none. */
export type PendingWithdrawal = {
  readonly shares: bigint;
  readonly requestedAt: bigint;
  readonly unlockAt: bigint;
};

/** The pool figures a page displays but no preview needs. */
export type PoolFacts = {
  readonly withdrawalCooldownSeconds: number;
  readonly pendingWithdrawalShares: bigint;
  readonly totalDeposited: bigint;
  readonly totalWithdrawn: bigint;
  readonly totalExitFees: bigint;
  readonly totalPerformanceFees: bigint;
};

export type PoolStatus = {
  readonly pool: PoolState;
  readonly facts: PoolFacts;
  readonly usdcMint: Address;
  readonly lpMint: Address;
  /** The LP's `slpUSD` balance. Zero both when they hold none and when the account is absent. */
  readonly shares: bigint;
  /** Whether the `slpUSD` token account exists, which decides if a deposit must create it. */
  readonly hasLpAccount: boolean;
  /** The LP's own USDC, i.e. what they could deposit. */
  readonly walletUsdc: bigint;
  readonly hasUsdcAccount: boolean;
  readonly pendingWithdrawal: PendingWithdrawal | null;
  /** Chain time, so a cooldown countdown is measured against the clock the program uses. */
  readonly nowSeconds: bigint;
};

/**
 * Everything the pool page shows, in one round trip after the two PDA reads.
 *
 * `owner` may be `null` for a disconnected visitor: the pool's own figures are public and
 * worth showing before anyone connects a wallet.
 */
export async function readPoolStatus(
  rpc: Rpc<SolanaRpcApi>,
  owner: Address | null
): Promise<PoolStatus> {
  const [protocolPda] = await findProtocolPda();
  const [lpPoolPda] = await findLpPoolPda();
  const [protocol, lpPool] = await Promise.all([
    fetchProtocol(rpc, protocolPda),
    fetchLpPool(rpc, lpPoolPda),
  ]);

  const usdcMint = protocol.data.usdcMint;
  const lpMint = lpPool.data.lpMint;
  const pool: PoolState = {
    aum: lpPool.data.aum,
    lpTokenSupply: lpPool.data.lpTokenSupply,
    exitFeeBps: lpPool.data.exitFeeBps,
    performanceFeeBps: lpPool.data.performanceFeeBps,
    highWaterMarkPerShare: lpPool.data.highWaterMarkPerShare,
  };
  const facts: PoolFacts = {
    withdrawalCooldownSeconds: lpPool.data.withdrawalCooldownSeconds,
    pendingWithdrawalShares: lpPool.data.pendingWithdrawalShares,
    totalDeposited: lpPool.data.totalDeposited,
    totalWithdrawn: lpPool.data.totalWithdrawn,
    totalExitFees: lpPool.data.totalExitFees,
    totalPerformanceFees: lpPool.data.totalPerformanceFees,
  };

  // The chain's clock, not the browser's. A cooldown compared against a laptop whose clock
  // drifts is a countdown that hits zero before the program agrees it has.
  const nowSeconds = BigInt(
    (await rpc.getBlockTime(await rpc.getSlot().send()).send()) ?? 0
  );

  if (!owner) {
    return {
      pool,
      facts,
      usdcMint,
      lpMint,
      shares: 0n,
      hasLpAccount: false,
      walletUsdc: 0n,
      hasUsdcAccount: false,
      pendingWithdrawal: null,
      nowSeconds,
    };
  }

  const [[lpAta], [usdcAta], [withdrawRequest]] = await Promise.all([
    findAssociatedTokenPda({
      mint: lpMint,
      owner,
      tokenProgram: TOKEN_PROGRAM_ADDRESS,
    }),
    findAssociatedTokenPda({
      mint: usdcMint,
      owner,
      tokenProgram: TOKEN_PROGRAM_ADDRESS,
    }),
    findWithdrawRequestPda({ provider: owner }),
  ]);

  // All three may legitimately be absent, so read raw rather than through decoders that
  // throw on a missing account — "you have never provided liquidity" is not an error.
  const { value: accounts } = await rpc
    .getMultipleAccounts([lpAta, usdcAta, withdrawRequest], {
      encoding: "base64",
    })
    .send();

  const [lpRaw, usdcRaw, requestRaw] = accounts;

  let pendingWithdrawal: PendingWithdrawal | null = null;
  if (requestRaw) {
    const req = getLpWithdrawRequestDecoder().decode(
      base64ToBytes(requestRaw.data[0])
    );
    pendingWithdrawal = {
      shares: req.shares,
      requestedAt: req.requestedAt,
      unlockAt: req.unlockAt,
    };
  }

  return {
    pool,
    facts,
    usdcMint,
    lpMint,
    shares: lpRaw ? readTokenAmount(lpRaw.data[0]) : 0n,
    hasLpAccount: lpRaw !== null,
    walletUsdc: usdcRaw ? readTokenAmount(usdcRaw.data[0]) : 0n,
    hasUsdcAccount: usdcRaw !== null,
    pendingWithdrawal,
    nowSeconds,
  };
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** SPL token account: `amount` is a u64 at offset 64. Same read as `account.ts` uses. */
function readTokenAmount(b64: string): bigint {
  const bytes = base64ToBytes(b64);
  let v = 0n;
  for (let i = 7; i >= 0; i--) v = (v << 8n) | BigInt(bytes[64 + i] ?? 0);
  return v;
}

/**
 * Put `amount` of USDC into the pool.
 *
 * `minLpOut` is a real bound, not a formality: NAV per share can move between the page
 * reading it and the transaction landing, and `add_liquidity` compares. Callers pass a
 * preview-derived floor; there is no "disabled" value.
 *
 * The `slpUSD` account is created idempotently when absent, exactly as `buildDeposit` does
 * for USDC — a first-time LP has no account for a mint they have never held.
 */
export async function buildAddLiquidity(
  signer: TransactionSigner,
  status: Pick<PoolStatus, "usdcMint" | "lpMint" | "hasLpAccount">,
  amount: bigint,
  minLpOut: bigint
): Promise<Instruction[]> {
  const [protocolPda] = await findProtocolPda();
  const [lpPool] = await findLpPoolPda();
  const [lpVault] = await findLpVaultPda();
  const [lpMint] = await findLpMintPda();
  const [providerTokenAccount] = await findAssociatedTokenPda({
    mint: status.usdcMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });
  const [providerLpAccount] = await findAssociatedTokenPda({
    mint: status.lpMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });

  const ixs: Instruction[] = [];
  if (!status.hasLpAccount) {
    ixs.push(
      await getCreateAssociatedTokenIdempotentInstructionAsync({
        payer: signer,
        mint: status.lpMint,
        owner: signer.address,
      })
    );
  }
  ixs.push(
    getAddLiquidityInstruction({
      provider: signer,
      protocol: protocolPda,
      lpPool,
      lpVault,
      lpMint,
      providerTokenAccount,
      providerLpAccount,
      amount,
      minLpOut,
    })
  );
  return ixs;
}

/** Start the cooldown on `shares`. Creates the withdraw-request PDA. */
export async function buildRequestWithdrawal(
  signer: TransactionSigner,
  lpMint: Address,
  shares: bigint
): Promise<Instruction[]> {
  const [protocolPda] = await findProtocolPda();
  const [lpPool] = await findLpPoolPda();
  const [withdrawRequest] = await findWithdrawRequestPda({
    provider: signer.address,
  });
  const [providerLpAccount] = await findAssociatedTokenPda({
    mint: lpMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });

  return [
    getRequestRemoveLiquidityInstruction({
      provider: signer,
      protocol: protocolPda,
      lpPool,
      withdrawRequest,
      providerLpAccount,
      lpAmount: shares,
    }),
  ];
}

/**
 * Settle a matured request.
 *
 * No client-side cooldown check. The program owns that rule and re-derives it from its own
 * clock; a page that refuses early would only ever disagree with the chain about *when*,
 * and a page that allows early gets a clean refusal. Send it and surface the answer.
 */
export async function buildRemoveLiquidity(
  signer: TransactionSigner,
  status: Pick<PoolStatus, "usdcMint" | "lpMint">,
  minUsdcOut: bigint
): Promise<Instruction[]> {
  const [protocolPda] = await findProtocolPda();
  const [lpPool] = await findLpPoolPda();
  const [lpVault] = await findLpVaultPda();
  const [lpMint] = await findLpMintPda();
  const [feeVault] = await findFeeVaultPda();
  const [withdrawRequest] = await findWithdrawRequestPda({
    provider: signer.address,
  });
  const [providerTokenAccount] = await findAssociatedTokenPda({
    mint: status.usdcMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });
  const [providerLpAccount] = await findAssociatedTokenPda({
    mint: status.lpMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });

  return [
    getRemoveLiquidityInstruction({
      provider: signer,
      protocol: protocolPda,
      lpPool,
      lpVault,
      lpMint,
      feeVault,
      withdrawRequest,
      providerTokenAccount,
      providerLpAccount,
      minUsdcOut,
    }),
  ];
}

/** Abandon a pending request and keep the shares. Returns the PDA's rent. */
export async function buildCancelWithdrawal(
  signer: TransactionSigner
): Promise<Instruction[]> {
  const [lpPool] = await findLpPoolPda();
  const [withdrawRequest] = await findWithdrawRequestPda({
    provider: signer.address,
  });

  return [
    getCancelRemoveLiquidityInstruction({
      provider: signer,
      lpPool,
      withdrawRequest,
      authority: signer.address,
    }),
  ];
}
