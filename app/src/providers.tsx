import { autoDiscover, createClient } from "@solana/client";
import { SolanaProvider } from "@solana/react-hooks";
import type { PropsWithChildren } from "react";

import { RPC_URL } from "@/config";

const client = createClient({
  endpoint: RPC_URL,
  // Wallet Standard discovery: Phantom, Solflare, Backpack and anything else installed,
  // with no per-wallet adapter packages.
  walletConnectors: autoDiscover(),
});

export function Providers({ children }: PropsWithChildren) {
  return <SolanaProvider client={client}>{children}</SolanaProvider>;
}
