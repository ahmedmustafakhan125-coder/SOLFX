// Reads the account status the AccountPanel shows, straight from devnet, so the numbers can
// be checked against what the Rust `trade status` command independently reports.
import { createSolanaRpc } from "@solana/kit";
import type { Address, Rpc, SolanaRpcApi } from "@solana/kit";
import { readAccountStatus } from "../src/lib/account.js";

async function main() {
  const rpc = createSolanaRpc(
    process.env.SOLFX_RPC_URL!
  ) as unknown as Rpc<SolanaRpcApi>;
  const owner = (process.argv[2] ??
    "7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs") as Address;
  const s = await readAccountStatus(rpc, owner);
  const usd = (v: bigint) => {
    const w = v / 1_000_000n;
    const f = (v % 1_000_000n).toString().padStart(6, "0");
    return `${w.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",")}.${f}`;
  };
  console.log(`owner            ${owner}`);
  console.log(`usdc mint        ${s.usdcMint}`);
  console.log(
    `user account     ${s.userAccountPda}  ${s.hasUserAccount ? "exists" : "MISSING"}`
  );
  console.log(`ata              ${s.ata}  ${s.hasAta ? "exists" : "MISSING"}`);
  console.log(`free collateral  ${usd(s.freeCollateral)}`);
  console.log(`wallet usdc      ${usd(s.walletUsdc)}`);
}
void main();
