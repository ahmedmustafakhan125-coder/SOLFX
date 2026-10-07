/**
 * Fill the faucet: mint test USDC to it and send it devnet SOL.
 *
 * Run by the operator, on their own machine, with the **mint authority** — the one key in this
 * flow that must never be on a server. `scripts/fund-faucet.sh` is the wrapper; see it for usage.
 * The faucet service only ever transfers what this put there.
 *
 * `--simulate-as <pubkey>` builds the same transaction with a placeholder signer for that address
 * and simulates it without signature checks. It needs no key, so the instructions can be proven
 * against the live mint before anyone holding the key runs it for real.
 */
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import {
  address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createNoopSigner,
  createSolanaRpc,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  getSignatureFromTransaction,
  partiallySignTransactionMessageWithSigners,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
} from "@solana/kit";
import {
  TOKEN_PROGRAM_ADDRESS,
  fetchMint,
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstructionAsync,
  getMintToInstruction,
} from "@solana-program/token";
import { getTransferSolInstruction } from "@solana-program/system";

import { formatUnits, parseUnits, validateWallet } from "./lib.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

const { values: args } = parseArgs({
  options: {
    faucet: { type: "string" },
    usdc: { type: "string", default: "10000000" },
    sol: { type: "string", default: "10" },
    authority: { type: "string", default: join(homedir(), ".config", "solana", "id.json") },
    mint: { type: "string" },
    rpc: { type: "string" },
    "simulate-as": { type: "string" },
  },
});

function fail(message) {
  console.error(`error: ${message}`);
  process.exit(1);
}

const faucet = validateWallet(args.faucet ?? "");
if (!faucet.ok) fail(`--faucet <pubkey> is required and must be a wallet address (${faucet.error})`);

const rpcUrl = args.rpc ?? process.env.SOLFX_RPC_URL;
if (!rpcUrl) fail("no RPC: pass --rpc or `set -a && . ./.env && set +a` for SOLFX_RPC_URL");

let mintText = args.mint ?? process.env.SOLFX_USDC_MINT;
if (!mintText) {
  try {
    mintText = JSON.parse(readFileSync(join(repoRoot, "deployment.json"), "utf8")).usdc_mint;
  } catch {
    fail("no mint: pass --mint, set SOLFX_USDC_MINT, or run from a checkout with deployment.json");
  }
}

const rpc = createSolanaRpc(rpcUrl);
const mint = address(mintText);
const owner = address(faucet.wallet);
const simulateAs = args["simulate-as"];
const authority = simulateAs
  ? createNoopSigner(address(simulateAs))
  : await createKeyPairSignerFromBytes(
      Uint8Array.from(JSON.parse(readFileSync(args.authority, "utf8"))),
    );

const { data: mintData } = await fetchMint(rpc, mint);
const usdc = parseUnits(args.usdc, mintData.decimals);
const sol = parseUnits(args.sol, 9);

// Said plainly before anything is signed: the commonest mistake is the wrong keypair, and the
// Token Program's own refusal for it is an opaque custom error code.
const mintAuthority = mintData.mintAuthority.__option === "Some" ? mintData.mintAuthority.value : null;
if (mintAuthority !== authority.address) {
  fail(
    `${authority.address} is not this mint's authority (${mintAuthority ?? "none — minting is disabled"}). ` +
      "Use --authority with the mint authority's keypair.",
  );
}

const [faucetAta] = await findAssociatedTokenPda({ mint, owner, tokenProgram: TOKEN_PROGRAM_ADDRESS });
const instructions = [
  await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: authority, owner, mint }),
  getMintToInstruction({ mint, token: faucetAta, mintAuthority: authority, amount: usdc }),
];
if (sol > 0n) {
  instructions.push(getTransferSolInstruction({ source: authority, destination: owner, amount: sol }));
}

const { value: blockhash } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
const message = pipe(
  createTransactionMessage({ version: 0 }),
  (m) => setTransactionMessageFeePayerSigner(authority, m),
  (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
  (m) => appendTransactionMessageInstructions(instructions, m),
);

console.log(`faucet        ${owner}`);
console.log(`token account ${faucetAta}`);
console.log(`mint          ${mint} (${mintData.decimals} dp), authority ${authority.address}`);
console.log(`sending       ${formatUnits(usdc, mintData.decimals)} USDC + ${formatUnits(sol, 9)} SOL`);

if (simulateAs) {
  const tx = await partiallySignTransactionMessageWithSigners(message);
  const { value } = await rpc
    .simulateTransaction(getBase64EncodedWireTransaction(tx), {
      encoding: "base64",
      sigVerify: false,
      replaceRecentBlockhash: true,
    })
    .send();
  console.log(value.err ? `simulation FAILED: ${JSON.stringify(value.err)}` : "simulation OK");
  for (const l of value.logs ?? []) console.log(`  ${l}`);
  process.exit(value.err ? 1 : 0);
}

const tx = await signTransactionMessageWithSigners(message);
const signature = getSignatureFromTransaction(tx);
await rpc
  .sendTransaction(getBase64EncodedWireTransaction(tx), { encoding: "base64", preflightCommitment: "confirmed" })
  .send();
for (let i = 0; i < 80; i++) {
  await new Promise((r) => setTimeout(r, 750));
  const { value } = await rpc.getSignatureStatuses([signature]).send();
  if (value[0]?.err) fail(`transaction failed: ${JSON.stringify(value[0].err)} (${signature})`);
  if (value[0]?.confirmationStatus === "confirmed" || value[0]?.confirmationStatus === "finalized") {
    console.log(`confirmed     ${signature}`);
    console.log(`explorer      https://explorer.solana.com/tx/${signature}?cluster=devnet`);
    process.exit(0);
  }
}
fail(`not confirmed after 60 s; check https://explorer.solana.com/tx/${signature}?cluster=devnet`);
