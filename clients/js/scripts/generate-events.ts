/**
 * Generate typed decoders for the program's events.
 *
 * Codama emits nothing for events: it only renders types something references, and no
 * instruction or account references an event struct. The IDL carries both the discriminators
 * and the full layouts, so this reads them and emits the decoders Codama skipped.
 *
 * Output goes to `src/events/generated.ts`, deliberately NOT under `src/generated/` — Codama
 * runs with `deleteFolderBeforeRendering`, so anything left there is destroyed on the next
 * `npm run generate`.
 *
 * Every codec below is the one Codama itself emits for that field type elsewhere in this
 * client, so the two halves cannot decode the same bytes differently.
 */
import { createHash } from "node:crypto";
import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, "..", "..", "..");
const idlPath = path.join(root, "target", "idl", "solfx_core.json");
const outPath = path.join(here, "..", "src", "events", "generated.ts");

type IdlType = string | { array: [IdlType, number] } | Record<string, unknown>;
type Field = { name: string; type: IdlType };
type Def = { name: string; type: { kind: string; fields?: Field[] } };
type Idl = {
  events: { name: string; discriminator: number[] }[];
  types: Def[];
};

const idl = JSON.parse(readFileSync(idlPath, "utf8")) as Idl;

const camel = (s: string) => s.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());

/** Maps an IDL field type to [TypeScript type, decoder expression, imports used]. */
function mapType(t: IdlType): { ts: string; dec: string; imports: string[] } {
  if (typeof t === "string") {
    switch (t) {
      case "u8":
        return { ts: "number", dec: "getU8Decoder()", imports: ["getU8Decoder"] };
      case "u16":
        return { ts: "number", dec: "getU16Decoder()", imports: ["getU16Decoder"] };
      case "u32":
        return { ts: "number", dec: "getU32Decoder()", imports: ["getU32Decoder"] };
      case "u64":
        return { ts: "bigint", dec: "getU64Decoder()", imports: ["getU64Decoder"] };
      case "u128":
        return { ts: "bigint", dec: "getU128Decoder()", imports: ["getU128Decoder"] };
      case "i64":
        return { ts: "bigint", dec: "getI64Decoder()", imports: ["getI64Decoder"] };
      case "i128":
        return { ts: "bigint", dec: "getI128Decoder()", imports: ["getI128Decoder"] };
      case "bool":
        return { ts: "boolean", dec: "getBooleanDecoder()", imports: ["getBooleanDecoder"] };
      case "pubkey":
        return { ts: "Address", dec: "getAddressDecoder()", imports: ["getAddressDecoder"] };
      case "string":
        return {
          ts: "string",
          dec: "addDecoderSizePrefix(getUtf8Decoder(), getU32Decoder())",
          imports: ["addDecoderSizePrefix", "getUtf8Decoder", "getU32Decoder"],
        };
    }
  } else if (typeof t === "object" && "array" in t) {
    const [inner, len] = (t as { array: [IdlType, number] }).array;
    if (inner === "u8") {
      return {
        ts: "ReadonlyUint8Array",
        dec: `fixDecoderSize(getBytesDecoder(), ${len})`,
        imports: ["fixDecoderSize", "getBytesDecoder"],
      };
    }
  }
  // Fail loudly. A guessed layout decodes to plausible nonsense.
  throw new Error(`unsupported event field type: ${JSON.stringify(t)}`);
}

const layouts = new Map(idl.types.map((t) => [t.name, t]));
const imports = new Set<string>(["type Decoder", "getStructDecoder", "fixDecoderSize", "getBytesDecoder"]);
const blocks: string[] = [];
const registry: string[] = [];

for (const ev of idl.events) {
  const def = layouts.get(ev.name);
  if (!def?.type.fields) throw new Error(`event ${ev.name} has no layout in the IDL`);

  // Trust but verify: the IDL's discriminator must be Anchor's derivation.
  const derived = Array.from(createHash("sha256").update(`event:${ev.name}`).digest()).slice(0, 8);
  if (JSON.stringify(derived) !== JSON.stringify(ev.discriminator)) {
    throw new Error(`${ev.name}: IDL discriminator is not sha256("event:${ev.name}")[..8]`);
  }

  const fields = def.type.fields.map((f) => {
    const m = mapType(f.type);
    for (const i of m.imports) imports.add(i);
    if (m.ts === "Address") imports.add("type Address");
    if (m.ts === "ReadonlyUint8Array") imports.add("type ReadonlyUint8Array");
    return { name: camel(f.name), ...m };
  });

  blocks.push(
    `export type ${ev.name} = {\n` +
      fields.map((f) => `  ${f.name}: ${f.ts};`).join("\n") +
      `\n};\n\n` +
      `export const ${toConst(ev.name)}_DISCRIMINATOR = new Uint8Array([${ev.discriminator.join(", ")}]);\n\n` +
      `export function get${ev.name}Decoder(): Decoder<${ev.name}> {\n` +
      `  return getStructDecoder([\n` +
      fields.map((f) => `    ["${f.name}", ${f.dec}],`).join("\n") +
      `\n  ]);\n}\n`,
  );

  registry.push(
    `  { name: "${ev.name}" as const, discriminator: ${toConst(ev.name)}_DISCRIMINATOR, decoder: get${ev.name}Decoder() },`,
  );
}

function toConst(name: string): string {
  return name.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toUpperCase();
}

const named = [...imports].filter((i) => !i.startsWith("type ")).sort();
const typed = [...imports].filter((i) => i.startsWith("type ")).sort();

const out = `/**
 * GENERATED by scripts/generate-events.ts from target/idl/solfx_core.json. DO NOT EDIT.
 *
 * Codama does not render event structs — it only emits types that an instruction or account
 * references, and nothing references an event. Every codec here is the same one Codama emits
 * for that field type elsewhere in this client.
 *
 * ${idl.events.length} events.
 */
import {
${[...named, ...typed].map((i) => `  ${i},`).join("\n")}
} from "@solana/kit";

${blocks.join("\n")}
/** Every event, for dispatch by discriminator. */
export const SOLFX_EVENTS = [
${registry.join("\n")}
] as const;

export type SolfxEventName = (typeof SOLFX_EVENTS)[number]["name"];
`;

mkdirSync(path.dirname(outPath), { recursive: true });
writeFileSync(outPath, out);
console.log(`wrote ${path.relative(process.cwd(), outPath)} — ${idl.events.length} events`);
