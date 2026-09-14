/**
 * An exact, diffable snapshot of every position on chain.
 *
 * Task 6 § 12.5 step 4 asks whether a position opened *before* a market was listed is
 * bit-for-bit unaffected by trading on that market afterwards. A screenshot cannot answer
 * that; these are the stored fields, printed so two runs can be `diff`ed.
 *
 * Deliberately not the `trade` CLI. That binary signs with a keypair file, so the carried
 * position would belong to the operator wallet while the browser trade belongs to Phantom —
 * two different `UserAccount`s, which is a weaker test than one. Reading the chain directly
 * lets both halves live in the same account.
 *
 *   cd app
 *   npx tsx scripts/position-snapshot.mts > /tmp/before.txt
 *   ... trade ETH in the browser ...
 *   npx tsx scripts/position-snapshot.mts > /tmp/after.txt
 *   diff /tmp/before.txt /tmp/after.txt
 *
 * It lives under `app/` rather than `scripts/` only because that is where `@solana/kit`
 * and `@solfx/client` resolve; the repo root has no node_modules.
 */
import { createSolanaRpc, getBase58Decoder, type Address } from "@solana/kit";
import { POSITION_DISCRIMINATOR, getPositionDecoder } from "@solfx/client";

const RPC = process.env.SOLFX_RPC_URL ?? "https://solfx.cloud/rpc";
const PROGRAM = "2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi" as Address;

const rpc = createSolanaRpc(RPC);
const accounts = await rpc
  .getProgramAccounts(PROGRAM, {
    encoding: "base64",
    commitment: "confirmed",
    filters: [
      {
        memcmp: {
          offset: 0n,
          bytes: getBase58Decoder().decode(POSITION_DISCRIMINATOR) as never,
          encoding: "base58",
        },
      },
    ],
  })
  .send();

console.log(`${accounts.length} open position(s)`);

// Sorted by address so the two runs line up even if the RPC returns them in another order.
const rows = accounts
  .map((a) => ({
    address: a.pubkey,
    p: getPositionDecoder().decode(
      Uint8Array.from(atob(a.account.data[0]), (c) => c.charCodeAt(0))
    ),
  }))
  .sort((x, y) => (x.address < y.address ? -1 : 1));

for (const { address, p } of rows) {
  console.log(`\n${address}`);
  // Every field the extensibility claim is about. `lastFundingRate`/`lastCarryTs` are
  // expected to move — funding accrues hourly on every open position by design — so they
  // are printed too rather than hidden, and a diff on them is read, not treated as failure.
  for (const [k, v] of Object.entries(p)) {
    if (k === "discriminator" || k === "reserved") continue;
    console.log(
      `  ${k.padEnd(22)} ${typeof v === "object" ? JSON.stringify(v) : String(v)}`
    );
  }
}
