import { useEffect, useState } from "react";

import { Header } from "@/components/Header";
import { PriceChart } from "@/components/PriceChart";
import { MarketList } from "@/components/MarketList";
import { MarketPanel } from "@/components/MarketPanel";
import { AccountPanel } from "@/components/AccountPanel";
import { PositionsPanel } from "@/components/PositionsPanel";
import { usePositions } from "@/hooks/usePositions";
import { OrderTicket } from "@/components/OrderTicket";
import { useSolfx } from "@/hooks/useSolfx";
import { RPC_URL, rpcLabel } from "@/config";

export function Terminal() {
  const { markets, prices, priceAccounts, loading, error } = useSolfx();
  const [selected, setSelected] = useState<number | undefined>(undefined);

  // Default to the first market that can actually be traded, rather than index 0 — on this
  // cluster index 0 happens to be live, but that is a fact about the deployment, not a rule.
  useEffect(() => {
    if (selected === undefined) {
      const first = markets.find((m) => m.tradeable);
      if (first) setSelected(first.index);
    }
  }, [markets, selected]);

  // Positions are priced from the same poll the terminal already runs, keyed by market
  // index rather than feed id because that is what the position accounts carry.
  const priceByIndex = Object.fromEntries(
    markets.map((m) => [m.index, prices[m.feedIdHex]?.price]),
  );
  const accountByIndex = Object.fromEntries(
    markets.map((m) => [m.index, priceAccounts[m.feedIdHex]]),
  );
  const { positions, refresh: refreshPositions } = usePositions(
    markets.map((m) => m.index),
    priceByIndex,
  );

  const market = markets.find((m) => m.index === selected);

  return (
    <div className="flex h-screen flex-col bg-bg text-ink">
      <Header rpcLabel={rpcLabel(RPC_URL)} />

      {error ? (
        <div className="m-5 rounded-md border border-short/40 bg-short/10 p-4 text-sm text-short">
          <div className="font-semibold">Could not read the protocol</div>
          <div className="mt-1 font-mono text-xs opacity-80">{error}</div>
          <div className="mt-2 text-xs opacity-70">
            Set <span className="font-mono">VITE_SOLFX_RPC_URL</span> to an endpoint that has
            SolFX deployed.
          </div>
        </div>
      ) : loading ? (
        <div className="flex flex-1 items-center justify-center text-sm text-ink-dim">
          Reading markets from chain…
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          <MarketList
            markets={markets}
            prices={prices}
            selected={selected}
            onSelect={setSelected}
          />

          <main className="flex min-w-0 flex-1 flex-col">
            {market ? (
              <>
                <MarketPanel market={market} price={prices[market.feedIdHex]} />
                <PriceChart
                  feedIdHex={market.feedIdHex}
                  symbol={market.symbol}
                  live={prices[market.feedIdHex]}
                />
                <PositionsPanel
                  positions={positions}
                  markets={markets}
                  prices={priceByIndex}
                  priceAccounts={accountByIndex}
                  onClosed={refreshPositions}
                />
              </>
            ) : (
              <div className="flex flex-1 items-center justify-center text-sm text-ink-dim">
                No tradeable market on this cluster.
              </div>
            )}
          </main>

          <div className="flex w-80 shrink-0 flex-col overflow-y-auto border-l border-line-soft">
            <AccountPanel />
            {market ? (
              <OrderTicket
                market={market}
                price={prices[market.feedIdHex]}
                priceAccount={priceAccounts[market.feedIdHex]}
              />
            ) : null}
          </div>
        </div>
      )}
    </div>
  );
}
