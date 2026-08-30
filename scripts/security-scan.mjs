// Run Ackee Blockchain Security's Solana detectors over the workspace, headlessly.
//
// The scanner ships as a VS Code extension (ackeeblockchain.solana) whose only interface is
// a language server speaking LSP over stdio — there is no CLI. This drives it the way the
// extension does: initialize, then executeCommand("solana.scanWorkspace"), then collect
// publishDiagnostics and the custom solana/scanComplete notification.
//
//   ACKEE_BIN=~/.cursor-server/extensions/ackeeblockchain.solana-*/bin/language-server \
//     node scripts/security-scan.mjs .
//
// Read the output against the codebase rather than counting it. The detectors do not see
// crate-level lint configuration, cannot resolve an `Accounts` struct defined in another
// file, and treat a guarded division as unchecked arithmetic — so the raw total is
// dominated by findings this workspace already handles by construction. See
// docs/security-scan.md for the triage of the first run.
import { spawn } from "node:child_process";
import { pathToFileURL } from "node:url";

const BIN = process.env.ACKEE_BIN;
const ROOT = process.argv[2] ?? process.cwd();
const rootUri = pathToFileURL(ROOT).href;

const srv = spawn(BIN, [], { stdio: ["pipe", "pipe", "pipe"] });
let nextId = 1;
const diagnostics = new Map();
let complete = null;

function send(msg) {
  const body = JSON.stringify({ jsonrpc: "2.0", ...msg });
  srv.stdin.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
}

let buf = Buffer.alloc(0);
srv.stdout.on("data", (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  for (;;) {
    const sep = buf.indexOf("\r\n\r\n");
    if (sep < 0) return;
    const header = buf.subarray(0, sep).toString();
    const m = /Content-Length: (\d+)/i.exec(header);
    if (!m) return;
    const len = Number(m[1]);
    const start = sep + 4;
    if (buf.length < start + len) return;
    const msg = JSON.parse(buf.subarray(start, start + len).toString());
    buf = buf.subarray(start + len);
    handle(msg);
  }
});

srv.stderr.on("data", (d) => {
  const s = d.toString().trim();
  if (s && process.env.ACKEE_VERBOSE) console.error("[stderr]", s);
});

function handle(msg) {
  if (msg.method === "textDocument/publishDiagnostics") {
    const { uri, diagnostics: ds } = msg.params;
    if (ds?.length) diagnostics.set(uri, ds);
  } else if (msg.method === "solana/scanComplete") {
    complete = msg.params;
  } else if (msg.method === "window/logMessage" && process.env.ACKEE_VERBOSE) {
    console.error("[log]", msg.params.message);
  } else if (msg.id !== undefined && msg.method) {
    // Server-to-client request; answer so it does not block.
    send({ id: msg.id, result: null });
  }
}

send({
  id: nextId++,
  method: "initialize",
  params: {
    processId: process.pid,
    rootUri,
    workspaceFolders: [{ uri: rootUri, name: "workspace" }],
    capabilities: {
      workspace: { executeCommand: { dynamicRegistration: false }, workspaceFolders: true },
      textDocument: { publishDiagnostics: { relatedInformation: true } },
      window: { workDoneProgress: true },
    },
  },
});

setTimeout(() => {
  send({ method: "initialized", params: {} });
  send({ id: nextId++, method: "workspace/executeCommand", params: { command: "solana.scanWorkspace", arguments: [] } });
}, 1500);

const deadline = Number(process.env.ACKEE_WAIT ?? 120000);
const started = Date.now();
const timer = setInterval(() => {
  if (complete || Date.now() - started > deadline) {
    clearInterval(timer);
    report();
    srv.kill();
    process.exit(0);
  }
}, 1000);

function report() {
  console.log("=== Ackee Solana security scan ===");
  console.log(`root: ${ROOT}\n`);
  if (complete) {
    console.log(`Rust files      : ${complete.total_rust_files}`);
    console.log(`Anchor programs : ${complete.anchor_program_files}`);
    console.log(`Files w/ issues : ${complete.files_with_issues}`);
    console.log(`Total issues    : ${complete.total_issues}`);
  } else {
    console.log("(no solana/scanComplete received before the deadline)");
  }
  const files = [...diagnostics.entries()];
  if (files.length === 0) { console.log("\nNo diagnostics published."); return; }
  const sev = { 1: "ERROR", 2: "WARN", 3: "INFO", 4: "HINT" };
  let n = 0;
  console.log(`\n--- diagnostics in ${files.length} file(s) ---`);
  for (const [uri, ds] of files) {
    console.log(`\n${uri.replace(pathToFileURL(ROOT).href + "/", "")}`);
    for (const d of ds) {
      n++;
      const line = (d.range?.start?.line ?? 0) + 1;
      console.log(`  [${sev[d.severity] ?? d.severity}] line ${line}: ${d.message.split("\n")[0]}`);
      if (d.code) console.log(`        code: ${typeof d.code === "object" ? JSON.stringify(d.code) : d.code}`);
    }
  }
  console.log(`\ntotal diagnostics: ${n}`);
}
