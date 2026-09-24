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
import {
  getAddressDecoder,
  getBooleanDecoder,
  getI64Decoder,
  getStructDecoder,
  getU8Decoder,
  getU16Decoder,
  getU64Decoder,
  type Decoder,
} from "@solana/kit";

import { SOLFX_CORE_PROGRAM_ADDRESS } from "../generated/programs/solfxCore.js";
import {
  POSITION_DECREASED_DISCRIMINATOR,
  SOLFX_EVENTS,
  TRIGGER_ORDER_EXECUTED_DISCRIMINATOR,
  type SolfxEventName,
} from "./generated.js";

const PROGRAM_DATA = "Program data: ";
const INVOKE = /^Program ([1-9A-HJ-NP-Za-km-z]{32,44}) invoke \[\d+\]$/;
const RESULT = /^Program ([1-9A-HJ-NP-Za-km-z]{32,44}) (success|failed.*)$/;

/** A decoded event: its name, and its fields. */
export type SolfxEvent = {
  readonly name: SolfxEventName;
  readonly data: Record<string, unknown>;
};

/**
 * Base64 to bytes, in whichever runtime this is.
 *
 * `Buffer` is Node-only, and this parser has to run in the browser too — the terminal reads
 * its own trade history straight from transaction logs. `atob` is the web platform's decoder
 * and exists in every browser; Node has had it since 16, but `Buffer` is still preferred
 * there because `atob` is deprecated in that runtime. Neither branch throws on bad input in
 * the same way, so the caller keeps its try/catch.
 */
function fromBase64(b64: string): Uint8Array {
  if (typeof Buffer !== "undefined") return Uint8Array.from(Buffer.from(b64, "base64"));
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const sameBytes = (a: Uint8Array, b: Uint8Array) =>
  a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * The two events as they were emitted **before** the Phase 10 upgrade, which inserted
 * `entry_price` into both.
 *
 * A transaction is a permanent record: every close and every fired stop from before that
 * upgrade is still on chain in the old shape, and the current codec runs out of bytes trying
 * to read the field that was not there yet. The whole history panel died on it with
 * `SOLANA_ERROR__CODECS__...` naming an `i64` — one old trade made every newer one
 * unreadable too.
 *
 * `entryPrice` comes back as `0n`. Nothing in the history or summary reads it (the positions
 * table takes entry price from the position account), so a zero is honest about what the old
 * event does not carry rather than a guess at what it might have been.
 */
const LEGACY_DECODERS: ReadonlyArray<{
  readonly discriminator: Uint8Array;
  readonly decoder: Decoder<Record<string, unknown>>;
}> = [
  {
    discriminator: POSITION_DECREASED_DISCRIMINATOR,
    decoder: getStructDecoder([
      ["position", getAddressDecoder()],
      ["userAccount", getAddressDecoder()],
      ["marketIndex", getU16Decoder()],
      ["sizeClosed", getU64Decoder()],
      ["remainingSize", getU64Decoder()],
      ["oraclePrice", getI64Decoder()],
      ["execPrice", getI64Decoder()],
      ["realizedPnl", getI64Decoder()],
      ["fee", getU64Decoder()],
      ["collateralReturned", getU64Decoder()],
      ["fullyClosed", getBooleanDecoder()],
      ["ts", getI64Decoder()],
    ]) as Decoder<Record<string, unknown>>,
  },
  {
    discriminator: TRIGGER_ORDER_EXECUTED_DISCRIMINATOR,
    decoder: getStructDecoder([
      ["order", getAddressDecoder()],
      ["position", getAddressDecoder()],
      ["keeper", getAddressDecoder()],
      ["marketIndex", getU16Decoder()],
      ["orderId", getU8Decoder()],
      ["kind", getU8Decoder()],
      ["triggerPrice", getI64Decoder()],
      ["oraclePrice", getI64Decoder()],
      ["sizeBase", getU64Decoder()],
      ["keeperTipLamports", getU64Decoder()],
      ["ts", getI64Decoder()],
    ]) as Decoder<Record<string, unknown>>,
  },
];

function decodeLegacy(disc: Uint8Array, body: Uint8Array): Record<string, unknown> | undefined {
  for (const legacy of LEGACY_DECODERS) {
    if (!sameBytes(disc, legacy.discriminator)) continue;
    try {
      return { ...legacy.decoder.decode(body), entryPrice: 0n };
    } catch {
      return undefined;
    }
  }
  return undefined;
}

function decodeOne(payload: Uint8Array): SolfxEvent | undefined {
  if (payload.length < 8) return undefined;
  const disc = payload.subarray(0, 8);
  for (const ev of SOLFX_EVENTS) {
    if (!sameBytes(disc, ev.discriminator)) continue;
    const body = payload.subarray(8);
    try {
      return { name: ev.name, data: ev.decoder.decode(body) as Record<string, unknown> };
    } catch {
      // An event whose shape changed in an upgrade, or one this client cannot read. Try the
      // older layout, and failing that skip it — **one undecodable event must never cost the
      // caller every other event in the transaction.**
      const data = decodeLegacy(disc, body);
      return data ? { name: ev.name, data } : undefined;
    }
  }
  return undefined; // another program's event, or one added after this client was generated
}

/**
 * Every `Program data:` payload one program emitted in a transaction's `logMessages`, in
 * emission order, following the invoke stack as described at the top of this file.
 *
 * Exported so another program's events (NOXFUNDS') are attributed by the same walk rather than
 * a second copy of it that drifts.
 */
export function programDataPayloads(logs: readonly string[], programAddress: string): Uint8Array[] {
  const found: Uint8Array[] = [];
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

    try {
      found.push(fromBase64(line.slice(PROGRAM_DATA.length)));
    } catch {
      continue; // not base64; not ours to interpret
    }
  }
  return found;
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
  for (const bytes of programDataPayloads(logs, programAddress)) {
    const event = decodeOne(bytes);
    if (event) found.push(event);
  }
  return found;
}

/** Narrow to one event type, keeping the decoded fields typed at the call site. */
export function eventsOfType<T>(events: readonly SolfxEvent[], name: SolfxEventName): T[] {
  return events.filter((e) => e.name === name).map((e) => e.data as T);
}
