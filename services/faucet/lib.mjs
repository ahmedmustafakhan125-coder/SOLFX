/**
 * The faucet's decisions, with no I/O: who may claim, how much, and what a claim sends.
 *
 * Kept apart from `faucet.mjs` so every rule here is tested directly — a rate limiter that is
 * only exercised through an HTTP server and a live cluster is a rate limiter nobody has tested.
 */
import { readFileSync, renameSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { isAddress, isOffCurveAddress } from "@solana/kit";

export const DAY_MS = 24 * 60 * 60 * 1000;

// --- amounts ------------------------------------------------------------------------------

/**
 * A decimal string in whole units ("10000", "0.05") to base units, exactly.
 *
 * No floats: `0.05 * 1e9` is 50000000.00000001 in IEEE 754, and the repo bans float arithmetic
 * on money paths for that reason. Too many fractional digits is an error rather than a silent
 * truncation, so a typo in an env var fails at startup instead of paying out a different amount.
 */
export function parseUnits(text, decimals) {
  const s = String(text).trim();
  const m = /^(\d+)(?:\.(\d+))?$/.exec(s);
  if (!m) throw new Error(`not a non-negative decimal amount: "${text}"`);
  const [, whole, frac = ""] = m;
  if (frac.length > decimals) {
    throw new Error(`"${text}" has more than ${decimals} decimal places`);
  }
  return BigInt(whole) * 10n ** BigInt(decimals) + BigInt(frac.padEnd(decimals, "0") || "0");
}

/** Base units back to a decimal string, trailing zeros trimmed. For messages, never for math. */
export function formatUnits(amount, decimals) {
  const neg = amount < 0n;
  const v = neg ? -amount : amount;
  const base = 10n ** BigInt(decimals);
  const whole = v / base;
  const frac = (v % base).toString().padStart(decimals, "0").replace(/0+$/, "");
  return `${neg ? "-" : ""}${whole}${frac ? `.${frac}` : ""}`;
}

/**
 * What one claim sends, given the claimant's current SOL balance.
 *
 * SOL only when they are short of it: a wallet that already holds enough to pay fees and rent
 * does not need the faucet's, and every lamport given away unnecessarily is one fewer claim
 * before somebody has to refill it.
 */
export function planClaim({ userLamports, usdc, solAmount, solBelow }) {
  return { usdc, lamports: userLamports < solBelow ? solAmount : 0n };
}

/**
 * Can the faucet cover this claim? Lamports include the claimant's token-account rent and a
 * fee margin, because the faucet pays both.
 */
export function canCover({ faucetUsdc, faucetLamports, plan, ataRent, feeMargin }) {
  if (faucetUsdc < plan.usdc) return { ok: false, short: "usdc" };
  if (faucetLamports < plan.lamports + ataRent + feeMargin) return { ok: false, short: "sol" };
  return { ok: true };
}

// --- who may claim ------------------------------------------------------------------------

/**
 * A wallet a person can own: a valid 32-byte base58 address **on** the ed25519 curve.
 *
 * Off-curve addresses are PDAs. Nobody holds a key for one, so test USDC sent to it is simply
 * lost — and a PDA is exactly what someone pasting a SolFX account address instead of their
 * wallet would send.
 */
export function validateWallet(text) {
  if (typeof text !== "string") return { ok: false, error: "wallet must be a string" };
  const s = text.trim();
  if (!isAddress(s)) return { ok: false, error: "not a valid Solana address" };
  if (isOffCurveAddress(s)) {
    return { ok: false, error: "that is a program-derived address, not a wallet" };
  }
  return { ok: true, wallet: s };
}

/**
 * One claim per wallet per 24 h, and at most `perIp` claims per address per 24 h.
 *
 * Two limits because each alone is trivially beaten: a per-wallet limit by generating wallets,
 * a per-IP limit by anyone behind a shared NAT being locked out by a stranger. Together they
 * cost an abuser a new address *and* a new wallet per claim, while a room of judges behind one
 * conference router still gets `perIp` claims.
 *
 * In memory, written through to a small JSON file on every change, so a restart neither
 * forgets who claimed nor hands everyone a fresh allowance.
 */
export class ClaimLimiter {
  constructor({ path, perIp, windowMs = DAY_MS, now = () => Date.now() }) {
    this.path = path;
    this.perIp = perIp;
    this.windowMs = windowMs;
    this.now = now;
    /** wallet -> last claim time (ms) */
    this.wallets = new Map();
    /** ip -> claim times (ms), oldest first */
    this.ips = new Map();
    /** Claims in flight, so two concurrent requests cannot both pass the check. */
    this.pending = new Set();
    this.load();
  }

  load() {
    if (!this.path) return;
    try {
      const raw = JSON.parse(readFileSync(this.path, "utf8"));
      for (const [w, t] of Object.entries(raw.wallets ?? {})) this.wallets.set(w, Number(t));
      for (const [ip, ts] of Object.entries(raw.ips ?? {})) this.ips.set(ip, ts.map(Number));
    } catch (e) {
      // A missing file is a first start. A corrupt one is logged and replaced, not fatal: a
      // faucet that refuses to start over its own bookkeeping is a faucet that is down.
      if (e?.code !== "ENOENT") console.warn(`[faucet] claims file unreadable, starting empty: ${e.message}`);
    }
    this.prune();
  }

  save() {
    if (!this.path) return;
    mkdirSync(dirname(this.path), { recursive: true });
    const tmp = `${this.path}.tmp`;
    writeFileSync(
      tmp,
      JSON.stringify({ wallets: Object.fromEntries(this.wallets), ips: Object.fromEntries(this.ips) }),
    );
    // Rename is atomic, so a crash mid-write leaves the previous file rather than half of one.
    renameSync(tmp, this.path);
  }

  prune() {
    const cutoff = this.now() - this.windowMs;
    for (const [w, t] of this.wallets) if (t <= cutoff) this.wallets.delete(w);
    for (const [ip, ts] of this.ips) {
      const kept = ts.filter((t) => t > cutoff);
      if (kept.length) this.ips.set(ip, kept);
      else this.ips.delete(ip);
    }
  }

  /** Is this claim allowed right now? `retryAfterMs` says how long until it would be. */
  check(wallet, ip) {
    this.prune();
    const now = this.now();
    if (this.pending.has(wallet)) {
      return { ok: false, reason: "in-progress", retryAfterMs: 0 };
    }
    const last = this.wallets.get(wallet);
    if (last !== undefined) {
      return { ok: false, reason: "wallet", retryAfterMs: last + this.windowMs - now };
    }
    const times = this.ips.get(ip) ?? [];
    if (times.length >= this.perIp) {
      return { ok: false, reason: "ip", retryAfterMs: times[0] + this.windowMs - now };
    }
    return { ok: true };
  }

  /** Hold the wallet while its transaction is in flight. */
  reserve(wallet) {
    this.pending.add(wallet);
  }

  /** The transaction failed before reaching the chain: the claim never happened. */
  release(wallet) {
    this.pending.delete(wallet);
  }

  /** The claim landed, or may have: count it, and write it down. */
  commit(wallet, ip) {
    const now = this.now();
    this.pending.delete(wallet);
    this.wallets.set(wallet, now);
    this.ips.set(ip, [...(this.ips.get(ip) ?? []), now]);
    this.save();
  }
}

/** "5h 12m" — for a person deciding whether to come back later. */
export function humanDuration(ms) {
  const mins = Math.max(1, Math.ceil(ms / 60_000));
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return h ? `${h}h ${m}m` : `${m}m`;
}
