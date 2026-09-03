import { useMemo } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import { createWalletTransactionSigner, isWalletSession } from "@solana/client";
import type { TransactionSigner } from "@solana/kit";

/**
 * One signer instance per wallet session, for the lifetime of that session.
 *
 * This cache is the whole point of the module, and it is not an optimisation.
 * `@solana/kit` requires that an address be represented by **the same signer object**
 * everywhere it appears in a transaction; two distinct objects for one address fail with
 * "Multiple distinct signers were identified for address ...".
 *
 * `useMemo` alone cannot provide that. A memo cache belongs to one component instance, so
 * `AccountPanel` calling `useSigner()` and `useSend()` calling it again produce two separate
 * caches and therefore two distinct signers — for the same wallet, the same address, in the
 * same transaction. Keying on the session object instead makes identity global and stable.
 *
 * A `WeakMap` so a disconnected session is collectable rather than pinned forever.
 */
const cache = new WeakMap<object, TransactionSigner>();

/**
 * The connected wallet as a `TransactionSigner`.
 *
 * The instruction builders need one: it is what marks the `authority` account meta as a
 * signer and supplies the address the program derives `UserAccount` from. A wallet session
 * is not a signer, and casting one into the slot would compile and then build an instruction
 * with the wrong account metas — so this converts properly and guards the conversion.
 *
 * Every caller gets the *same* instance for a given session. See the cache above for why
 * that is a correctness requirement rather than a nicety.
 */
export function useSigner(): TransactionSigner | undefined {
  const { wallet } = useWalletConnection();
  return useMemo(() => {
    if (!wallet || !isWalletSession(wallet)) return undefined;
    const existing = cache.get(wallet);
    if (existing) return existing;
    // Returns { mode, signer }: `mode` says whether the wallet can partial-sign, `signer` is
    // the TransactionSigner the instruction builders want.
    const signer = createWalletTransactionSigner(wallet).signer;
    cache.set(wallet, signer);
    return signer;
  }, [wallet]);
}
