/**
 * The faucet end to end against a local validator: fund it with `fund.mjs`, run the real
 * service behind the real `server.mjs`, claim over HTTP, and read the results off the chain.
 *
 * Not part of `npm test` (no `.test.mjs`), because it needs a validator and two key files:
 *
 *   solana-test-validator --rpc-port 18899 --faucet-port 19900 --reset &
 *   FAUCET_E2E_RPC=http://127.0.0.1:18899 node --test services/faucet/test/localnet.e2e.mjs
 *
 * It creates its own throwaway mint, so it touches nothing on devnet.
 */
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createTransactionMessage,
  generateKeyPairSigner,
  getBase64EncodedWireTransaction,
  getSignatureFromTransaction,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
} from "@solana/kit";
import { getCreateAccountInstruction } from "@solana-program/system";
import {
  TOKEN_PROGRAM_ADDRESS,
  findAssociatedTokenPda,
  getInitializeMint2Instruction,
  getMintSize,
} from "@solana-program/token";

const RPC_URL = process.env.FAUCET_E2E_RPC;
const here = dirname(fileURLToPath(import.meta.url));
const faucetDir = join(here, "..");
const serverPath = join(faucetDir, "..", "api", "server.mjs");
const FAUCET_PORT = 18789;
const SERVER_PORT = 18787;

const rpc = RPC_URL ? createSolanaRpc(RPC_URL) : undefined;
const tmp = mkdtempSync(join(tmpdir(), "faucet-e2e-"));
const children = [];
let mint;
let faucetKeyPath;

function keygen(name) {
  const path = join(tmp, `${name}.json`);
  execFileSync("solana-keygen", ["new", "--no-bip39-passphrase", "--silent", "-o", path]);
  return path;
}
const signerFrom = (path) => createKeyPairSignerFromBytes(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function airdrop(to, lamports) {
  const sig = await rpc.requestAirdrop(to, lamports).send();
  for (let i = 0; i < 60; i++) {
    const { value } = await rpc.getSignatureStatuses([sig]).send();
    if (value[0]?.confirmationStatus === "confirmed" || value[0]?.confirmationStatus === "finalized") return;
    await sleep(250);
  }
  throw new Error("airdrop not confirmed");
}

async function send(payer, instructions) {
  const { value: blockhash } = await rpc.getLatestBlockhash().send();
  const tx = await signTransactionMessageWithSigners(
    pipe(
      createTransactionMessage({ version: 0 }),
      (m) => setTransactionMessageFeePayerSigner(payer, m),
      (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
      (m) => appendTransactionMessageInstructions(instructions, m),
    ),
  );
  const sig = getSignatureFromTransaction(tx);
  await rpc.sendTransaction(getBase64EncodedWireTransaction(tx), { encoding: "base64" }).send();
  for (let i = 0; i < 60; i++) {
    const { value } = await rpc.getSignatureStatuses([sig]).send();
    if (value[0]?.err) throw new Error(JSON.stringify(value[0].err));
    if (value[0]?.confirmationStatus === "confirmed" || value[0]?.confirmationStatus === "finalized") return sig;
    await sleep(250);
  }
  throw new Error("not confirmed");
}

async function usdcOf(owner) {
  const [ata] = await findAssociatedTokenPda({ mint, owner, tokenProgram: TOKEN_PROGRAM_ADDRESS });
  return BigInt((await rpc.getTokenAccountBalance(ata).send()).value.amount);
}

function startFaucet(extraEnv = {}) {
  const child = spawn(process.execPath, [join(faucetDir, "faucet.mjs")], {
    env: {
      ...process.env,
      SOLFX_FAUCET_KEYPAIR: faucetKeyPath,
      SOLFX_USDC_MINT: mint,
      SOLFX_FAUCET_RPC_URL: RPC_URL,
      SOLFX_FAUCET_HOST: "127.0.0.1",
      SOLFX_FAUCET_PORT: String(FAUCET_PORT),
      SOLFX_FAUCET_CLAIMS: join(tmp, "claims.json"),
      SOLFX_FAUCET_USDC: "100",
      SOLFX_FAUCET_PER_IP: "2",
      ...extraEnv,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.push(child);
  let log = "";
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  child.log = () => log;
  return child;
}

async function stop(child) {
  if (child.exitCode !== null) return;
  child.kill("SIGTERM");
  await new Promise((r) => child.once("exit", r));
}

async function waitFor(url) {
  for (let i = 0; i < 80; i++) {
    try {
      await fetch(url);
      return;
    } catch {
      await sleep(250);
    }
  }
  throw new Error(`nothing listening at ${url}`);
}

async function claim(wallet, forwardedFor = "203.0.113.7") {
  const r = await fetch(`http://127.0.0.1:${SERVER_PORT}/faucet`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-for": forwardedFor },
    body: JSON.stringify({ wallet }),
  });
  return { status: r.status, body: await r.json() };
}

before(async () => {
  if (!rpc) return;
  // The mint and its authority: a stand-in for SolFX's test mint and the operator's key.
  const authorityPath = keygen("authority");
  const authority = await signerFrom(authorityPath);
  await airdrop(authority.address, 10_000_000_000n);
  const mintSigner = await generateKeyPairSigner();
  const space = BigInt(getMintSize());
  await send(authority, [
    getCreateAccountInstruction({
      payer: authority,
      newAccount: mintSigner,
      lamports: await rpc.getMinimumBalanceForRentExemption(space).send(),
      space,
      programAddress: TOKEN_PROGRAM_ADDRESS,
    }),
    getInitializeMint2Instruction({ mint: mintSigner.address, decimals: 6, mintAuthority: authority.address }),
  ]);
  mint = mintSigner.address;

  faucetKeyPath = keygen("faucet");
  const faucet = await signerFrom(faucetKeyPath);
  // Funded exactly as the operator would: fund.mjs, signed by the mint authority.
  execFileSync(process.execPath, [
    join(faucetDir, "fund.mjs"),
    "--faucet", faucet.address, "--rpc", RPC_URL, "--mint", mint,
    "--authority", authorityPath, "--usdc", "1000", "--sol", "2",
  ]);

  startFaucet();
  const server = spawn(process.execPath, [serverPath], {
    env: {
      ...process.env,
      PORT: String(SERVER_PORT),
      HOST: "127.0.0.1",
      SOLFX_FAUCET_URL: `http://127.0.0.1:${FAUCET_PORT}`,
      SOLFX_WEB_ROOT: tmp,
    },
    stdio: "ignore",
  });
  children.push(server);
  await waitFor(`http://127.0.0.1:${FAUCET_PORT}/faucet/health`);
  await waitFor(`http://127.0.0.1:${SERVER_PORT}/healthz`);
});

after(async () => {
  for (const c of children) await stop(c);
});

const skip = !RPC_URL && "set FAUCET_E2E_RPC to a local validator";

test("a fresh wallet with no SOL gets test USDC and SOL, in one transaction", { skip }, async () => {
  const w = await generateKeyPairSigner();
  const r = await claim(w.address);
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.usdc, "100");
  assert.equal(r.body.sol, "0.05");
  assert.match(r.body.signature, /^[1-9A-HJ-NP-Za-km-z]{64,88}$/);
  assert.equal(await usdcOf(w.address), 100_000_000n);
  assert.equal((await rpc.getBalance(w.address).send()).value, 50_000_000n);

  const again = await claim(w.address, "198.51.100.9");
  assert.equal(again.status, 429);
  assert.match(again.body.error, /This wallet has already claimed .* Try again in 2[34]h/);
  assert.ok(again.body.retryAfterSeconds > 86_000);
  assert.equal(await usdcOf(w.address), 100_000_000n, "the refusal moved nothing");
});

test("a wallet that already holds SOL gets USDC only", { skip }, async () => {
  const w = await generateKeyPairSigner();
  await airdrop(w.address, 1_000_000_000n);
  const r = await claim(w.address, "198.51.100.20");
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.sol, "0");
  assert.equal((await rpc.getBalance(w.address).send()).value, 1_000_000_000n);
});

test("the per-address limit counts the hop Traefik appended, not a forged one", { skip }, async () => {
  // 203.0.113.7 claimed once in the first test; the limit is 2.
  const a = await generateKeyPairSigner();
  assert.equal((await claim(a.address, "6.6.6.6, 203.0.113.7")).status, 200);
  const b = await generateKeyPairSigner();
  // A client prepending a fresh address of its own does not escape the limit.
  const r = await claim(b.address, "7.7.7.7, 203.0.113.7");
  assert.equal(r.status, 429);
  assert.match(r.body.error, /This network has already claimed/);
});

test("malformed wallets are refused before anything is sent", { skip }, async () => {
  assert.equal((await claim("not-a-wallet", "192.0.2.1")).status, 400);
  // SolFX's protocol PDA on devnet: off the curve, so nobody could spend what was sent to it.
  const pda = await claim("GbgsnqqqRws5Ch33eHuoAWqKghQiVWt8wBSKzwNffjwc", "192.0.2.1");
  assert.equal(pda.status, 400);
  assert.match(pda.body.error, /program-derived/);
});

test("the limit survives a restart, and an empty faucet says so", { skip }, async () => {
  await stop(children[0]);
  // A claim size larger than the remaining balance: the faucet must refuse, not send a partial.
  const big = startFaucet({ SOLFX_FAUCET_USDC: "100000" });
  children[0] = big;
  await waitFor(`http://127.0.0.1:${FAUCET_PORT}/faucet/health`);
  const w = await generateKeyPairSigner();
  const r = await claim(w.address, "192.0.2.50");
  assert.equal(r.status, 503);
  assert.match(r.body.error, /faucet is empty/);
  assert.match(big.log(), /EMPTY/);
  assert.match(big.log(), /LOW USDC/);
  // The refused claim was not counted: the same wallet is refused for emptiness, not for 24 h.
  assert.equal((await claim(w.address, "192.0.2.50")).status, 503);

  await stop(big);
  children[0] = startFaucet();
  await waitFor(`http://127.0.0.1:${FAUCET_PORT}/faucet/health`);
  const health = await (await fetch(`http://127.0.0.1:${FAUCET_PORT}/faucet/health`)).json();
  assert.equal(health.usdc, "700", "1000 funded, three claims of 100 paid");
  assert.equal(health.walletsLast24h, 3, "read back from the claims file");
});
