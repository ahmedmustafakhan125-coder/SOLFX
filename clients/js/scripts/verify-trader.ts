// Re-derive every NOXFUNDS trader's record from the program's events and compare it with the
// record the program keeps. The same check `/nox/verify` runs in the browser.
//
//   SOLFX_RPC_URL=https://api.devnet.solana.com npx tsx scripts/verify-trader.ts [trader]
import { createSolanaRpc, getBase58Decoder, type Address } from "@solana/kit";

import { getTraderProfileDecoder, TRADER_PROFILE_DISCRIMINATOR } from "../src/generated-noxfunds/accounts/traderProfile.js";
import { NOXFUNDS_PROGRAM_ADDRESS } from "../src/generated-noxfunds/programs/noxfunds.js";
import { compareProfile, fetchProfileHistory, rederiveProfile } from "../src/nox/verify.js";

const rpcUrl = process.env.SOLFX_RPC_URL ?? "https://api.devnet.solana.com";
const rpc = createSolanaRpc(rpcUrl);
const only = process.argv[2];

const accounts = await rpc
  .getProgramAccounts(NOXFUNDS_PROGRAM_ADDRESS, {
    encoding: "base64",
    filters: [
      {
        memcmp: {
          offset: 0n,
          bytes: getBase58Decoder().decode(TRADER_PROFILE_DISCRIMINATOR) as never,
          encoding: "base58",
        },
      },
    ],
  })
  .send();

let failures = 0;
for (const { pubkey, account } of accounts) {
  const p = getTraderProfileDecoder().decode(Buffer.from(account.data[0], "base64"));
  if (only && p.authority !== only) continue;
  const txs = await fetchProfileHistory(rpc, pubkey as Address, { concurrency: 2 });
  const r = rederiveProfile(pubkey as Address, p.authority, txs);
  const rows = compareProfile(p, r.derived);
  const bad = rows.filter((x) => !x.agrees);
  failures += bad.length;
  console.log(`\ntrader ${p.authority}  profile ${pubkey}`);
  console.log(
    `  ${txs.length} transactions, ${r.trail.length} events that moved the record, ` +
      `${r.failedSkipped} failed transactions skipped, ${r.undecodable} undecodable`,
  );
  for (const x of rows) {
    console.log(`  ${x.agrees ? "ok  " : "DIFF"} ${x.field.padEnd(24)} on chain ${x.onChain.padStart(14)}   replayed ${x.replayed.padStart(14)}`);
  }
  if (r.firstDisagreement) console.log("  running totals first disagree at", r.firstDisagreement);
  if (r.unprovable.length) console.log("  settled-in-profit unprovable for", r.unprovable);
}
console.log(failures === 0 ? "\nevery figure re-derived" : `\n${failures} figure(s) disagree`);
process.exitCode = failures === 0 ? 0 : 1;
