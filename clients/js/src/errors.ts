/**
 * Making a failed send readable.
 *
 * `@solana/kit`'s transaction-plan executor reports every failure as one sentence:
 *
 *   "The provided transaction plan failed to execute. See the `transactionPlanResult`
 *    attribute for more details. Note that the `cause` property is deprecated..."
 *
 * That is the *low-level* error (`SOLANA_ERROR__INSTRUCTION_PLANS__FAILED_TO_EXECUTE_TRANSACTION_PLAN`),
 * and on its own it says nothing about what went wrong. The real cause is inside
 * `context.transactionPlanResult`, a tree mirroring the plan: nodes of kind `single`,
 * `sequential` or `parallel`, containers holding `plans[]`, and a failed leaf identified by
 * `kind === "single" && status === "failed"` carrying the actual `error`.
 *
 * Two properties of that tree defeat a naive error walker, and both defeated ours:
 *
 * 1. **`transactionPlanResult` is non-enumerable.** It is attached with
 *    `Object.defineProperty(..., { enumerable: false })` deliberately, so that it does not
 *    get serialised with the error. `Object.keys`, spread and `JSON.stringify` all skip it —
 *    it can only be reached by naming it.
 * 2. **The tree branches through arrays.** `plans` is an array, and a walker that only
 *    descends named object properties stops at the first container.
 *
 * The cost of getting this wrong is exactly what this repo's rules warn about: an error that
 * swallows its logs turns a one-line diagnosis into an afternoon. A preflight failure carries
 * the program logs that name `OracleStale`, `SlippageExceeded` or `LeverageTooHigh` outright.
 */

/** Keys worth descending. `transactionPlanResult` must be named — see above. */
const DESCEND = [
  "transactionPlanResult",
  "plans",
  "error",
  "cause",
  "context",
  "data",
  "value",
  "err",
  "abortReason",
  "preflightData",
] as const;

export type SendDiagnosis = {
  /** The most specific message found — the innermost, not the generic wrapper. */
  readonly message: string;
  /** Program logs from a failed preflight, when the chain produced any. */
  readonly logs: readonly string[] | undefined;
};

function isStringArray(v: unknown): v is string[] {
  return Array.isArray(v) && v.every((x) => typeof x === "string");
}

/**
 * Pull the most specific message and the program logs out of anything kit throws.
 *
 * Messages are collected depth-first and the **last** one wins, because the outermost is
 * always the generic wrapper and the innermost is the one that names the real failure.
 */
export function diagnoseSendError(e: unknown): SendDiagnosis {
  const seen = new Set<unknown>();
  const messages: string[] = [];
  let logs: readonly string[] | undefined;

  const walk = (v: unknown): void => {
    if (!v || typeof v !== "object" || seen.has(v)) return;
    seen.add(v);

    if (Array.isArray(v)) {
      for (const item of v) walk(item);
      return;
    }

    const o = v as Record<string, unknown>;
    if (logs === undefined && isStringArray(o.logs)) logs = o.logs;
    if (typeof o.message === "string" && o.message.trim() !== "") messages.push(o.message);

    for (const key of DESCEND) walk(o[key]);
  };

  walk(e);

  const fallback = e instanceof Error ? e.message : String(e);
  return { message: messages[messages.length - 1] ?? fallback, logs };
}
