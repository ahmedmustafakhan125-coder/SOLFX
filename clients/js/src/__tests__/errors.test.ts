import { describe, expect, it } from "vitest";
import { diagnoseSendError } from "../errors.js";

/**
 * The structures here mirror what `@solana/kit`'s executor actually throws, per
 * `createFailedToExecuteTransactionPlanError`: a generic outer message, the real cause buried
 * in a plan-result tree, and `transactionPlanResult` attached **non-enumerably**.
 */
function planError(result: unknown): Error {
  const err = new Error(
    "The provided transaction plan failed to execute. See the `transactionPlanResult` " +
      "attribute for more details.",
  );
  const context: Record<string, unknown> = {};
  Object.defineProperty(context, "transactionPlanResult", {
    configurable: false,
    enumerable: false,
    value: result,
    writable: false,
  });
  (err as unknown as { context: unknown }).context = context;
  return err;
}

describe("diagnoseSendError", () => {
  it("reaches through the non-enumerable transactionPlanResult", () => {
    // The property is deliberately hidden from serialisation, so a walker that enumerates
    // keys finds nothing at all. This is the case that produced the unreadable error.
    const e = planError({
      kind: "single",
      status: "failed",
      error: { message: "Attempt to debit an account but found no record of a prior credit." },
    });
    expect(Object.keys((e as unknown as { context: object }).context)).toEqual([]);
    expect(diagnoseSendError(e).message).toBe(
      "Attempt to debit an account but found no record of a prior credit.",
    );
  });

  it("descends through the plans array of a container node", () => {
    const e = planError({
      kind: "sequential",
      plans: [
        { kind: "single", status: "successful" },
        {
          kind: "single",
          status: "failed",
          error: {
            message: "Transaction simulation failed",
            context: { logs: ["Program log: Instruction: OpenPosition", "Program log: OracleStale"] },
          },
        },
      ],
    });
    const d = diagnoseSendError(e);
    expect(d.logs).toEqual([
      "Program log: Instruction: OpenPosition",
      "Program log: OracleStale",
    ]);
    expect(d.message).toBe("Transaction simulation failed");
  });

  it("prefers the innermost message over the generic wrapper", () => {
    const e = planError({
      kind: "single",
      status: "failed",
      error: { message: "outer", cause: { message: "the one that actually explains it" } },
    });
    expect(diagnoseSendError(e).message).toBe("the one that actually explains it");
  });

  it("falls back to the thrown error's own message when there is no tree", () => {
    expect(diagnoseSendError(new Error("wallet rejected the request")).message).toBe(
      "wallet rejected the request",
    );
    expect(diagnoseSendError("a bare string").message).toBe("a bare string");
  });

  it("survives a cycle rather than recursing forever", () => {
    const a: Record<string, unknown> = { message: "inner" };
    a.cause = a;
    expect(diagnoseSendError({ message: "outer", cause: a }).message).toBe("inner");
  });
});

/**
 * The real failure from devnet, 20 Sep 2026: an offer of $1,000 from a wallet holding $500.
 *
 * Kit reported `Solana error #4615026` plus a base64 blob and an instruction to run a CLI to
 * decode it, while the program had already said what was wrong, in words, in the logs. The
 * headline should be the program's sentence, not the number.
 */
describe("diagnoseSendError, with program logs", () => {
  const anchorLogs = [
    "Program 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx invoke [1]",
    "Program log: Instruction: PostOffer",
    "Program log: AnchorError thrown in programs/noxfunds/src/instructions/marketplace.rs:312. " +
      "Error Code: InsufficientPrincipal. Error Number: 6030. " +
      "Error Message: The investor's token account holds less than the principal.",
    "Program 9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx failed: custom program error: 0x178e",
  ];

  it("prefers the program's own sentence over kit's error number", () => {
    const e = planError({
      kind: "single",
      status: "failed",
      error: {
        message:
          'Solana error #4615026; Decode this error by running `npx @solana/errors decode -- 4615026 "X19jb2RlPTQ2MTUwMjYmY29kZT02MDMwJmluZGV4PTI="`',
        context: { logs: anchorLogs },
      },
    });
    const { message, logs } = diagnoseSendError(e);
    expect(message).toBe(
      "The investor's token account holds less than the principal (InsufficientPrincipal)",
    );
    expect(logs).toEqual(anchorLogs);
  });

  it("names the error code when Anchor split the line and only the code survived", () => {
    const e = planError({
      kind: "single",
      status: "failed",
      error: {
        message: "Solana error #4615026",
        context: { logs: ["Program log: Error Code: OfferNotOpen. Error Number: 6033."] },
      },
    });
    expect(diagnoseSendError(e).message).toBe("Refused: OfferNotOpen");
  });

  it("falls back to the innermost message when the chain produced no logs", () => {
    const e = planError({
      kind: "single",
      status: "failed",
      error: { message: "User rejected the request." },
    });
    expect(diagnoseSendError(e).message).toBe("User rejected the request.");
  });
});
