/**
 * Re-derive a NOXFUNDS trader's record from the program's events, and compare it with the record
 * the program keeps.
 *
 * `TraderProfile` is what an investor reads to choose a trader, and every figure on it is written
 * by the program. This is the check that the figures are what the events say happened: replay
 * every event the program emitted about the trader, in order, fold them exactly as the program
 * folds them, and see whether the result is the account. Anyone can run it; it needs an RPC
 * endpoint and nothing else.
 *
 * # What it folds, and from where
 *
 * | Field | Source |
 * |---|---|
 * | trades, wins, losses, gross profit/loss, largest win/loss, total hold | `TradeRecorded`, as `record_trade` folds it: zero counts as a loss |
 * | untimed, ambiguous trades | `TradeRecorded` inside a reconcile (a transaction with `PositionReconciled` for that mandate); ambiguous when that reconcile closed more than one |
 * | max drawdown | `EquityObserved` on the trader's mandates, the largest, saturating at `u16::MAX` |
 * | mandates funded, active | `MandateFunded` and `MandateSettled` |
 * | settled in profit | `MandateSettled` for a mandate whose trades summed above zero |
 * | tier | the last `TierChanged` |
 *
 * `TradeRecorded` also carries the program's running totals after each trade, and every one of
 * them is checked against the replay as it goes — so a disagreement is pinned to the transaction
 * where it began, not just reported at the end.
 *
 * # What cannot be proven from events
 *
 * A reconcile that closed several positions at once and netted a **profit** records each of them
 * at zero, against the trader (see `reconcile_position`), while the mandate's own running result
 * takes the true total. That total is in no event. So "settled in profit" for such a mandate is
 * reported as unprovable rather than guessed.
 */
import type { Address, Rpc, Signature, SolanaRpcApi } from "@solana/kit";

import { parseNoxEvents, type NoxEvent } from "./events.js";

/** One transaction that touched the profile, as the RPC returned it. */
export type ProfileTx = {
  readonly signature: string;
  readonly slot: bigint;
  readonly blockTime: bigint | null;
  readonly logs: readonly string[];
  /** A failed transaction's logs can still carry events emitted before it failed. */
  readonly failed: boolean;
};

export type Derived = {
  trades: number;
  wins: number;
  losses: number;
  grossProfit: bigint;
  grossLoss: bigint;
  largestWin: bigint;
  largestLoss: bigint;
  totalHoldSlots: bigint;
  untimedTrades: number;
  ambiguousTrades: number;
  maxDrawdownBps: number;
  mandatesFunded: number;
  activeMandates: number;
  mandatesSettledInProfit: number;
  /** `null` when the tier never changed, which means it is still the default, Bronze (0). */
  lastTier: number | null;
};

export type TrailEntry = {
  readonly signature: string;
  readonly blockTime: bigint | null;
  readonly name: string;
  readonly data: Record<string, unknown>;
};

export type Rederivation = {
  derived: Derived;
  /** Every event that moved the record, oldest first, with the transaction that carried it. */
  trail: TrailEntry[];
  /** The first transaction whose `TradeRecorded` running totals disagreed with the replay. */
  firstDisagreement: { signature: string; field: string; event: string; replay: string } | null;
  undecodable: number;
  failedSkipped: number;
  /** Mandates whose "settled in profit" status the events cannot establish. See the header. */
  unprovable: Address[];
};

const U16_MAX = 65_535;

/**
 * Fold `txs` (oldest first) into a trader's record. Pure: the same transactions always give the
 * same answer, which is what makes it a verification rather than a second opinion.
 */
export function rederiveProfile(
  profile: Address,
  trader: Address,
  txs: readonly ProfileTx[],
): Rederivation {
  const d: Derived = {
    trades: 0,
    wins: 0,
    losses: 0,
    grossProfit: 0n,
    grossLoss: 0n,
    largestWin: 0n,
    largestLoss: 0n,
    totalHoldSlots: 0n,
    untimedTrades: 0,
    ambiguousTrades: 0,
    maxDrawdownBps: 0,
    mandatesFunded: 0,
    activeMandates: 0,
    mandatesSettledInProfit: 0,
    lastTier: null,
  };
  const trail: TrailEntry[] = [];
  const mandates = new Set<string>();
  const mandatePnl = new Map<string, bigint>();
  const ambiguousProfit = new Set<string>();
  let firstDisagreement: Rederivation["firstDisagreement"] = null;
  let undecodable = 0;
  let failedSkipped = 0;

  for (const tx of txs) {
    if (tx.failed) {
      failedSkipped++;
      continue;
    }
    const parsed = parseNoxEvents(tx.logs);
    undecodable += parsed.undecodable;
    const events = parsed.events;

    // Reconciles in this transaction, per mandate: how many positions each closed.
    const reconciled = new Map<string, number>();
    for (const e of events) {
      if (e.name !== "PositionReconciled") continue;
      const m = String(e.data.mandate);
      reconciled.set(m, (reconciled.get(m) ?? 0) + 1);
    }
    // Per mandate, what the reconcile recorded in total — to spot an ambiguous batch whose
    // true result (a profit) the events do not carry.
    const reconciledRecorded = new Map<string, bigint>();

    const keep = (e: NoxEvent) =>
      trail.push({ signature: tx.signature, blockTime: tx.blockTime, name: e.name, data: e.data });

    for (const e of events) {
      const data = e.data;
      switch (e.name) {
        case "TradeRecorded": {
          if (String(data.profile) !== profile) break;
          const pnl = data.realizedPnl as bigint;
          const hold = data.holdSlots as bigint;
          const mandate = String(data.mandate);
          d.trades++;
          d.totalHoldSlots += hold;
          if (pnl > 0n) {
            d.wins++;
            d.grossProfit += pnl;
            if (pnl > d.largestWin) d.largestWin = pnl;
          } else {
            const loss = -pnl;
            d.losses++;
            d.grossLoss += loss;
            if (loss > d.largestLoss) d.largestLoss = loss;
          }
          mandatePnl.set(mandate, (mandatePnl.get(mandate) ?? 0n) + pnl);
          const closedTogether = reconciled.get(mandate) ?? 0;
          if (closedTogether > 0) {
            d.untimedTrades++;
            if (closedTogether > 1) d.ambiguousTrades++;
            reconciledRecorded.set(mandate, (reconciledRecorded.get(mandate) ?? 0n) + pnl);
          }
          if (!firstDisagreement) {
            const checks: [string, unknown, unknown][] = [
              ["trades", data.trades, d.trades],
              ["wins", data.wins, d.wins],
              ["losses", data.losses, d.losses],
              ["grossProfit", data.grossProfit, d.grossProfit],
              ["grossLoss", data.grossLoss, d.grossLoss],
            ];
            for (const [field, event, replay] of checks) {
              if (String(event) !== String(replay)) {
                firstDisagreement = {
                  signature: tx.signature,
                  field,
                  event: String(event),
                  replay: String(replay),
                };
                break;
              }
            }
          }
          keep(e);
          break;
        }
        case "EquityObserved": {
          if (!mandates.has(String(data.mandate))) break;
          const bps = data.drawdownBps as bigint;
          const capped = bps > BigInt(U16_MAX) ? U16_MAX : Number(bps);
          if (capped > d.maxDrawdownBps) {
            d.maxDrawdownBps = capped;
            keep(e);
          }
          break;
        }
        case "MandateFunded": {
          if (String(data.trader) !== trader) break;
          mandates.add(String(data.mandate));
          d.mandatesFunded++;
          d.activeMandates++;
          keep(e);
          break;
        }
        case "MandateSettled": {
          if (String(data.trader) !== trader) break;
          const m = String(data.mandate);
          d.activeMandates = Math.max(0, d.activeMandates - 1);
          if ((mandatePnl.get(m) ?? 0n) > 0n) d.mandatesSettledInProfit++;
          keep(e);
          break;
        }
        case "TierChanged": {
          if (String(data.profile) !== profile) break;
          d.lastTier = Number(data.to);
          keep(e);
          break;
        }
        default:
          break;
      }
    }

    // A batch of two or more, recorded at zero in total: the program credited nothing, and
    // whether the true total was a profit is not in any event.
    for (const [m, count] of reconciled) {
      if (count > 1 && reconciledRecorded.get(m) === 0n) ambiguousProfit.add(m);
    }
  }

  return {
    derived: d,
    trail,
    firstDisagreement,
    undecodable,
    failedSkipped,
    unprovable: [...ambiguousProfit] as Address[],
  };
}

/** One row of the comparison: the account's figure beside the replay's. */
export type Comparison = {
  readonly field: string;
  readonly onChain: string;
  readonly replayed: string;
  readonly agrees: boolean;
};

/** The profile fields this module re-derives, as the generated client names them. */
export type ProfileFigures = {
  tier: number;
  trades: number;
  wins: number;
  losses: number;
  grossProfit: bigint;
  grossLoss: bigint;
  largestWin: bigint;
  largestLoss: bigint;
  totalHoldSlots: bigint;
  maxDrawdownBps: number;
  mandatesFunded: number;
  activeMandates: number;
  mandatesSettledInProfit: number;
  untimedTrades: number;
  ambiguousTrades: number;
};

export function compareProfile(onChain: ProfileFigures, d: Derived): Comparison[] {
  const row = (field: string, a: unknown, b: unknown): Comparison => ({
    field,
    onChain: String(a),
    replayed: String(b),
    agrees: String(a) === String(b),
  });
  return [
    row("trades", onChain.trades, d.trades),
    row("wins", onChain.wins, d.wins),
    row("losses", onChain.losses, d.losses),
    row("grossProfit", onChain.grossProfit, d.grossProfit),
    row("grossLoss", onChain.grossLoss, d.grossLoss),
    row("largestWin", onChain.largestWin, d.largestWin),
    row("largestLoss", onChain.largestLoss, d.largestLoss),
    row("totalHoldSlots", onChain.totalHoldSlots, d.totalHoldSlots),
    row("untimedTrades", onChain.untimedTrades, d.untimedTrades),
    row("ambiguousTrades", onChain.ambiguousTrades, d.ambiguousTrades),
    row("maxDrawdownBps", onChain.maxDrawdownBps, d.maxDrawdownBps),
    row("mandatesFunded", onChain.mandatesFunded, d.mandatesFunded),
    row("activeMandates", onChain.activeMandates, d.activeMandates),
    row("mandatesSettledInProfit", onChain.mandatesSettledInProfit, d.mandatesSettledInProfit),
    row("tier", onChain.tier, d.lastTier ?? 0),
  ];
}

/**
 * Every transaction that touched `profile`, oldest first, with its logs.
 *
 * Every instruction that changes a trader's record takes the profile account — the closes, the
 * crank, the reconcile, funding, settlement and the tier — so its signature list is the complete
 * history, and nothing has to be trusted to point at the right transactions.
 *
 * `concurrency` bounds the `getTransaction` calls in flight; a public endpoint rate-limits well
 * before a long history is fetched otherwise.
 */
/**
 * Retry a rate-limited call with backoff. A long history is hundreds of `getTransaction` calls,
 * and every public endpoint answers 429 long before that — measured against
 * api.devnet.solana.com on 2026-09-24, which refused the first run of this check outright.
 */
async function withRetry<T>(call: () => Promise<T>, attempts = 6): Promise<T> {
  let wait = 500;
  for (let i = 1; ; i++) {
    try {
      return await call();
    } catch (e) {
      const text = String((e as { message?: string })?.message ?? e);
      if (i >= attempts || !/429|Too Many Requests/i.test(text)) throw e;
      await new Promise((r) => setTimeout(r, wait));
      wait *= 2;
    }
  }
}

export async function fetchProfileHistory(
  rpc: Rpc<SolanaRpcApi>,
  profile: Address,
  opts: { concurrency?: number; onProgress?: (done: number, total: number) => void } = {},
): Promise<ProfileTx[]> {
  const sigs: { signature: Signature; slot: bigint; blockTime: bigint | null; failed: boolean }[] =
    [];
  let before: Signature | undefined;
  for (;;) {
    const page = await withRetry(() =>
      rpc.getSignaturesForAddress(profile, { limit: 1000, before, commitment: "confirmed" }).send(),
    );
    for (const s of page) {
      sigs.push({
        signature: s.signature,
        slot: s.slot,
        blockTime: s.blockTime === null ? null : BigInt(s.blockTime),
        failed: s.err !== null,
      });
    }
    if (page.length < 1000) break;
    before = page[page.length - 1]?.signature;
    if (!before) break;
  }
  sigs.reverse(); // the RPC returns newest first; the fold needs oldest first

  const out: ProfileTx[] = new Array(sigs.length);
  let next = 0;
  let done = 0;
  const worker = async () => {
    for (;;) {
      const i = next++;
      const s = sigs[i];
      if (!s) return;
      if (s.failed) {
        out[i] = { ...s, logs: [] };
      } else {
        const tx = await withRetry(() =>
          rpc
            .getTransaction(s.signature, {
              commitment: "confirmed",
              maxSupportedTransactionVersion: 0,
              encoding: "json",
            })
            .send(),
        );
        out[i] = {
          ...s,
          logs: tx?.meta?.logMessages ?? [],
          failed: tx?.meta?.err != null,
        };
      }
      opts.onProgress?.(++done, sigs.length);
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, opts.concurrency ?? 3) }, worker));
  return out;
}
