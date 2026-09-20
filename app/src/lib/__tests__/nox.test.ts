import { describe, expect, it } from "vitest";

import {
  fmtFactorBps,
  fmtPctBps,
  fmtSlots,
  marketBitmap,
  marketIndices,
  noteText,
  parseUsdc,
  previewSplit,
} from "@/lib/nox";

describe("parseUsdc — the number that gets escrowed", () => {
  it.each([
    ["5000", 5_000_000_000n],
    ["5,000", 5_000_000_000n],
    ["0.000001", 1n],
    ["12.5", 12_500_000n],
    ["  7  ", 7_000_000n],
  ])("%s", (input, want) => {
    expect(parseUsdc(input)).toBe(want);
  });

  it.each(["", "0", "0.0", "-5", "1e3", "1.2345678", "abc", "5.", ".5"])(
    "refuses %j",
    (input) => {
      expect(parseUsdc(input)).toBeNull();
    }
  );

  it("never goes through a float", () => {
    // 0.1 + 0.2 is the classic; in base units it must be exactly 300,000.
    expect(parseUsdc("0.3")).toBe(300_000n);
    expect(parseUsdc("9007199254740993")).toBe(9_007_199_254_740_993_000_000n);
  });
});

describe("statistics formatting is integer-exact", () => {
  it("percent", () => {
    expect(fmtPctBps(4_500n)).toBe("45.00%");
    expect(fmtPctBps(12)).toBe("0.12%");
    expect(fmtPctBps(null)).toBe("—");
  });

  it("profit factor", () => {
    expect(fmtFactorBps(15_000n)).toBe("1.50×");
    expect(fmtFactorBps(12_345n)).toBe("1.23×");
    expect(fmtFactorBps(null)).toBe("—");
  });

  it("hold time is marked approximate", () => {
    // 150 slots × 400 ms = exactly one minute, which rolls over into minutes.
    expect(fmtSlots(149n)).toBe("≈59s");
    expect(fmtSlots(150n)).toBe("≈1m");
    expect(fmtSlots(9_000n)).toBe("≈1h 0m");
    expect(fmtSlots(null)).toBe("—");
  });
});

describe("market bitmaps", () => {
  it("round-trips", () => {
    expect(marketIndices(marketBitmap([0, 5, 127]))).toEqual([0, 5, 127]);
    expect(marketBitmap([])).toBe(0n);
  });
});

describe("notes", () => {
  it("reads only the stored length", () => {
    const buf = new Uint8Array(180);
    buf.set(new TextEncoder().encode("hello world"));
    expect(noteText(buf, 5)).toBe("hello");
  });

  it("invalid UTF-8 reads as empty rather than as mojibake", () => {
    expect(noteText(Uint8Array.of(0xff, 0xfe), 2)).toBe("");
  });
});

describe("previewSplit", () => {
  const usdc = (n: number) => BigInt(Math.round(n * 1e6));

  /**
   * The worked example from `programs/noxfunds/src/settlement.rs` and `docs/NOXFUNDS.md`.
   * If the page previews a different number from the one the program pays, the page is lying.
   */
  it("matches the program's worked example", () => {
    const s = previewSplit(usdc(4500), usdc(3500), 500, 7000);
    expect(s.gross).toBe(usdc(1000));
    expect(s.protocol).toBe(usdc(50)); // 5% of gross
    expect(s.trader).toBe(usdc(665)); // 70% of the 950 net
    expect(s.investor).toBe(usdc(3785)); // principal + the other 30%
  });

  it("pays nobody but the investor when there is no profit", () => {
    const s = previewSplit(usdc(198.54), usdc(200), 500, 7000);
    expect(s.gross).toBe(0n);
    expect(s.protocol).toBe(0n);
    expect(s.trader).toBe(0n);
    // The investor takes what is left — that is what bearing the loss means.
    expect(s.investor).toBe(usdc(198.54));
  });

  it("rounds the fee up and the trader's share down, and still conserves every unit", () => {
    // One unit of profit: the fee ceiling takes it, and nothing is created or lost.
    const s = previewSplit(usdc(100) + 1n, usdc(100), 500, 7000);
    expect(s.gross).toBe(1n);
    expect(s.protocol).toBe(1n); // ceil(0.05) = 1, adverse to the party being charged
    expect(s.trader).toBe(0n);
    expect(s.investor + s.trader + s.protocol).toBe(usdc(100) + 1n);
  });

  it("conserves every unit across a range of awkward amounts", () => {
    for (let profit = 0n; profit < 97n; profit++) {
      const final = usdc(1000) + profit;
      const s = previewSplit(final, usdc(1000), 500, 8000);
      expect(s.investor + s.trader + s.protocol).toBe(final);
      expect(s.protocol).toBeLessThanOrEqual(s.gross);
    }
  });
});
