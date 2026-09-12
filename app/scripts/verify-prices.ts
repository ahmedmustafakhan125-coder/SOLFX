// Exercises exactly the decode path the browser uses, against live devnet.
import { createSolanaRpc, getBase64Encoder } from "@solana/kit";
import { getPriceUpdateV2Decoder, PriceAccountMap } from "@solfx/client";
import { readFileSync } from "node:fs";

const PRICE_PRECISION = 1_000_000_000n;
function normalise(m: bigint, e: number): bigint {
  if (e <= 0) return (m * PRICE_PRECISION) / 10n ** BigInt(-e);
  return m * PRICE_PRECISION * 10n ** BigInt(e);
}

async function main() {
  const rpc = createSolanaRpc(process.env.SOLFX_RPC_URL!);
  const map = new PriceAccountMap(
    JSON.parse(readFileSync("public/price-accounts.json", "utf8"))
  );
  console.log(`price map: ${map.size} feeds\n`);

  for (const sym of ["BTC/USD", "EUR/USD", "XAU/USD", "USD/JPY"]) {
    const acct = map.forSymbol(sym);
    if (!acct) {
      console.log(`  ${sym.padEnd(8)} no account`);
      continue;
    }
    const { value } = await rpc
      .getAccountInfo(acct, { encoding: "base64" })
      .send();
    if (!value) {
      console.log(`  ${sym.padEnd(8)} account missing`);
      continue;
    }
    const raw = new Uint8Array(getBase64Encoder().encode(value.data[0]));
    const u = getPriceUpdateV2Decoder().decode(raw.subarray(8));
    const m = u.priceMessage;
    const price = normalise(m.price, m.exponent);
    const age = Math.floor(Date.now() / 1000) - Number(m.publishTime);
    const whole = price / PRICE_PRECISION;
    const frac = (price % PRICE_PRECISION)
      .toString()
      .padStart(9, "0")
      .slice(0, 5);
    console.log(
      `  ${sym.padEnd(8)} ${String(whole).padStart(7)}.${frac}  exp=${m.exponent}  age=${age}s  ${age > 60 ? "STALE" : "ok"}  verif=${u.verificationLevel.__kind ?? u.verificationLevel}`
    );
  }
}
void main();
