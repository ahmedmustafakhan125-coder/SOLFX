import { useEffect, useState } from "react";

import { HERMES_TOKEN, HERMES_URL } from "@/config";

/** The six markets actually listed on chain, in display order. */
export const TICKER = [
  { symbol: "EUR/USD", name: "Euro / US Dollar", glyph: "€", places: 5,
    id: "a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b" },
  { symbol: "USD/JPY", name: "US Dollar / Japanese Yen", glyph: "¥", places: 3,
    id: "ef2c98c804ba503c6a707e38be4dfbb16683775f195b091252bf24693042fd52" },
  { symbol: "XAU/USD", name: "Gold / US Dollar", glyph: "Au", places: 2,
    id: "765d2ba906dbc32ca17cc11f5310a89e9ee1f6420508c63861f2f8ba4ee34bb2" },
  { symbol: "USD/CNH", name: "US Dollar / Offshore Yuan", glyph: "¥", places: 4,
    id: "eef52e09c878ad41f6a81803e3640fe04dceea727de894edd4ea117e2e332e66" },
  { symbol: "XAG/USD", name: "Silver / US Dollar", glyph: "Ag", places: 3,
    id: "f2fb02c32b055c805e7238d628e5e9dadef274376114eb1f012337cabe93871e" },
  { symbol: "BTC/USD", name: "Bitcoin / US Dollar", glyph: "₿", places: 2,
    id: "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43" },
] as const;

export type Quote = {
  readonly price: number;
  /** Change against the EMA, which is the only reference Hermes hands back in one call. */
  readonly changePct: number;
  readonly ageSeconds: number;
};

/**
 * Live quotes straight from Hermes, for the landing page.
 *
 * Deliberately not read from chain: the landing page should render for a visitor with no
 * wallet and no RPC, and these are the same Pyth numbers the on-chain accounts carry.
 */
export function useTicker(pollMs = 10_000): Record<string, Quote | undefined> {
  const [quotes, setQuotes] = useState<Record<string, Quote | undefined>>({});

  useEffect(() => {
    let stop = false;
    const read = async () => {
      const ids = TICKER.map((t) => `ids[]=${t.id}`).join("&");
      const url = `${HERMES_URL.replace(/\/$/, "")}/v2/updates/price/latest?${ids}&parsed=true`;
      try {
        const res = await fetch(url, {
          headers: HERMES_TOKEN ? { Authorization: `Bearer ${HERMES_TOKEN}` } : {},
        });
        if (!res.ok) return;
        const json = (await res.json()) as {
          parsed?: {
            id: string;
            price: { price: string; expo: number; publish_time: number };
            ema_price: { price: string; expo: number };
          }[];
        };
        const now = Math.floor(Date.now() / 1000);
        const next: Record<string, Quote> = {};
        for (const p of json.parsed ?? []) {
          const t = TICKER.find((x) => x.id === p.id.replace(/^0x/, ""));
          if (!t) continue;
          const price = Number(p.price.price) * 10 ** p.price.expo;
          const ema = Number(p.ema_price.price) * 10 ** p.ema_price.expo;
          next[t.symbol] = {
            price,
            changePct: ema === 0 ? 0 : ((price - ema) / ema) * 100,
            ageSeconds: now - p.price.publish_time,
          };
        }
        if (!stop) setQuotes(next);
      } catch {
        /* the page renders without quotes rather than failing */
      }
    };
    void read();
    const id = setInterval(() => void read(), pollMs);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [pollMs]);

  return quotes;
}
