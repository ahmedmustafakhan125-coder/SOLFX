import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";

import { findMarketPda } from "../generated/pdas/market.js";
import { findProtocolPda } from "../generated/pdas/protocol.js";
import { MARKET_DISCRIMINATOR } from "../generated/accounts/market.js";
import { PROTOCOL_DISCRIMINATOR } from "../generated/accounts/protocol.js";
import { SET_MARKET_ORACLE_DISCRIMINATOR } from "../generated/instructions/setMarketOracle.js";
import { OPEN_POSITION_DISCRIMINATOR } from "../generated/instructions/openPosition.js";
import { SET_MARKET_STATUS_DISCRIMINATOR } from "../generated/instructions/setMarketStatus.js";

/** Anchor's rule: first 8 bytes of sha256("<namespace>:<name>"). */
const anchorDiscriminator = (namespace: string, name: string) =>
  new Uint8Array(createHash("sha256").update(`${namespace}:${name}`).digest()).slice(0, 8);

// § 10 of the repo's client rules: a hardcoded discriminator needs a test that re-derives
// it, so drift fails here rather than as an opaque rejection on chain.
describe("discriminators are Anchor's, not transcribed", () => {
  it.each([
    ["set_market_oracle", SET_MARKET_ORACLE_DISCRIMINATOR],
    ["open_position", OPEN_POSITION_DISCRIMINATOR],
    ["set_market_status", SET_MARKET_STATUS_DISCRIMINATOR],
  ])("instruction %s", (name, generated) => {
    expect(Array.from(generated)).toEqual(Array.from(anchorDiscriminator("global", name)));
  });

  it.each([
    ["Market", MARKET_DISCRIMINATOR],
    ["Protocol", PROTOCOL_DISCRIMINATOR],
  ])("account %s", (name, generated) => {
    expect(Array.from(generated)).toEqual(Array.from(anchorDiscriminator("account", name)));
  });
});

// The seeds live in the program's constants.rs and the client must never retype them.
// Pinning against addresses that actually exist on devnet is what proves the generated
// encoding — including u16 little-endian for market_index — matches the program.
describe("PDA derivation matches the deployed program", () => {
  it("protocol", async () => {
    const [pda] = await findProtocolPda();
    expect(pda).toBe("GbgsnqqqRws5Ch33eHuoAWqKghQiVWt8wBSKzwNffjwc");
  });

  it.each([
    [0, "2eSpi3fMiX86WUhVGwmEBH8YTJdCNFgKoQnkhNGKiC99"],
    [1, "GoivTozh4Mc31bxwmaTFxHC1nLz396kwYowKedKgff9i"],
    [2, "Gjara6wAE5AiToUPHeWRvWuoA8mVeX1o7pH3ZxML146R"],
    [3, "FRszCxu8XQ5TGLNwxc8A2WV5By8dTGX9Vajz1jfaQmjW"],
    [4, "CibsqhRWf4edJHEvEQ29dkZKzfqmfYDQNqAgQZ4qdQfd"],
    [5, "9bqPbgoUyJgp9B9zXD5vsEDys3t7u59gK6Boh2fonfEj"],
    [6, "B2o1DxMmMNNcRrUX1vbVkaHyokQKHrUEWDVFv9HdGhVo"],
    [7, "EE6Na7xGuk8Uh9a2bxoRaLY8BqcQdz6mXQswkX5Fbwr1"],
    [8, "F2vk7e5XdGtXZBcYexrbUHMo9s1Q2c5YAQiWakh8AhFa"],
  ])("market %i", async (index, expected) => {
    const [pda] = await findMarketPda({ marketIndex: index });
    expect(pda).toBe(expected);
  });
});
