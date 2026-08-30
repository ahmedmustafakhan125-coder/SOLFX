/**
 * Where the price actually lives — the trap that has already produced one live bug.
 *
 * `open_position` takes `price_update` as a **passed account** with no seeds constraint. On a
 * cluster with a sponsored feed that account is the canonical `[shard, feed_id]` PDA. On
 * devnet and localnet, where nothing sponsors these feeds, it is whatever keypair
 * `price-poster` created and persisted.
 *
 * So a client that derives the sponsored address compiles, reads correctly, looks right in
 * review, and then fails on chain against an address nobody publishes to.
 * `crates/solfx-keeper/src/book.rs` still does exactly this.
 *
 * The map is `price-accounts.json`, written by the poster. This module takes it as data
 * rather than reading a file, because the same code has to run in a browser.
 */
import type { Address } from "@solana/kit";

/** One row of `price-accounts.json`. */
export type PriceAccountEntry = {
  /** Lowercase hex, 64 chars, no `0x`. */
  readonly feed_id: string;
  readonly price_account: string;
  /** SolFX symbol, e.g. `BTC/USD`. */
  readonly symbol: string;
};

const normalise = (symbol: string) =>
  symbol
    .split("")
    .filter((c) => /[a-zA-Z0-9]/.test(c))
    .join("")
    .toUpperCase();

const hex = (feedId: string) => feedId.trim().replace(/^0x/i, "").toLowerCase();

/**
 * Resolves a market to the account that is actually being published to.
 *
 * Lookup is by **feed id** first, because that is what the program checks
 * (`feed_id == market.pyth_feed_id`). Symbol is a convenience for callers holding a name
 * rather than a market account, and is matched ignoring case and separators.
 */
export class PriceAccountMap {
  readonly #byFeed = new Map<string, Address>();
  readonly #bySymbol = new Map<string, Address>();

  constructor(entries: readonly PriceAccountEntry[]) {
    for (const e of entries) {
      this.#byFeed.set(hex(e.feed_id), e.price_account as Address);
      this.#bySymbol.set(normalise(e.symbol), e.price_account as Address);
    }
  }

  /** `feedId` may be hex, or the raw 32 bytes as they appear on `Market.pythFeedId`. */
  forFeed(feedId: string | Uint8Array | readonly number[]): Address | undefined {
    const key =
      typeof feedId === "string"
        ? hex(feedId)
        : Array.from(feedId)
            .map((b) => b.toString(16).padStart(2, "0"))
            .join("");
    return this.#byFeed.get(key);
  }

  forSymbol(symbol: string): Address | undefined {
    return this.#bySymbol.get(normalise(symbol));
  }

  /**
   * Same as `forFeed`, but throws with the reason rather than returning `undefined`.
   * A missing entry means the poster has never published this feed on this cluster, and a
   * transaction built without it fails with `AccountNotFound` on an address that looks
   * perfectly plausible.
   */
  requireFeed(feedId: string | Uint8Array | readonly number[], symbol?: string): Address {
    const found = this.forFeed(feedId);
    if (!found) {
      const label = symbol ? `${symbol} ` : "";
      throw new Error(
        `no price account for ${label}feed — run price-poster against this cluster, ` +
          `then reload price-accounts.json`,
      );
    }
    return found;
  }

  get size(): number {
    return this.#byFeed.size;
  }
}
