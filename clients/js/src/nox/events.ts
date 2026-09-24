/**
 * NOXFUNDS events, decoded from transaction logs.
 *
 * The codecs are generated from the IDL as published on chain (`npm run generate:events --
 * noxfunds`); the log walk is SolFX's own (`programDataPayloads`), so a NOXFUNDS event is never
 * confused with a SolFX event emitted in the same transaction by the CPI underneath it.
 */
import { NOXFUNDS_PROGRAM_ADDRESS } from "../generated-noxfunds/programs/noxfunds.js";
import { programDataPayloads } from "../events/parse.js";
import { NOX_EVENTS, type NoxEventName } from "./events.generated.js";

export * from "./events.generated.js";

export type NoxEvent = {
  readonly name: NoxEventName;
  readonly data: Record<string, unknown>;
};

const sameBytes = (a: Uint8Array, b: Uint8Array) =>
  a.length >= b.length && b.every((x, i) => x === a[i]);

/**
 * Decode every NOXFUNDS event in a transaction's `logMessages`, in emission order.
 *
 * An event this client cannot read — one added after it was generated, or one whose layout an
 * upgrade changed — is counted in `undecodable` rather than dropped silently: a verification
 * that quietly skips what it cannot read is not a verification.
 */
export function parseNoxEvents(
  logs: readonly string[],
  programAddress: string = NOXFUNDS_PROGRAM_ADDRESS,
): { events: NoxEvent[]; undecodable: number } {
  const events: NoxEvent[] = [];
  let undecodable = 0;
  for (const bytes of programDataPayloads(logs, programAddress)) {
    const ev = NOX_EVENTS.find((e) => sameBytes(bytes, e.discriminator));
    if (!ev) {
      undecodable++;
      continue;
    }
    try {
      events.push({
        name: ev.name,
        data: ev.decoder.decode(bytes.subarray(8)) as Record<string, unknown>,
      });
    } catch {
      undecodable++;
    }
  }
  return { events, undecodable };
}
