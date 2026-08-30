// Decodes the events from the real BTC round trip on devnet.
import { parseEvents } from "../src/events/parse.js";

const rpcUrl = process.env.SOLFX_RPC_URL;
if (!rpcUrl) throw new Error("SOLFX_RPC_URL is not set");

const SIGS: Record<string, string> = {
  open: "ReBgxzFqVNSUQnFS2FMrtCUeJGPdAmAYne6Rrq6AD3eiZPR2prLMkTuQiYgcQ92WbqPmdCMdJqDqaCNvmroNXGi",
  close: "56RRDkEwQ4diAfGXaUXHwNPSMzCJSkhiV7kP7cx67DGWRxuGuB4gN3ms1GigjF79fWw2ku9oZzJWuwAxtS9bP2W1",
};

for (const [label, sig] of Object.entries(SIGS)) {
  const res = await fetch(rpcUrl, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getTransaction",
      params: [sig, { encoding: "json", maxSupportedTransactionVersion: 0, commitment: "confirmed" }],
    }),
  });
  const json = (await res.json()) as { result?: { meta?: { logMessages?: string[] } } };
  const logs = json.result?.meta?.logMessages ?? [];
  const events = parseEvents(logs);
  console.log(`\n=== ${label} (${events.length} event${events.length === 1 ? "" : "s"}) ===`);
  for (const e of events) {
    console.log(`  ${e.name}`);
    for (const [k, v] of Object.entries(e.data)) {
      const shown = v instanceof Uint8Array ? Buffer.from(v).toString("hex") : String(v);
      console.log(`    ${k.padEnd(22)} ${shown}`);
    }
  }
}
