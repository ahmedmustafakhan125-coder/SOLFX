import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  ClaimLimiter,
  DAY_MS,
  canCover,
  formatUnits,
  humanDuration,
  parseUnits,
  planClaim,
  validateWallet,
} from "../lib.mjs";
import { clientIp, lastForwardedFor } from "../../api/client-ip.mjs";

// A real wallet (the devnet mint authority) and a real PDA (SolFX's protocol account).
const WALLET = "7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs";
const PDA = "GbgsnqqqRws5Ch33eHuoAWqKghQiVWt8wBSKzwNffjwc";
const OTHER = "EyvqeDSh2Y4ZhobJY4bF8ueEZAjRx2V3r2GDf35ktPyo";

// --- amounts ------------------------------------------------------------------------------

test("parseUnits is exact where floats are not", () => {
  assert.equal(parseUnits("10000", 6), 10_000_000_000n);
  assert.equal(parseUnits("0.05", 9), 50_000_000n); // 0.05 * 1e9 in floats is 50000000.00000001
  assert.equal(parseUnits("0.02", 9), 20_000_000n);
  assert.equal(parseUnits("1.5", 6), 1_500_000n);
  assert.equal(parseUnits("0", 6), 0n);
  assert.equal(parseUnits(" 7 ", 0), 7n);
});

test("parseUnits refuses what it cannot represent exactly, rather than truncating", () => {
  assert.throws(() => parseUnits("0.0000001", 6), /decimal places/);
  for (const bad of ["", "-1", "1e3", "abc", "1.2.3", ".5", "5."]) {
    assert.throws(() => parseUnits(bad, 6), /not a non-negative decimal/, bad);
  }
});

test("formatUnits round-trips parseUnits", () => {
  for (const [text, dp] of [["10000", 6], ["0.05", 9], ["1.000001", 6], ["0", 9], ["123.4", 6]]) {
    assert.equal(formatUnits(parseUnits(text, dp), dp), text);
  }
  assert.equal(formatUnits(-1_500_000n, 6), "-1.5");
});

test("planClaim sends SOL only to a wallet short of it", () => {
  const base = { usdc: 10_000_000_000n, solAmount: 50_000_000n, solBelow: 20_000_000n };
  assert.deepEqual(planClaim({ ...base, userLamports: 0n }), { usdc: base.usdc, lamports: 50_000_000n });
  assert.deepEqual(planClaim({ ...base, userLamports: 19_999_999n }).lamports, 50_000_000n);
  // At the threshold exactly, the wallet has enough: the comparison is strict.
  assert.deepEqual(planClaim({ ...base, userLamports: 20_000_000n }).lamports, 0n);
  assert.deepEqual(planClaim({ ...base, userLamports: 5_000_000_000n }).lamports, 0n);
});

test("canCover counts the claimant's account rent and the fee, which the faucet pays", () => {
  const plan = { usdc: 100n, lamports: 50n };
  const ok = { faucetUsdc: 100n, faucetLamports: 50n + 7n + 3n, plan, ataRent: 7n, feeMargin: 3n };
  assert.deepEqual(canCover(ok), { ok: true });
  assert.deepEqual(canCover({ ...ok, faucetUsdc: 99n }), { ok: false, short: "usdc" });
  assert.deepEqual(canCover({ ...ok, faucetLamports: 59n }), { ok: false, short: "sol" });
});

// --- who may claim ------------------------------------------------------------------------

test("validateWallet accepts a wallet and trims it", () => {
  assert.deepEqual(validateWallet(` ${WALLET} `), { ok: true, wallet: WALLET });
});

test("validateWallet refuses a PDA: nobody holds its key, so tokens sent there are lost", () => {
  const r = validateWallet(PDA);
  assert.equal(r.ok, false);
  assert.match(r.error, /program-derived/);
});

test("validateWallet refuses what is not an address at all", () => {
  for (const bad of [undefined, null, 42, "", "not-an-address", `${WALLET}x`, WALLET.slice(0, 30), "0".repeat(44)]) {
    assert.equal(validateWallet(bad).ok, false, String(bad));
  }
});

test("the forwarded address is the hop Traefik appended, not one the client wrote", () => {
  assert.equal(lastForwardedFor("1.2.3.4"), "1.2.3.4");
  // A client sending its own X-Forwarded-For cannot choose the address that is counted.
  assert.equal(lastForwardedFor("6.6.6.6, 1.2.3.4"), "1.2.3.4");
  assert.equal(lastForwardedFor(["6.6.6.6", "9.9.9.9, 1.2.3.4"]), "1.2.3.4");
  assert.equal(lastForwardedFor(undefined), "");
  assert.equal(lastForwardedFor(" , "), "");
  assert.equal(clientIp({ headers: {}, socket: { remoteAddress: "10.0.0.1" } }), "10.0.0.1");
  assert.equal(clientIp({ headers: { "x-forwarded-for": "a, b" }, socket: {} }), "b");
});

// --- the limiter --------------------------------------------------------------------------

function clock(start = 1_000_000_000_000) {
  let t = start;
  return { now: () => t, advance: (ms) => (t += ms) };
}

test("one claim per wallet per 24 h, with the time remaining", () => {
  const c = clock();
  const l = new ClaimLimiter({ path: null, perIp: 5, now: c.now });
  assert.deepEqual(l.check(WALLET, "ip1"), { ok: true });
  l.reserve(WALLET);
  l.commit(WALLET, "ip1");

  c.advance(60 * 60 * 1000);
  const again = l.check(WALLET, "ip2"); // a different network does not help
  assert.equal(again.ok, false);
  assert.equal(again.reason, "wallet");
  assert.equal(again.retryAfterMs, DAY_MS - 60 * 60 * 1000);

  c.advance(DAY_MS - 60 * 60 * 1000 - 1);
  assert.equal(l.check(WALLET, "ip1").ok, false, "one millisecond early is still early");
  c.advance(1);
  assert.equal(l.check(WALLET, "ip1").ok, true);
});

test("at most perIp claims per address per 24 h, and the window slides", () => {
  const c = clock();
  const l = new ClaimLimiter({ path: null, perIp: 2, now: c.now });
  l.commit(WALLET, "ip");
  c.advance(1000);
  l.commit(OTHER, "ip");
  const third = l.check("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM", "ip");
  assert.equal(third.ok, false);
  assert.equal(third.reason, "ip");
  assert.equal(third.retryAfterMs, DAY_MS - 1000, "until the oldest claim ages out");
  c.advance(DAY_MS - 1000);
  assert.equal(l.check("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM", "ip").ok, true);
});

test("a claim in flight blocks a second one for the same wallet", () => {
  const l = new ClaimLimiter({ path: null, perIp: 5 });
  l.reserve(WALLET);
  assert.equal(l.check(WALLET, "ip").reason, "in-progress");
  l.release(WALLET); // the send failed: nothing was given, so nothing is counted
  assert.deepEqual(l.check(WALLET, "ip"), { ok: true });
});

test("claims survive a restart, and expired ones are dropped on load", () => {
  const dir = mkdtempSync(join(tmpdir(), "faucet-"));
  const path = join(dir, "claims.json");
  const c = clock();
  const a = new ClaimLimiter({ path, perIp: 5, now: c.now });
  a.commit(WALLET, "ip");
  c.advance(1000);
  a.commit(OTHER, "ip");

  const b = new ClaimLimiter({ path, perIp: 5, now: c.now });
  assert.equal(b.check(WALLET, "x").reason, "wallet");
  assert.equal(b.ips.get("ip").length, 2);

  c.advance(DAY_MS - 500); // WALLET's claim has aged out, OTHER's has not
  const d = new ClaimLimiter({ path, perIp: 5, now: c.now });
  assert.equal(d.check(WALLET, "x").ok, true);
  assert.equal(d.check(OTHER, "x").reason, "wallet");
  assert.equal(d.ips.get("ip").length, 1, "only OTHER's claim still counts against the address");
});

test("a corrupt claims file starts empty instead of taking the faucet down", () => {
  const dir = mkdtempSync(join(tmpdir(), "faucet-"));
  const path = join(dir, "claims.json");
  writeFileSync(path, "{not json");
  const l = new ClaimLimiter({ path, perIp: 5 });
  assert.equal(l.check(WALLET, "ip").ok, true);
  l.commit(WALLET, "ip");
  assert.ok(JSON.parse(readFileSync(path, "utf8")).wallets[WALLET]);
});

test("humanDuration rounds up, so 'try again in' is never too early", () => {
  assert.equal(humanDuration(1), "1m");
  assert.equal(humanDuration(60_001), "2m");
  assert.equal(humanDuration(DAY_MS - 1), "24h 0m");
  assert.equal(humanDuration(5 * 3_600_000 + 12 * 60_000), "5h 12m");
});
