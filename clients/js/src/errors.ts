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
 * The program's own explanation, taken out of the preflight logs.
 *
 * Anchor logs a refusal in full before the transaction fails:
 *
 *   `Program log: AnchorError caused by account: offer. Error Code: OfferNotOpen.`
 *   `Error Number: 6033. Error Message: Offer is not open.`
 *
 * while kit reports the same failure as `Solana error #4615026` plus a base64 blob and an
 * instruction telling you to run a CLI to decode it. Both were on screen together when an offer
 * was refused for `InsufficientPrincipal`: the sentence that named the cause was collapsed under
 * "Program logs", and the headline was the number.
 *
 * So the log wins. It is the program's own words, it needs no decoder, it survives a production
 * build — a generated `get*ErrorMessage` does not; codama compiles the table out — and it works
 * for both programs, including an error thrown inside a CPI.
 *
 * Anchor splits the line in two when it is long, so the code and the message are matched
 * separately and joined.
 */
function anchorMessage(logs: readonly string[] | undefined): string | undefined {
  if (!logs) return undefined;
  let name: string | undefined;
  let text: string | undefined;
  for (const line of logs) {
    name ??= /Error Code: (\w+)/.exec(line)?.[1];
    text ??= /Error Message: (.+?)\.?\s*$/.exec(line)?.[1];
  }
  if (text) return name ? `${text} (${name})` : text;
  return name ? `Refused: ${name}` : undefined;
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
  // The program's own words first; kit's error code only when the chain said nothing.
  const message =
    anchorMessage(logs) ?? messages[messages.length - 1] ?? fallback;
  return { message, logs };
}
