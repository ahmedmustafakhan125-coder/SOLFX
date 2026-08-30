import { useCallback, useState } from "react";
import { useSolanaClient, useWalletConnection } from "@solana/react-hooks";
import type { Instruction } from "@solana/kit";

export type SendState = {
  readonly busy: boolean;
  readonly signature: string | undefined;
  readonly error: string | undefined;
  /** Program logs from a failed preflight. Without these a rejection is unreadable. */
  readonly logs: readonly string[] | undefined;
};

/**
 * Pull the program logs out of whatever the RPC threw.
 *
 * Preflight failures carry the logs that say *why* the program rejected the transaction —
 * `OracleStale`, `SlippageExceeded`, `LeverageTooHigh`. An error handler that drops them
 * turns a one-line diagnosis into an afternoon.
 */
function extractLogs(e: unknown): string[] | undefined {
  const seen = new Set<unknown>();
  const walk = (v: unknown): string[] | undefined => {
    if (!v || typeof v !== "object" || seen.has(v)) return undefined;
    seen.add(v);
    const o = v as Record<string, unknown>;
    if (Array.isArray(o.logs) && o.logs.every((l) => typeof l === "string")) {
      return o.logs as string[];
    }
    for (const key of ["cause", "context", "data", "value", "err", "error"]) {
      const found = walk(o[key]);
      if (found) return found;
    }
    return undefined;
  };
  return walk(e);
}

function message(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * Prepare and send instructions with the connected wallet.
 *
 * Preflight is deliberately left on. It costs a simulation round trip and it is the only
 * thing standing between a user and a signed transaction that was always going to fail.
 */
export function useSend() {
  const client = useSolanaClient();
  const { wallet } = useWalletConnection();
  const [state, setState] = useState<SendState>({
    busy: false,
    signature: undefined,
    error: undefined,
    logs: undefined,
  });

  const send = useCallback(
    async (instructions: Instruction[], computeUnitLimit?: number): Promise<string | undefined> => {
      if (!wallet) {
        setState({ busy: false, signature: undefined, error: "Connect a wallet first", logs: undefined });
        return undefined;
      }
      setState({ busy: true, signature: undefined, error: undefined, logs: undefined });
      try {
        const prepared = await client.helpers.transaction.prepare({
          authority: wallet,
          instructions,
          ...(computeUnitLimit === undefined ? {} : { computeUnitLimit }),
        });
        const sig = await client.helpers.transaction.send(prepared, {
          commitment: "confirmed",
          skipPreflight: false,
        });
        const signature = sig.toString();
        setState({ busy: false, signature, error: undefined, logs: undefined });
        return signature;
      } catch (e) {
        setState({
          busy: false,
          signature: undefined,
          error: message(e),
          logs: extractLogs(e),
        });
        return undefined;
      }
    },
    [client, wallet],
  );

  const reset = useCallback(
    () => setState({ busy: false, signature: undefined, error: undefined, logs: undefined }),
    [],
  );

  return { ...state, send, reset };
}
