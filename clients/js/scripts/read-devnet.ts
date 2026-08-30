// Proves the generated client against the live program: derive every PDA with the
// generated helpers, fetch, decode, and print. Nothing here is hand-derived.
import { createSolanaRpc } from "@solana/kit";
import { fetchProtocol } from "../src/generated/accounts/protocol.js";
import { fetchMarket } from "../src/generated/accounts/market.js";
import { findProtocolPda } from "../src/generated/pdas/protocol.js";
import { findMarketPda } from "../src/generated/pdas/market.js";

const rpcUrl = process.env.SOLFX_RPC_URL;
if (!rpcUrl) throw new Error("SOLFX_RPC_URL is not set");
const rpc = createSolanaRpc(rpcUrl);

const [protocolPda] = await findProtocolPda();
const protocol = await fetchProtocol(rpc, protocolPda);
console.log(`protocol   ${protocolPda}`);
console.log(`admin      ${protocol.data.admin}`);
console.log(`markets    ${protocol.data.numMarkets}`);
console.log(`paused     ${protocol.data.paused}`);
console.log();

const decoder = new TextDecoder();
for (let i = 0; i < protocol.data.numMarkets; i++) {
  const [pda] = await findMarketPda({ marketIndex: i });
  const m = await fetchMarket(rpc, pda);
  const symbol = decoder.decode(Uint8Array.from(m.data.symbol)).replace(/\0+$/, "");
  const feed = Buffer.from(m.data.pythFeedId).toString("hex");
  console.log(
    `  [${i}] ${symbol.padEnd(8)} status=${m.data.status} lev=${m.data.maxLeverage}x`,
  );
  console.log(`      ${pda}`);
  console.log(`      feed ${feed}`);
}
