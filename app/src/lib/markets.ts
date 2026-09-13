/**
 * Reading the venue. Every address here comes from a generated PDA helper and every number
 * from a generated decoder — nothing about the program's layout is restated in this app.
 */
import {
  fetchMarket,
  fetchProtocol,
  findMarketPda,
  findProtocolPda,
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

  const out: LoadedMarket[] = [];
  for (let index = 0; index < protocol.data.numMarkets; index++) {
    const [address] = await findMarketPda({ marketIndex: index });
    const market = await fetchMarket(rpc, address, {
      commitment: READ_COMMITMENT,
    });
    const d = market.data;
    const status = STATUS[d.status] ?? `Unknown(${d.status})`;
    out.push({
      index,
      address,
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
