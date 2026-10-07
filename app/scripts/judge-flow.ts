/**
 * The first-time user's whole path on devnet, from a brand-new wallet to a closed trade, using
 * the app's own instruction builders against the live site's own endpoints:
 *
 *   POST /faucet  ->  init account + deposit  ->  open BTC/USD  ->  stop-loss  ->  cancel, close
 *
 * Every read and send goes through `https://solfx.cloud/rpc`, the proxy a browser uses, so a
 * method missing from its allowlist fails here as it would for a judge. The one thing this does
 * not exercise is Phantom itself: the signer is a fresh in-memory keypair.
 *
 *   clients/js/node_modules/.bin/tsx --tsconfig app/tsconfig.json app/scripts/judge-flow.ts
 *   SOLFX_SITE=http://127.0.0.1:8787 ...   # against a local server.mjs instead
 */
import {
  appendTransactionMessageInstructions,
  createSolanaRpc,
  createTransactionMessage,
  generateKeyPairSigner,
  getBase64EncodedWireTransaction,
  getSignatureFromTransaction,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";
import {
  Direction,
  PriceAccountMap,
  TriggerKind,
  findPositionPda,
  findTriggerOrderPda,
  findUserAccountPda,
  type PriceAccountEntry,
} from "@solfx/client";

import {
  buildDeposit,
  buildInitUserAccount,
  readAccountStatus,
} from "@/lib/account";
import { loadMarkets } from "@/lib/markets";
import { buildClosePosition } from "@/lib/positions";
import { readPrice } from "@/lib/prices";
import { buildOpenPosition, firstFreeNonce } from "@/lib/trade";
import {
  buildCancelTrigger,
  buildPlaceTrigger,
  firstFreeOrderId,
} from "@/lib/triggers";

const SITE = process.env.SOLFX_SITE ?? "https://solfx.cloud";
const rpc = createSolanaRpc(`${SITE}/rpc`);
const QUOTE = 1_000_000n;
const NOTIONAL_DIVISOR = 1_000_000_000_000n;
const steps: [string, string][] = [];

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Sign, send and confirm. No compute-budget instruction: every step here is well under the
 * 200k default per instruction (`open_position` measures ~60k), so the browser's explicit
 * limits are a margin this script does not need.
 */
async function send(
  signer: TransactionSigner,
  label: string,
  ixs: Instruction[]
): Promise<string> {
  const { value: blockhash } = await rpc.getLatestBlockhash().send();
  const tx = await signTransactionMessageWithSigners(
    pipe(
      createTransactionMessage({ version: 0 }),
      (m) => setTransactionMessageFeePayerSigner(signer, m),
      (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
      (m) => appendTransactionMessageInstructions(ixs, m)
    )
  );
  const sig = getSignatureFromTransaction(tx);
  await rpc
    .sendTransaction(getBase64EncodedWireTransaction(tx), {
      encoding: "base64",
      preflightCommitment: "confirmed",
    })
    .send();
  for (let i = 0; i < 80; i++) {
    await sleep(750);
    const { value } = await rpc.getSignatureStatuses([sig]).send();
    if (value[0]?.err)
      throw new Error(`${label} failed: ${JSON.stringify(value[0].err)}`);
    if (
      value[0]?.confirmationStatus === "confirmed" ||
      value[0]?.confirmationStatus === "finalized"
    ) {
      steps.push([label, sig]);
      console.log(`  ok  ${label.padEnd(26)} ${sig}`);
      return sig;
    }
  }
  throw new Error(`${label}: not confirmed in 60 s (${sig})`);
}

async function claim(wallet: string) {
  const r = await fetch(`${SITE}/faucet`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ wallet }),
  });
  return {
    status: r.status,
    body: (await r.json()) as Record<string, unknown>,
  };
}

const fmt = (v: bigint) =>
  `${v / QUOTE}.${(v % QUOTE).toString().padStart(6, "0")}`;

async function main() {
  const signer = await generateKeyPairSigner();
  console.log(`fresh wallet ${signer.address}\n`);

  // 1. Faucet.
  const first = await claim(signer.address);
  if (first.status !== 200 && first.status !== 202) {
    throw new Error(
      `faucet: HTTP ${first.status} ${JSON.stringify(first.body)}`
    );
  }
  steps.push(["faucet claim", String(first.body.signature)]);
  console.log(
    `  ok  ${"faucet claim".padEnd(26)} ${String(first.body.signature)}  (+${String(first.body.usdc)} USDC, +${String(first.body.sol)} SOL)`
  );
  const second = await claim(signer.address);
  console.log(
    `      second claim refused: HTTP ${second.status} "${String(second.body.error)}"`
  );
  if (second.status !== 429) throw new Error("a second claim was not refused");

  let status = await readAccountStatus(rpc, signer.address);
  for (let i = 0; i < 20 && status.walletUsdc === 0n; i++) {
    await sleep(1000);
    status = await readAccountStatus(rpc, signer.address);
  }
  console.log(`      wallet holds ${fmt(status.walletUsdc)} test USDC`);

  // 2. SolFX account + deposit, as the account panel does it.
  const deposit = 1_000n * QUOTE;
  await send(signer, "create SolFX account", [
    await buildInitUserAccount(signer),
  ]);
  await send(
    signer,
    "deposit 1,000 USDC",
    await buildDeposit(signer, status.usdcMint, deposit, !status.hasAta)
  );

  // 3. Open BTC/USD — the one market open around the clock.
  const markets = await loadMarkets(rpc);
  const btc = markets.find((m) => m.symbol === "BTC/USD");
  if (!btc?.tradeable)
    throw new Error(`BTC/USD is not tradeable (${btc?.status ?? "missing"})`);
  const mapRes = await fetch(`${SITE}/price-accounts.json`);
  const priceMap = new PriceAccountMap(
    (await mapRes.json()) as PriceAccountEntry[]
  );
  const priceUpdate = priceMap.forFeed(btc.feedIdHex);
  if (!priceUpdate) throw new Error("no price account for BTC/USD");
  const live = await readPrice(rpc, priceUpdate);
  if (!live || live.stale)
    throw new Error(`BTC/USD price is stale (${live?.ageSeconds}s)`);

  // $200 notional, $50 margin: size = notional * 1e12 / price, at least the market minimum.
  let sizeBase = (200n * QUOTE * NOTIONAL_DIVISOR) / live.price;
  if (sizeBase < btc.data.minPositionSize) sizeBase = btc.data.minPositionSize;
  const [userAccount] = await findUserAccountPda({ authority: signer.address });
  const nonce = await firstFreeNonce(rpc, userAccount, btc.index);
  if (nonce === undefined) throw new Error("no free position nonce");
  await send(signer, "open BTC/USD long", [
    await buildOpenPosition({
      signer,
      marketIndex: btc.index,
      direction: Direction.Long,
      sizeBase,
      collateral: 50n * QUOTE,
      price: live.price,
      slippageBps: 100,
      priceUpdate,
      nonce,
    }),
  ]);

  // 4. A stop 3% below, as the trigger panel places it.
  const [position] = await findPositionPda({
    userAccount,
    marketIndex: btc.index,
    nonce,
  });
  const orderId = await firstFreeOrderId(rpc, position);
  if (orderId === undefined) throw new Error("no free order id");
  await send(signer, "stop-loss 3% below", [
    await buildPlaceTrigger({
      signer,
      marketIndex: btc.index,
      position,
      orderId,
      kind: TriggerKind.StopLoss,
      triggerPrice: (live.price * 97n) / 100n,
      sizeBase,
      priceUpdate,
    }),
  ]);

  // 5. Close. The stop is cancelled first: close_position does not cancel triggers, and a
  // stop left behind would re-attach to the next position opened at this nonce.
  const [triggerOrder] = await findTriggerOrderPda({ position, orderId });
  await send(signer, "cancel the stop", [
    buildCancelTrigger(signer, triggerOrder),
  ]);
  const now = await readPrice(rpc, priceUpdate);
  await send(signer, "close BTC/USD", [
    await buildClosePosition({
      signer,
      marketIndex: btc.index,
      nonce,
      direction: Direction.Long,
      price: now?.price ?? live.price,
      slippageBps: 100,
      priceUpdate,
    }),
  ]);

  const end = await readAccountStatus(rpc, signer.address);
  console.log(
    `\n      free collateral ${fmt(end.freeCollateral)} of ${fmt(deposit)} deposited ` +
      `(round trip cost ${fmt(deposit - end.freeCollateral)}: spread and fees)`
  );
  console.log("\n| Step | Signature |\n|---|---|");
  for (const [label, sig] of steps) {
    console.log(
      `| ${label} | [\`${sig.slice(0, 16)}…\`](https://explorer.solana.com/tx/${sig}?cluster=devnet) |`
    );
  }
}

main().catch((e: unknown) => {
  console.error(`\nFAILED: ${e instanceof Error ? e.message : String(e)}`);
  process.exit(1);
});
