// Builds the open_position instruction the ticket would send and simulates it on devnet.
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
  buildOpenPosition,
  firstFreeNonce,
  priceLimitFor,
  OPEN_POSITION_CU,
} from "../src/lib/trade.js";
import { loadMarkets } from "../src/lib/markets.js";
import { readPrice } from "../src/lib/prices.js";
import { readAccountStatus } from "../src/lib/account.js";

async function main() {
  const rpc = createSolanaRpc(
    process.env.SOLFX_RPC_URL!
  ) as unknown as Rpc<SolanaRpcApi>;
  const owner = "7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs" as Address;
  const signer = createNoopSigner(owner);
  const want = process.argv[2] ?? "BTC/USD";

  const map = new PriceAccountMap(
    JSON.parse(readFileSync("public/price-accounts.json", "utf8"))
  );
  const markets = await loadMarkets(rpc);
  const market = markets.find((m) => m.symbol === want && m.tradeable);
  if (!market) throw new Error(`${want} not tradeable`);

  const priceAccount = map.requireFeed(market.feedIdHex, market.symbol);
  const price = await readPrice(rpc, priceAccount);
  if (!price) throw new Error("no price");

  const status = await readAccountStatus(rpc, owner);
  const nonce = await firstFreeNonce(rpc, status.userAccountPda, market.index);
  if (nonce === undefined) throw new Error("no free nonce");

  const sizeBase = 12_800_000n; // ~$1,000 of BTC
  const collateral = 200_000_000n; // $200
  const slippageBps = 100;

  console.log(`${market.symbol} (market ${market.index}, ${market.status})`);
  console.log(
    `  oracle       ${price.price}  age ${price.ageSeconds}s${price.stale ? "  STALE" : ""}`
  );
  console.log(
    `  price limit  ${priceLimitFor(Direction.Long, price.price, slippageBps)}  (${slippageBps} bps)`
  );
  console.log(`  nonce        ${nonce}`);

  const ix = await buildOpenPosition({
    signer,
    marketIndex: market.index,
    direction: Direction.Long,
    sizeBase,
    collateral,
    price: price.price,
    slippageBps,
    priceUpdate: priceAccount,
    nonce,
  });
  console.log(`  accounts     ${ix.accounts?.length}`);

  const { value: blockhash } = await rpc.getLatestBlockhash().send();
  const msg = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(signer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
    (m) => appendTransactionMessageInstructions([ix], m)
  );
  const wire = getBase64EncodedWireTransaction(compileTransaction(msg));
  const sim = await rpc
    .simulateTransaction(wire, {
      encoding: "base64",
      sigVerify: false,
      replaceRecentBlockhash: true,
    })
    .send();

  const j = (v: unknown) =>
    JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x));
  console.log(`\nerr    : ${j(sim.value.err)}`);
  console.log(
    `units  : ${sim.value.unitsConsumed} (requested ${OPEN_POSITION_CU})`
  );
  for (const l of sim.value.logs ?? []) console.log(`  ${l}`);
  const events = parseEvents(sim.value.logs ?? []);
  if (events.length) {
    console.log("\ndecoded events:");
    for (const e of events) {
      console.log(`  ${e.name}`);
      for (const [k, v] of Object.entries(e.data))
        console.log(`    ${k.padEnd(20)} ${v}`);
    }
  }
}
void main();
