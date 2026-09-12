// Reads the live position and simulates closing it, decoding the resulting events.
import {
  appendTransactionMessageInstructions,
  compileTransaction,
  createNoopSigner,
  createSolanaRpc,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
} from "@solana/kit";
import type { Address, Rpc, SolanaRpcApi } from "@solana/kit";
import { Direction, PriceAccountMap, parseEvents } from "@solfx/client";
import { readFileSync } from "node:fs";
import {
  buildClosePosition,
  closePriceLimit,
  loadPositions,
} from "../src/lib/positions.js";
import { loadMarkets } from "../src/lib/markets.js";
import { readPrice } from "../src/lib/prices.js";

const usd = (v: bigint) => {
  const n = v < 0n ? -v : v;
  return `${v < 0n ? "-" : ""}${(n / 1_000_000n).toString()}.${(n % 1_000_000n).toString().padStart(6, "0")}`;
};

async function main() {
  const rpc = createSolanaRpc(
    process.env.SOLFX_RPC_URL!
  ) as unknown as Rpc<SolanaRpcApi>;
  const owner = "7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs" as Address;
  const signer = createNoopSigner(owner);

  const map = new PriceAccountMap(
    JSON.parse(readFileSync("public/price-accounts.json", "utf8"))
  );
  const markets = await loadMarkets(rpc);

  const prices: Record<number, bigint | undefined> = {};
  const accounts: Record<number, Address | undefined> = {};
  for (const m of markets) {
    const acct = map.forFeed(m.feedIdHex);
    accounts[m.index] = acct;
    if (acct) prices[m.index] = (await readPrice(rpc, acct))?.price;
  }

  const positions = await loadPositions(
    rpc,
    owner,
    markets.map((m) => m.index),
    prices
  );
  console.log(`open positions: ${positions.length}`);
  for (const p of positions) {
    const m = markets.find((x) => x.index === p.marketIndex);
    console.log(
      `  ${m?.symbol} nonce ${p.nonce}  ${Direction[p.data.direction]}`
    );
    console.log(`    size        ${p.data.sizeBase}`);
    console.log(`    entry       ${p.data.entryPrice}`);
    console.log(`    collateral  ${usd(p.data.collateral)}`);
    console.log(
      `    notional@   ${usd(p.notionalNow)}   entry ${usd(p.data.entryNotional)}`
    );
    console.log(`    unrealised  ${usd(p.unrealised)}`);
  }
  const pos = positions[0];
  if (!pos) return;

  const price = prices[pos.marketIndex]!;
  const priceUpdate = accounts[pos.marketIndex]!;
  console.log(`\nclosing: oracle ${price}`);
  console.log(
    `  limit  ${closePriceLimit(pos.data.direction, price, 100)}  (100 bps, ${Direction[pos.data.direction] === "Long" ? "minimum — a sell" : "maximum — a buy"})`
  );

  const ix = await buildClosePosition({
    signer,
    marketIndex: pos.marketIndex,
    nonce: pos.nonce,
    direction: pos.data.direction,
    price,
    slippageBps: 100,
    priceUpdate,
  });
  console.log(`  accounts ${ix.accounts?.length}`);

  const { value: blockhash } = await rpc.getLatestBlockhash().send();
  const msg = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(signer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
    (m) => appendTransactionMessageInstructions([ix], m)
  );
  const sim = await rpc
    .simulateTransaction(
      getBase64EncodedWireTransaction(compileTransaction(msg)),
      { encoding: "base64", sigVerify: false, replaceRecentBlockhash: true }
    )
    .send();

  const j = (v: unknown) =>
    JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x));
  console.log(`\nerr   : ${j(sim.value.err)}`);
  console.log(`units : ${sim.value.unitsConsumed}`);
  for (const e of parseEvents(sim.value.logs ?? [])) {
    console.log(`\n  ${e.name}`);
    for (const [k, v] of Object.entries(e.data))
      console.log(`    ${k.padEnd(22)} ${v}`);
  }
}
void main();
