/**
 * Every NOXFUNDS address this package derives, pinned against an account that really exists on
 * devnet.
 *
 * A wrong seed is not a type error and not a runtime error: it derives a valid address that holds
 * nothing, and the UI shows "no data". These fixtures are real — a settled lifecycle run by the
 * `nox` CLI on 2026-09-19 — so a drifted seed fails here instead of silently emptying a page.
 */
import { describe, expect, it } from "vitest";
import type { Address } from "@solana/kit";

import { nox } from "../index.js";
import {
  NOX_SEEDS,
  findEvaluation,
  findEvaluationVault,
  findMandate,
  findNoxConfig,
  findProfile,
  findVirtualPosition,
} from "../nox/pdas.js";

const TRADER = "Fa3M8NweDi4ZHNzzi1PbqHC32knbFTtAzskvxpSkcSNy" as Address;
const INVESTOR = "2t4b9m1EEZXEZsDyPgQ5hoYMCa9c6CrK5i8pehMKbZjj" as Address;

describe("NOXFUNDS addresses match devnet", () => {
  it("config", async () => {
    expect(await findNoxConfig()).toBe(
      "8xT4KH1xnpqKW7ic1Pypu7vUJU5HRGnyJZ6z5kWtkzQ4",
    );
  });

  it("trader profile — hand-written and generated agree", async () => {
    const want = "85FqmY8snMoFvtKyrLYhXFJW6jCPxBoKfKSzzpEP8F8g";
    expect(await findProfile(TRADER)).toBe(want);
    const [generated] = await nox.findTraderProfilePda({ trader: TRADER });
    expect(generated).toBe(want);
  });

  it.each([
    [0, "Bf7aVemJQwTq5trPgqo6t7vjmHy4M3FcxjjHSp1BCyrc"],
    [1, "5mhW6R4pZ5vjspS7kPGaojAf3NMaqkRrvgofSHuRBhd7"],
    [2, "5qyCdLD2NXCK5jJuHryVwkwvjkxtc2CjeP34DgUohwEs"],
  ])("mandate seq %i", async (seq, want) => {
    expect(await findMandate(INVESTOR, TRADER, seq)).toBe(want);
  });

  it("evaluation addresses match an independent derivation", async () => {
    // No evaluation exists on devnet yet, so these are pinned against a separate Python
    // implementation of the PDA rules — validated by reproducing the real trader profile above.
    const evaluation = await findEvaluation(TRADER, 3);
    expect(evaluation).toBe("6RGjAEmBDByuYqgFEZmYZLrzSmCb3znbEGCS8hxYNXGM");
    expect(await findEvaluationVault(evaluation)).toBe(
      "2wNhhQr1TMMcnL6mQGgattp1TUcv57u5VwcbnLnrFu37",
    );
    // Market 258 is 0x0102, so the byte order shows. The program writes `to_le_bytes()`.
    const vpos = await findVirtualPosition(evaluation, 258, 7);
    expect(vpos).toBe("FwykutPdcVvHe5YbDhSrUL1BYmVkb56ajJmqoDER3RhA");
    expect(vpos).not.toBe("A2umvJtwkFnqyBuFGcY5txfAZuw7zwQAaj4eW38WFZqs"); // big-endian
  });

  it("refuses a seq that is not a u8", async () => {
    await expect(findMandate(INVESTOR, TRADER, 256)).rejects.toThrow(
      RangeError,
    );
  });

  it("the seed table matches the program's constants", () => {
    // Read the Rust constants directly, so a seed changed in the program and not here fails.
    // Paths are relative to this package; the program lives in the same repository.
    return import("node:fs").then(({ readFileSync }) => {
      const rs = readFileSync(
        new URL(
          "../../../../programs/noxfunds/src/constants.rs",
          import.meta.url,
        ),
        "utf8",
      );
      const inRust = new Map(
        [...rs.matchAll(/pub const (\w+_SEED): &\[u8\] = b"([^"]+)";/g)].map(
          (m) => [m[1], m[2]],
        ),
      );
      const expected: Record<string, string> = {
        CONFIG_SEED: NOX_SEEDS.config,
        MANDATE_SEED: NOX_SEEDS.mandate,
        MANDATE_SIGNER_SEED: NOX_SEEDS.signer,
        MANDATE_VAULT_SEED: NOX_SEEDS.vault,
        TRADER_SEED: NOX_SEEDS.trader,
        LISTING_SEED: NOX_SEEDS.listing,
        OFFER_SEED: NOX_SEEDS.offer,
        OFFER_VAULT_SEED: NOX_SEEDS.offerVault,
        INVESTOR_LISTING_SEED: NOX_SEEDS.investorListing,
        REQUEST_SEED: NOX_SEEDS.request,
        EVAL_SEED: NOX_SEEDS.evaluation,
        EVAL_VAULT_SEED: NOX_SEEDS.evaluationVault,
        VPOS_SEED: NOX_SEEDS.virtualPosition,
      };
      for (const [name, value] of Object.entries(expected)) {
        expect(inRust.get(name), name).toBe(value);
      }
      expect(inRust.size).toBe(Object.keys(expected).length);
    });
  });
});
