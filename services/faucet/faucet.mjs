/**
 * The devnet faucet: test USDC, and a little SOL when the claimant has none, for a wallet that
 * has never used SolFX.
 *
 * # Why it exists
 *
 * The collateral mint is SolFX's own test mint, so no public faucet carries it, and its mint
 * authority is a key that must never be on a server. A first-time visitor with a fresh Phantom
 * wallet therefore had no way to get collateral without asking the operator — and the public
 * devnet SOL faucet is unreliable enough that they often had no SOL for fees either.
 *
 * # Why it can be on a server at all
 *
 * It holds a **dedicated** keypair that owns nothing but a balance of test USDC and devnet SOL,
 * funded from the mint authority by `scripts/fund-faucet.sh` on the operator's own machine.
 * The server never mints. Losing this key loses test tokens, nothing else.
 *
 * # Why it is its own process, not a route inside `server.mjs`
 *
 * `server.mjs` runs inside the internet-facing `solfx-web` container, which is deliberately
 * given no secrets — `price-accounts/` and `deployment.json` stay out of it for the same reason
 * (`/docker/solfx/docker-compose.yml`). A signing key belongs on the host, beside the keeper's.
 * It also keeps `server.mjs` dependency-free: building SPL transactions needs `@solana/kit` and
 * `@solana-program/token`, which live in this directory's own `package.json`. The container only
 * forwards `POST /faucet` here, with the caller's address.
 *
 * # Shape
 *
 *   browser --POST /faucet--> server.mjs (container) --> this, on 172.18.0.1:8789 (host)
 *                                                          |
 *                                         RPC gateway /low lane --> keyed devnet RPC
 *
 * The RPC is the gateway's low-priority lane, not the keyed URL directly: measured 2026-09-08, a
 * second client on the key starved the price poster, and stale prices fail every trade.
 */
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  getSignatureFromTransaction,
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
  getTransferCheckedInstruction,
} from "@solana-program/token";
import { getTransferSolInstruction } from "@solana-program/system";

import {
  ClaimLimiter,
  canCover,
  formatUnits,
  humanDuration,
  parseUnits,
  planClaim,
  validateWallet,
} from "./lib.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");

/** Environment first, then the repo `.env`, as every other service here resolves settings. */
function setting(name) {
  const fromEnv = process.env[name]?.trim();
  if (fromEnv) return fromEnv;
  try {
    const env = readFileSync(join(repoRoot, ".env"), "utf8");
    return new RegExp(`^${name}=(.*)$`, "m").exec(env)?.[1]?.trim() || undefined;
  } catch {
    return undefined;
  }
}

/** The collateral mint: env, else the deployment record `init-protocol` wrote. */
function usdcMint() {
  const fromEnv = setting("SOLFX_USDC_MINT");
  if (fromEnv) return fromEnv;
  try {
    return JSON.parse(readFileSync(join(repoRoot, "deployment.json"), "utf8")).usdc_mint;
  } catch {
    return undefined;
  }
}

const SOL_DECIMALS = 9;
const HOST = setting("SOLFX_FAUCET_HOST") ?? "127.0.0.1";
const PORT = Number(setting("SOLFX_FAUCET_PORT") ?? 8789);
const RPC_URL = setting("SOLFX_FAUCET_RPC_URL") ?? "http://127.0.0.1:8899/low";
const KEYPAIR = setting("SOLFX_FAUCET_KEYPAIR");
const MINT = usdcMint();
const USDC_TEXT = setting("SOLFX_FAUCET_USDC") ?? "10000";
const SOL_TEXT = setting("SOLFX_FAUCET_SOL") ?? "0.05";
const SOL_BELOW_TEXT = setting("SOLFX_FAUCET_SOL_BELOW") ?? "0.02";
const PER_IP = Number(setting("SOLFX_FAUCET_PER_IP") ?? 5);
const WARN_USDC_TEXT = setting("SOLFX_FAUCET_WARN_USDC") ?? "100000";
const WARN_SOL_TEXT = setting("SOLFX_FAUCET_WARN_SOL") ?? "1";
const CLAIMS_PATH = setting("SOLFX_FAUCET_CLAIMS") ?? join(here, "data", "claims.json");
/** Covers the transaction fee, which the faucet pays, with room for a priority fee later. */
const FEE_MARGIN = 20_000n;
const CONFIRM_TIMEOUT_MS = 60_000;
/** Claims are served one at a time; past this many waiting, a caller is told to come back. */
const MAX_WAITING = 20;

function die(message) {
  console.error(`[faucet] ${message} — refusing to start`);
  process.exit(1);
}
if (!KEYPAIR) die("SOLFX_FAUCET_KEYPAIR is not set");
if (!MINT) die("no USDC mint: set SOLFX_USDC_MINT or provide deployment.json");
if (!Number.isInteger(PER_IP) || PER_IP < 1) die("SOLFX_FAUCET_PER_IP must be a positive integer");

const rpc = createSolanaRpc(RPC_URL);
const signer = await createKeyPairSignerFromBytes(
  Uint8Array.from(JSON.parse(readFileSync(KEYPAIR, "utf8"))),
);
const mint = address(MINT);
const { data: mintData } = await fetchMint(rpc, mint);
const DECIMALS = mintData.decimals;
const USDC = parseUnits(USDC_TEXT, DECIMALS);
const SOL = parseUnits(SOL_TEXT, SOL_DECIMALS);
const SOL_BELOW = parseUnits(SOL_BELOW_TEXT, SOL_DECIMALS);
const WARN_USDC = parseUnits(WARN_USDC_TEXT, DECIMALS);
const WARN_SOL = parseUnits(WARN_SOL_TEXT, SOL_DECIMALS);
const [faucetAta] = await findAssociatedTokenPda({
  mint,
  owner: signer.address,
  tokenProgram: TOKEN_PROGRAM_ADDRESS,
});
/** A classic SPL token account is 165 bytes; the faucet pays it for every new claimant. */
const ATA_RENT = await rpc.getMinimumBalanceForRentExemption(165n).send();

const limiter = new ClaimLimiter({ path: CLAIMS_PATH, perIp: PER_IP });

async function balances() {
  const [{ value: lamports }, usdc] = await Promise.all([
    rpc.getBalance(signer.address, { commitment: "confirmed" }).send(),
    rpc
      .getTokenAccountBalance(faucetAta, { commitment: "confirmed" })
      .send()
      .then((r) => BigInt(r.value.amount))
      // No token account yet means nothing to give, not an error in the faucet.
      .catch(() => 0n),
  ]);
  return { lamports, usdc };
}

function warnIfLow({ lamports, usdc }) {
  if (usdc < WARN_USDC) {
    console.warn(
      `[faucet] LOW USDC: ${formatUnits(usdc, DECIMALS)} left (warn below ${WARN_USDC_TEXT}). ` +
        "Refill with scripts/fund-faucet.sh.",
    );
  }
  if (lamports < WARN_SOL) {
    console.warn(
      `[faucet] LOW SOL: ${formatUnits(lamports, SOL_DECIMALS)} left (warn below ${WARN_SOL_TEXT}). ` +
        "Refill with scripts/fund-faucet.sh.",
    );
  }
}

/** Wait for `confirmed`, or for the blockhash to expire, or the timeout. */
async function confirm(signature, lastValidBlockHeight) {
  const deadline = Date.now() + CONFIRM_TIMEOUT_MS;
  for (let i = 0; Date.now() < deadline; i++) {
    await new Promise((r) => setTimeout(r, 750));
    const { value } = await rpc.getSignatureStatuses([signature]).send();
    const st = value[0];
    if (st?.err) return { state: "failed", err: st.err };
    if (st?.confirmationStatus === "confirmed" || st?.confirmationStatus === "finalized") {
      return { state: "confirmed" };
    }
    // Checking the block height every poll would double the calls for no benefit.
    if (i % 4 === 3) {
      const height = await rpc.getBlockHeight({ commitment: "confirmed" }).send();
      if (height > lastValidBlockHeight) return { state: "expired" };
    }
  }
  return { state: "timeout" };
}

/** One claim, end to end. Returns [status, body]. */
async function claim(wallet, ip) {
  const allowed = limiter.check(wallet, ip);
  if (!allowed.ok) {
    if (allowed.reason === "in-progress") {
      return [409, { error: "A claim for this wallet is already being processed." }];
    }
    const secs = Math.ceil(allowed.retryAfterMs / 1000);
    const who = allowed.reason === "wallet" ? "This wallet has" : "This network has";
    return [
      429,
      {
        error: `${who} already claimed test USDC in the last 24 hours. Try again in ${humanDuration(allowed.retryAfterMs)}.`,
        retryAfterSeconds: secs,
      },
    ];
  }

  limiter.reserve(wallet);
  let sent = false;
  try {
    const owner = address(wallet);
    const [{ value: userLamports }, faucet] = await Promise.all([
      rpc.getBalance(owner, { commitment: "confirmed" }).send(),
      balances(),
    ]);
    const plan = planClaim({ userLamports, usdc: USDC, solAmount: SOL, solBelow: SOL_BELOW });
    const cover = canCover({
      faucetUsdc: faucet.usdc,
      faucetLamports: faucet.lamports,
      plan,
      ataRent: ATA_RENT,
      feeMargin: FEE_MARGIN,
    });
    if (!cover.ok) {
      limiter.release(wallet);
      console.error(`[faucet] EMPTY: cannot cover a claim (short of ${cover.short}). Refill it.`);
      warnIfLow(faucet);
      return [503, { error: "The faucet is empty. The operator has been alerted; please try again later." }];
    }

    const [userAta] = await findAssociatedTokenPda({
      mint,
      owner,
      tokenProgram: TOKEN_PROGRAM_ADDRESS,
    });
    const instructions = [
      // Idempotent: a wallet that already has the account pays nothing and fails nothing.
      await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: signer, owner, mint }),
      getTransferCheckedInstruction({
        source: faucetAta,
        mint,
        destination: userAta,
        authority: signer,
        amount: plan.usdc,
        decimals: DECIMALS,
      }),
    ];
    if (plan.lamports > 0n) {
      instructions.push(
        getTransferSolInstruction({ source: signer, destination: owner, amount: plan.lamports }),
      );
    }

    const { value: blockhash } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
    const message = pipe(
      createTransactionMessage({ version: 0 }),
      (m) => setTransactionMessageFeePayerSigner(signer, m),
      (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
      (m) => appendTransactionMessageInstructions(instructions, m),
    );
    const tx = await signTransactionMessageWithSigners(message);
    const signature = getSignatureFromTransaction(tx);

    // Preflight on: a transaction that would fail is refused here, before it costs a fee.
    await rpc
      .sendTransaction(getBase64EncodedWireTransaction(tx), {
        encoding: "base64",
        preflightCommitment: "confirmed",
      })
      .send();
    sent = true;

    const outcome = await confirm(signature, blockhash.lastValidBlockHeight);
    if (outcome.state === "failed" || outcome.state === "expired") {
      limiter.release(wallet);
      console.error(`[faucet] claim for ${wallet} ${outcome.state}: ${signature}`);
      return [502, { error: "The faucet transaction did not land. Please try again.", signature }];
    }
    // Confirmed, or still pending at the timeout. Pending is counted too: it may yet land, and
    // a limit that a slow cluster could reset is not a limit.
    limiter.commit(wallet, ip);
    const body = {
      signature,
      usdc: formatUnits(plan.usdc, DECIMALS),
      sol: formatUnits(plan.lamports, SOL_DECIMALS),
    };
    console.log(
      `[faucet] ${outcome.state} ${wallet}: ${body.usdc} USDC, ${body.sol} SOL, ${signature}`,
    );
    balances().then(warnIfLow).catch(() => {});
    return outcome.state === "confirmed"
      ? [200, body]
      : [202, { ...body, pending: true, note: "Sent; confirmation is slow. Check the explorer link." }];
  } catch (e) {
    // Before the send nothing happened, so the claim is released. After it, it may have landed.
    if (sent) limiter.commit(wallet, ip);
    else limiter.release(wallet);
    console.error(`[faucet] claim for ${wallet} errored ${sent ? "after" : "before"} sending: ${e?.message ?? e}`);
    return [502, { error: "The faucet could not send right now. Please try again in a minute." }];
  }
}

// One claim at a time. Concurrency buys nothing at this volume and makes the balance check and
// the rate limit racy; a queue makes both trivially correct.
let chain = Promise.resolve();
let waiting = 0;
function serialised(fn) {
  waiting += 1;
  const run = chain.then(fn, fn);
  chain = run.catch(() => {}).finally(() => (waiting -= 1));
  return run;
}

function reply(res, status, body) {
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
}

const server = createServer(async (req, res) => {
  const { pathname } = new URL(req.url ?? "/", "http://localhost");

  if (pathname === "/faucet/health" && req.method === "GET") {
    try {
      const b = await balances();
      return reply(res, 200, {
        ok: true,
        address: signer.address,
        usdc: formatUnits(b.usdc, DECIMALS),
        sol: formatUnits(b.lamports, SOL_DECIMALS),
        claimsPerUsdc: Number(b.usdc / USDC),
        walletsLast24h: limiter.wallets.size,
      });
    } catch (e) {
      return reply(res, 502, { ok: false, error: `rpc: ${e?.message ?? e}` });
    }
  }

  if (pathname !== "/faucet") return reply(res, 404, { error: "not found" });
  if (req.method !== "POST") return reply(res, 405, { error: "POST only" });

  let raw = "";
  for await (const chunk of req) {
    raw += chunk;
    if (raw.length > 1024) return reply(res, 413, { error: "request too large" });
  }
  let wallet;
  try {
    wallet = JSON.parse(raw)?.wallet;
  } catch {
    return reply(res, 400, { error: 'Body must be JSON: { "wallet": "<address>" }' });
  }
  const v = validateWallet(wallet);
  if (!v.ok) return reply(res, 400, { error: v.error });

  // Set by server.mjs from the hop Traefik appended. This port is bound to the Docker bridge,
  // so only the host and containers on that network can reach it to set the header at all.
  const ip = String(req.headers["x-solfx-client-ip"] ?? "") || req.socket.remoteAddress || "unknown";

  if (waiting >= MAX_WAITING) {
    return reply(res, 503, { error: "The faucet is busy. Please try again in a minute." });
  }
  const [status, body] = await serialised(() => claim(v.wallet, ip));
  reply(res, status, body);
});

const start = await balances();
server.listen(PORT, HOST, () => {
  console.log(
    `[faucet] ${signer.address} on ${HOST}:${PORT}, mint ${mint} (${DECIMALS} dp), ` +
      `${USDC_TEXT} USDC + ${SOL_TEXT} SOL below ${SOL_BELOW_TEXT}, ${PER_IP}/IP/day. ` +
      `Holds ${formatUnits(start.usdc, DECIMALS)} USDC, ${formatUnits(start.lamports, SOL_DECIMALS)} SOL.`,
  );
  warnIfLow(start);
});

for (const sig of ["SIGTERM", "SIGINT"]) {
  process.on(sig, () => server.close(() => process.exit(0)));
}
