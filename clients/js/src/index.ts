/**
 * @solfx/client — the public surface.
 *
 * Everything under `generated/` is Codama output and is re-exported as-is. Everything else in
 * this directory exists because the IDL cannot describe it.
 */

// Generated: instructions, account decoders, PDA helpers, errors, types.
export * from "./generated/index.js";

// Hand-written, because an IDL cannot express any of it.
export * from "./constants.js";
export * from "./carry.js";
export * from "./errors.js";
export * from "./margin.js";
export * from "./lp.js";
export * from "./reduce.js";
export * from "./adl.js";
export * from "./contracts.js";
export * from "./sizing.js";
export * from "./pdas.js";
export * from "./priceAccounts.js";
export * from "./events/index.js";
// Hand-written because `solfx_referral` is not in `codama.json`. See referral/index.ts.
export * as referral from "./referral/index.js";
