import { useCallback, useState } from "react";
import { useSolanaClient } from "@solana/react-hooks";
import type { Instruction } from "@solana/kit";
import { diagnoseSendError } from "@solfx/client";

import { useSigner } from "@/hooks/useSigner";

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
  const signer = useSigner();
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
      if (!signer) {
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
          // The *same* signer instance the instruction builders used. Passing the wallet
          // session here instead makes the client derive its own signer for this address,
          // and kit then sees two distinct signers for one address and refuses. The type
          // accepts either (`TransactionAuthority = TransactionSigner | WalletSession`), and
          // `resolveSignerMode` derives the same partial/send mode from the signer, so
          // nothing else changes.
          authority: signer,
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
    [client, signer]
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
