/**
 * The commitment every chain read in the terminal uses.
 *
 * # Why this is not left to the default
 *
 * `useSend` waits for `confirmed` before it reports a fill and refreshes the panels. Every
 * read, though, was passing no commitment at all, and an RPC node's default is **finalized**.
 * On devnet finalization trails confirmation by roughly 13–32 slots — call it 6–15 seconds,
 * and considerably worse when the cluster is unhappy.
 *
 * So the sequence was: transaction confirms, the panel refreshes immediately, reads a
 * finalized view that does not contain the position yet, and renders nothing. `usePositions`
 * only re-reads when the wallet or the market set changes, so the row then stayed missing
 * until something unrelated happened to retrigger it. The position was open and accruing the
 * whole time — the worst version of that bug, and the same one the `onOpened` callback was
 * added to fix at a different layer.
 *
 * Reading at the commitment we already waited for closes it. `confirmed` can in principle be
 * rolled back; a display that is one dropped fork optimistic is a far smaller problem than a
 * display that hides a live position for fifteen seconds.
 */
export const READ_COMMITMENT = "confirmed" as const;
