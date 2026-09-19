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
export * from "./pricing.js";
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

// NOXFUNDS. Codama output from `codama.noxfunds.json`, generated from the IDL **as published on
// chain** (`scripts/fetch-idl.sh noxfunds`), so it describes the program users actually call.
//
// Its own config and its own folder, and namespaced rather than flattened: `codama.json` renders
// with `deleteFolderBeforeRendering`, so sharing it would delete the SolFX client, and both
// programs define names like `Direction` that would collide in one flat namespace.
export * as nox from "./generated-noxfunds/index.js";
// The addresses the IDL cannot describe. See nox/pdas.ts.
export * as noxPdas from "./nox/pdas.js";
