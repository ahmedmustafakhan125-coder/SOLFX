/**
 * Turn transaction logs into decoded events.
 *
 * `emit!` writes through the `sol_log_data` syscall, which surfaces as a log line
 * `Program data: <base64>`. The payload is an 8-byte discriminator followed by the Borsh
 * fields. Verified against a real devnet transaction: a 21-byte payload for
 * `MarketStatusChanged`, 8 of discriminator and 13 of body, consumed exactly.
 *
 * # Why the invoke stack is tracked
 *
 * `Program data:` says nothing about which program emitted it. A transaction touching
 * several programs interleaves their logs, and matching on discriminator alone would decode
 * another program's bytes the moment its event happened to collide. So this follows Anchor's
 * own parser: push on `Program <id> invoke [n]`, pop on `success`/`failed`, and only decode
 * while our program is on top.
 */
import { SOLFX_CORE_PROGRAM_ADDRESS } from "../generated/programs/solfxCore.js";
import { SOLFX_EVENTS, type SolfxEventName } from "./generated.js";

const PROGRAM_DATA = "Program data: ";
const INVOKE = /^Program ([1-9A-HJ-NP-Za-km-z]{32,44}) invoke \[\d+\]$/;
const RESULT = /^Program ([1-9A-HJ-NP-Za-km-z]{32,44}) (success|failed.*)$/;

/** A decoded event: its name, and its fields. */
export type SolfxEvent = {
  readonly name: SolfxEventName;
  readonly data: Record<string, unknown>;
};

const sameBytes = (a: Uint8Array, b: Uint8Array) =>
  a.length === b.length && a.every((x, i) => x === b[i]);

function decodeOne(payload: Uint8Array): SolfxEvent | undefined {
  if (payload.length < 8) return undefined;
  const disc = payload.subarray(0, 8);
  for (const ev of SOLFX_EVENTS) {
    if (sameBytes(disc, ev.discriminator)) {
      return {
        name: ev.name,
        data: ev.decoder.decode(payload.subarray(8)) as Record<string, unknown>,
      };
    }
  }
  return undefined; // another program's event, or one added after this client was generated
}

/**
 * Decode every SolFX event in a transaction's `logMessages`, in emission order.
 *
 * Unknown `Program data:` lines are skipped rather than throwing: a client built against an
 * older IDL must still read the events it does understand.
 */
export function parseEvents(
  logs: readonly string[],
  programAddress: string = SOLFX_CORE_PROGRAM_ADDRESS,
): SolfxEvent[] {
  const found: SolfxEvent[] = [];
  const stack: string[] = [];

  for (const line of logs) {
    const invoke = INVOKE.exec(line);
    if (invoke?.[1]) {
      stack.push(invoke[1]);
      continue;
    }
    const result = RESULT.exec(line);
    if (result) {
      stack.pop();
      continue;
    }
    if (!line.startsWith(PROGRAM_DATA)) continue;
    if (stack[stack.length - 1] !== programAddress) continue;

    let bytes: Uint8Array;
    try {
      bytes = Uint8Array.from(Buffer.from(line.slice(PROGRAM_DATA.length), "base64"));
    } catch {
      continue; // not base64; not ours to interpret
    }
    const event = decodeOne(bytes);
    if (event) found.push(event);
  }
  return found;
}

/** Narrow to one event type, keeping the decoded fields typed at the call site. */
export function eventsOfType<T>(events: readonly SolfxEvent[], name: SolfxEventName): T[] {
  return events.filter((e) => e.name === name).map((e) => e.data as T);
}
