/**
 * Target 4: what a trader has already done, and what it came to.
 *
 * Open positions are accounts, so they can be read directly. History is the opposite: a
 * closed position's account is gone, and nothing on chain remembers it. The record lives in
 * the events the program emitted at the time, which survive only in the transaction logs.
 *
 * So this reads the log, not the chain state: signatures for the user's `UserAccount` PDA —
 * which every position instruction touches, and nothing else does — and then each
 * transaction's `logMessages`, decoded by the SDK's own parser.
 *
 * # Why `realizedPnl` is taken as given
 *
 * `PositionDecreased.realized_pnl` is signed and already converted into USDC by the program
 * (correction C-3). A client totalling P&L therefore never has to know whether a market was
 * quoted in JPY or USD, and never has to reconstruct a conversion rate that was current at
 * some past moment and is not recoverable now. Recomputing it here from prices would be both
 * more code and less correct.
 */
import {
  eventsOfType,
  parseEvents,
  type PositionDecreased,
  type PositionLiquidated,
  type PositionOpened,
} from "@solfx/client";
import type { Address, Rpc, Signature, SolanaRpcApi } from "@solana/kit";
import { findUserAccountPda } from "@solfx/client";

import { READ_COMMITMENT } from "@/lib/commitment";

const NOTIONAL_DIVISOR = 1_000_000_000_000n;

/** One thing that happened to a position, in the order it happened. */
export type HistoryRow = {
  readonly signature: string;
  readonly ts: number;
  readonly kind: "Opened" | "Increased" | "Closed" | "Reduced" | "Liquidated";
  readonly marketIndex: number;
  readonly size: bigint;
  readonly price: bigint;
  /** Signed, in USDC. Only a close or a liquidation realises anything. */
  readonly realised: bigint | undefined;
  readonly fee: bigint;
};

/** The totals a trader actually asks for, over the window that was read. */
export type HistoryTotals = {
  readonly realised: bigint;
  readonly fees: bigint;
  /** What actually reached the wallet: realised P&L after the fees paid to get it. */
  readonly net: bigint;
  readonly closes: number;
  readonly wins: number;
  readonly losses: number;
  readonly volume: bigint;
};

export type History = {
  readonly rows: readonly HistoryRow[];
  readonly totals: HistoryTotals;
  /**
   * True when the scan stopped at `limit` rather than at the beginning of the account's
   * life, so the totals cover a window and not the whole history. Shown rather than hidden:
   * a P&L figure that silently omits older trades is worse than one that says it does.
   */
  readonly truncated: boolean;
};

export const EMPTY_TOTALS: HistoryTotals = {
  realised: 0n,
  fees: 0n,
  net: 0n,
  closes: 0,
  wins: 0,
  losses: 0,
  volume: 0n,
};

/**
 * Read the user's trade history from transaction logs.
 *
 * `limit` is an RPC budget as much as a window. The browser shares one endpoint with the
 * poster and the keeper, and this costs one `getSignaturesForAddress` plus one
 * `getTransaction` per signature — so it is called when the tab is opened, not on a poll.
 * A hundred signatures is a few hundred trades' worth of events for a devnet account and
 * still only a couple of seconds of calls.
 */
export async function loadHistory(
  rpc: Rpc<SolanaRpcApi>,
  owner: Address,
  limit = 100
): Promise<History> {
  const [userAccount] = await findUserAccountPda({ authority: owner });

  const signatures = await rpc
    .getSignaturesForAddress(userAccount, {
      commitment: READ_COMMITMENT,
      limit,
    })
    .send();

  const rows: HistoryRow[] = [];

  for (const entry of signatures) {
    // A failed transaction emitted no events; it also cost a fee, but nothing that belongs
    // in a trade history.
    if (entry.err) continue;

    const tx = await rpc
      .getTransaction(entry.signature as Signature, {
        commitment: READ_COMMITMENT,
        maxSupportedTransactionVersion: 0,
        encoding: "json",
      })
      .send();

    const logs = tx?.meta?.logMessages;
    if (!logs) continue;

    const events = parseEvents(logs);
    const signature = entry.signature as string;
    // `blockTime` is null on a node that has pruned it; the events carry their own `ts`,
    // which is the program's own clock and the one the protocol reasoned about anyway.
    const fallbackTs = Number(entry.blockTime ?? 0n);

    for (const e of eventsOfType<PositionOpened>(events, "PositionOpened")) {
      rows.push({
        signature,
        ts: Number(e.ts) || fallbackTs,
        kind: "Opened",
        marketIndex: e.marketIndex,
        size: e.sizeBase,
        price: e.execPrice,
        realised: undefined,
        fee: e.fee,
      });
    }

    for (const e of eventsOfType<PositionDecreased>(
      events,
      "PositionDecreased"
    )) {
      rows.push({
        signature,
        ts: Number(e.ts) || fallbackTs,
        kind: e.fullyClosed ? "Closed" : "Reduced",
        marketIndex: e.marketIndex,
        size: e.sizeClosed,
        price: e.execPrice,
        realised: e.realizedPnl,
        fee: e.fee,
      });
    }

    for (const e of eventsOfType<PositionLiquidated>(
      events,
      "PositionLiquidated"
    )) {
      // A liquidation returns whatever equity survived the penalty. From the trader's side
      // the realised result is that residual less the collateral they had committed, and
      // the collateral is not in this event — so the honest figure here is the residual
      // itself, labelled as such, rather than a P&L reconstructed from a number we do not
      // have.
      rows.push({
        signature,
        ts: Number(e.ts) || fallbackTs,
        kind: "Liquidated",
        marketIndex: e.marketIndex,
        size: e.sizeBase,
        price: e.exitPrice,
        realised: e.residualToOwner - e.equity,
        fee: e.penalty,
      });
    }
  }

  rows.sort((a, b) => b.ts - a.ts);

  let realised = 0n;
  let fees = 0n;
  let closes = 0;
  let wins = 0;
  let losses = 0;
  let volume = 0n;

  for (const r of rows) {
    fees += r.fee;
    volume += (r.size * r.price) / NOTIONAL_DIVISOR;
    if (r.realised === undefined) continue;
    realised += r.realised;
    closes += 1;
    if (r.realised > 0n) wins += 1;
    else if (r.realised < 0n) losses += 1;
  }

  return {
    rows,
    totals: {
      realised,
      fees,
      net: realised - fees,
      closes,
      wins,
      losses,
      volume,
    },
    truncated: signatures.length >= limit,
  };
}
