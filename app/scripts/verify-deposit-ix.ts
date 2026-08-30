// Builds the deposit instruction the AccountPanel would send and simulates it against
// devnet. A noop signer is enough: simulation with sigVerify off exercises every account
// meta and the program's own validation without anyone signing anything.
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
import { buildDeposit, readAccountStatus } from "../src/lib/account.js";

async function main() {
  const rpc = createSolanaRpc(process.env.SOLFX_RPC_URL!) as unknown as Rpc<SolanaRpcApi>;
  const owner = "7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs" as Address;
  const signer = createNoopSigner(owner);

  const status = await readAccountStatus(rpc, owner);
  const amount = 1_000_000n; // $1
  const ixs = await buildDeposit(signer, status.usdcMint, amount, !status.hasAta);

  console.log(`instructions: ${ixs.length}`);
  for (const ix of ixs) {
    console.log(`  program ${ix.programAddress}  accounts=${ix.accounts?.length ?? 0}`);
  }

  const { value: blockhash } = await rpc.getLatestBlockhash().send();
  const msg = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(signer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
    (m) => appendTransactionMessageInstructions(ixs, m),
  );
  const wire = getBase64EncodedWireTransaction(compileTransaction(msg));

  const sim = await rpc
    .simulateTransaction(wire, { encoding: "base64", sigVerify: false, replaceRecentBlockhash: true })
    .send();

  console.log(`\nsimulation err : ${JSON.stringify(sim.value.err)}`);
  console.log(`units consumed : ${sim.value.unitsConsumed}`);
  console.log("logs:");
  for (const l of sim.value.logs ?? []) console.log(`  ${l}`);
}
void main();
