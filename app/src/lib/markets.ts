/**
 * Reading the venue. Every address here comes from a generated PDA helper and every number
 * from a generated decoder — nothing about the program's layout is restated in this app.
 */
import {
  fetchProtocol,
  findMarketPda,
  findProtocolPda,
  getMarketDecoder,
  type Market,
} from "@solfx/client";
import type { Address, Rpc, SolanaRpcApi } from "@solana/kit";

import { READ_COMMITMENT } from "@/lib/commitment";

/** `MarketStatus` as the program orders it. */
export const STATUS = [
  "Initialized",
  "Active",
  "ReduceOnly",
  "Halted",
  "GapWindow",
  "WeekendMode",
  "PreOpenWindow",
  "Delisted",
] as const;

export type LoadedMarket = {
  readonly index: number;
  readonly address: Address;
  readonly symbol: string;
  readonly status: string;
  /** Only Active and WeekendMode permit opening — mirrors `MarketStatus::allows_open`. */
  readonly tradeable: boolean;
  /**
   * `Market::effective_max_leverage()` — the weekend cap when one is set and in force, not
   * the weekday field. Reading `data.maxLeverage` directly lets a ticket offer weekday
   * leverage on a `WeekendMode` market, which the program then refuses.
   */
  readonly maxLeverage: number;
  /** `Market::effective_base_spread_bps()`, widened in `WeekendMode` the same way. */
  readonly baseSpreadBps: number;
  readonly feedIdHex: string;
  readonly data: Market;
};

const decoder = new TextDecoder();

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function symbolOf(m: Market): string {
  return decoder.decode(Uint8Array.from(m.symbol)).replace(/\0+$/, "");
}

export function feedHex(m: Market): string {
  return Array.from(m.pythFeedId)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * Every market the protocol knows about, in index order.
 *
 * `num_markets` only ever grows, so this is the complete set — including markets that have
 * been halted and taken out of service. Filtering is the caller's decision, and deliberately
 * not hidden here: a client that silently drops accounts cannot explain why a market it once
 * showed has gone.
 */
export async function loadMarkets(
  rpc: Rpc<SolanaRpcApi>
): Promise<LoadedMarket[]> {
  const [protocolPda] = await findProtocolPda();
  const protocol = await fetchProtocol(rpc, protocolPda, {
    commitment: READ_COMMITMENT,
  });

  const indexes = Array.from({ length: protocol.data.numMarkets }, (_, i) => i);
  const addresses = await Promise.all(
    indexes.map(async (marketIndex) => {
      const [address] = await findMarketPda({ marketIndex });
      return address;
    })
  );

  // One read for every market, not one read per market.
  //
  // This was a `fetchMarket` inside the loop: eleven markets meant eleven sequential round
  // trips after the protocol read, measured at 3.0s through the gateway from the same
  // datacentre and considerably worse from a browser, on the path that blocks first paint.
  // `getMultipleAccounts` takes up to 100 keys, which is well past the market count.
  const raw: (Market | undefined)[] = [];
  for (let i = 0; i < addresses.length; i += 100) {
    const { value } = await rpc
      .getMultipleAccounts(addresses.slice(i, i + 100), {
        commitment: READ_COMMITMENT,
        encoding: "base64",
      })
      .send();
    for (const account of value) {
      raw.push(
        account
          ? getMarketDecoder().decode(base64ToBytes(account.data[0]))
          : undefined
      );
    }
  }

  const out: LoadedMarket[] = [];
  for (const index of indexes) {
    const d = raw[index];
    // `num_markets` only ever grows and every index below it has been initialised, so a gap
    // here means the read failed rather than that the market is absent. Skipping is right:
    // rendering a market with invented fields would be worse than not listing it.
    if (!d) continue;
    const status = STATUS[d.status] ?? `Unknown(${d.status})`;
    out.push({
      index,
      address: addresses[index] as Address,
      symbol: symbolOf(d),
      status,
      tradeable: status === "Active" || status === "WeekendMode",
      maxLeverage:
        status === "WeekendMode" && d.weekendMaxLeverage > 0
          ? d.weekendMaxLeverage
          : d.maxLeverage,
      baseSpreadBps:
        status === "WeekendMode" && d.weekendSpreadBps > 0
          ? d.weekendSpreadBps
          : d.baseSpreadBps,
      feedIdHex: feedHex(d),
      data: d,
    });
  }
  return out;
}
