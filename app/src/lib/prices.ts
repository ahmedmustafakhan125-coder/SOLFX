/**
 * Live prices, read from the accounts that are actually being published to.
 *
 * The trap this navigates is documented in the SDK: on a cluster with no sponsored feed the
 * price-update account is a keypair the poster created, not the canonical `[shard, feed_id]`
 * PDA. Deriving the sponsored address compiles and then reads an account nobody writes to.
 * `price-accounts.json` is the poster's own map, served as a static asset.
 */
import { getPriceUpdateV2Decoder, PriceAccountMap, type PriceAccountEntry } from "@solfx/client";
import type { Address, Rpc, SolanaRpcApi } from "@solana/kit";
import { getBase64Encoder } from "@solana/kit";

export type LivePrice = {
  /** Normalised to PRICE_PRECISION (1e9), the scale the program works in. */
  readonly price: bigint;
  readonly conf: bigint;
  readonly publishTime: bigint;
  /** Seconds since publication, against the program's 60s staleness gate. */
  readonly ageSeconds: number;
  readonly stale: boolean;
};

/** The program's `MAX_ALLOWED_STALENESS_SECONDS`. */
export const MAX_STALENESS_SECONDS = 60;

const PRICE_PRECISION = 1_000_000_000n;

/**
 * A raw Pyth mantissa is not a price. `(price, exponent)` has to be scaled to
 * PRICE_PRECISION before it means anything — BTC/USD publishes at exponent −8 against an
 * engine at 1e9, and skipping this puts every size and bound out by a factor of ten.
 */
function normalise(mantissa: bigint, exponent: number): bigint {
  if (exponent <= 0) {
    const scale = 10n ** BigInt(-exponent);
    return (mantissa * PRICE_PRECISION) / scale;
  }
  return mantissa * PRICE_PRECISION * 10n ** BigInt(exponent);
}

export async function loadPriceMap(): Promise<PriceAccountMap> {
  const res = await fetch("/price-accounts.json");
  if (!res.ok) throw new Error(`price-accounts.json: ${res.status}`);
  return new PriceAccountMap((await res.json()) as PriceAccountEntry[]);
}

export async function readPrice(
  rpc: Rpc<SolanaRpcApi>,
  account: Address,
): Promise<LivePrice | undefined> {
  const { value } = await rpc.getAccountInfo(account, { encoding: "base64" }).send();
  if (!value) return undefined;

  const raw = new Uint8Array(getBase64Encoder().encode(value.data[0]));
  // PriceUpdateV2 begins after the 8-byte Anchor discriminator.
  const update = getPriceUpdateV2Decoder().decode(raw.subarray(8));
  const m = update.priceMessage;

  const publishTime = m.publishTime;
  const ageSeconds = Math.floor(Date.now() / 1000) - Number(publishTime);
  return {
    price: normalise(m.price, m.exponent),
    conf: normalise(m.conf, m.exponent),
    publishTime,
    ageSeconds,
    stale: ageSeconds > MAX_STALENESS_SECONDS,
  };
}
