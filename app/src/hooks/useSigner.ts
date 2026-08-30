import { useMemo } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import { createWalletTransactionSigner, isWalletSession } from "@solana/client";
import type { TransactionSigner } from "@solana/kit";

/**
 * The connected wallet as a `TransactionSigner`.
 *
 * The instruction builders need one: it is what marks the `authority` account meta as a
 * signer and supplies the address the program derives `UserAccount` from. A wallet session
 * is not a signer, and casting one into the slot would compile and then build an instruction
 * with the wrong account metas — so this converts properly and guards the conversion.
 */
export function useSigner(): TransactionSigner | undefined {
  const { wallet } = useWalletConnection();
  return useMemo(() => {
    if (!wallet || !isWalletSession(wallet)) return undefined;
    // Returns { mode, signer }: `mode` says whether the wallet can partial-sign, `signer` is
    // the TransactionSigner the instruction builders want.
    return createWalletTransactionSigner(wallet).signer;
  }, [wallet]);
}
