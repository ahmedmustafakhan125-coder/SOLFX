import { describe, expect, it } from "vitest";
import type { Address } from "@solana/kit";

import { findPositionPda, findTriggerOrderPda } from "../pdas.js";
import { PriceAccountMap } from "../priceAccounts.js";

// Pinned against the position actually opened on devnet during the BTC round trip:
// user account 9qSB3r..., market 5 (BTC/USD), nonce 0 -> 9Tsqv...
// If the seed order or the u16/u8 encoding is wrong, this is where it shows.
describe("position PDA", () => {
  it("matches the position opened on devnet", async () => {
    const [pda] = await findPositionPda({
      userAccount: "9qSB3r2BbLVMUPX9UxEtyJ4w8hzBhAhEFxaCEsoqPkv" as Address,
      marketIndex: 5,
      nonce: 0,
    });
    expect(pda).toBe("9TsqvMUumLowuwWdJEEGrQ5ttGmoMV94nyyEMsDcPfKK");
  });

  it("nonce and market index both move the address", async () => {
    const user = "9qSB3r2BbLVMUPX9UxEtyJ4w8hzBhAhEFxaCEsoqPkv" as Address;
    const [a] = await findPositionPda({ userAccount: user, marketIndex: 5, nonce: 0 });
    const [b] = await findPositionPda({ userAccount: user, marketIndex: 5, nonce: 1 });
    const [c] = await findPositionPda({ userAccount: user, marketIndex: 6, nonce: 0 });
    expect(new Set([a, b, c]).size).toBe(3);
  });
});

describe("trigger order PDA", () => {
  it("is seeded with 'order', and order id moves it", async () => {
    const position = "9TsqvMUumLowuwWdJEEGrQ5ttGmoMV94nyyEMsDcPfKK" as Address;
    const [a] = await findTriggerOrderPda({ position, orderId: 0 });
    const [b] = await findTriggerOrderPda({ position, orderId: 1 });
    expect(a).not.toBe(b);
    expect(a).toMatch(/^[1-9A-HJ-NP-Za-km-z]{32,44}$/);
  });
});

// The real rows the poster wrote for devnet.
const ENTRIES = [
  {
    feed_id: "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43",
    price_account: "HptpDroAu5BZuWWr8uEKhrhHD6FjQb2JK5yokXyQCzGS",
    symbol: "BTC/USD",
  },
  {
    feed_id: "a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b",
    price_account: "1rqo3w8a8X8MkHMavtzxJARwXYUtms2FpYvNEq3MwZ2",
    symbol: "EUR/USD",
  },
];

describe("price account map", () => {
  const map = new PriceAccountMap(ENTRIES);

  it("resolves by feed id, which is what the program checks", () => {
    expect(map.forFeed(ENTRIES[0]!.feed_id)).toBe("HptpDroAu5BZuWWr8uEKhrhHD6FjQb2JK5yokXyQCzGS");
    expect(map.forFeed(`0x${ENTRIES[0]!.feed_id.toUpperCase()}`)).toBe(
      "HptpDroAu5BZuWWr8uEKhrhHD6FjQb2JK5yokXyQCzGS",
    );
  });

  it("accepts the raw bytes as they come off Market.pythFeedId", () => {
    const bytes = Uint8Array.from(
      (ENTRIES[0]!.feed_id.match(/../g) ?? []).map((b) => parseInt(b, 16)),
    );
    expect(map.forFeed(bytes)).toBe("HptpDroAu5BZuWWr8uEKhrhHD6FjQb2JK5yokXyQCzGS");
  });

  it("resolves by symbol regardless of separator or case", () => {
    for (const s of ["BTC/USD", "btcusd", "BTCUSD", "btc/usd"]) {
      expect(map.forSymbol(s)).toBe("HptpDroAu5BZuWWr8uEKhrhHD6FjQb2JK5yokXyQCzGS");
    }
  });

  it("returns undefined rather than a plausible wrong address", () => {
    expect(map.forSymbol("XAU/USD")).toBeUndefined();
    expect(map.forFeed("00".repeat(32))).toBeUndefined();
  });

  it("requireFeed explains what to do instead of failing on chain", () => {
    expect(() => map.requireFeed("00".repeat(32), "XAU/USD")).toThrow(/price-poster/);
  });
});
