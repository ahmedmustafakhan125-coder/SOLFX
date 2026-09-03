import { useCallback, useState } from "react";
import { useSolanaClient, useWalletConnection } from "@solana/react-hooks";
import type { Instruction } from "@solana/kit";
import { diagnoseSendError } from "@solfx/client";

export type SendState = {
  readonly busy: boolean;
  readonly signature: string | undefined;
  readonly error: string | undefined;
  /** Program logs from a failed preflight. Without these a rejection is unreadable. */
  readonly logs: readonly string[] | undefined;
};

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
    async (
      instructions: Instruction[],
      computeUnitLimit?: number
    ): Promise<string | undefined> => {
      if (!wallet) {
        setState({
          busy: false,
          signature: undefined,
          error: "Connect a wallet first",
          logs: undefined,
        });
        return undefined;
      }
      setState({
        busy: true,
        signature: undefined,
        error: undefined,
        logs: undefined,
      });
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
        // `diagnoseSendError` reaches into kit's transaction-plan result tree, which is where
        // the real cause lives. Without it every failure reads "The provided transaction plan
        // failed to execute", which names neither the program error nor the wallet's refusal.
        const { message, logs } = diagnoseSendError(e);
        setState({ busy: false, signature: undefined, error: message, logs });
        return undefined;
      }
    },
    [client, wallet]
  );

  const reset = useCallback(
    () =>
      setState({
        busy: false,
        signature: undefined,
        error: undefined,
        logs: undefined,
      }),
    []
  );

  return { ...state, send, reset };
}
