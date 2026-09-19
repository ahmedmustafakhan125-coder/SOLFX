import { describe, expect, it } from "vitest";

import {
  fmtFactorBps,
  fmtPctBps,
  fmtSlots,
  marketBitmap,
  marketIndices,
  noteText,
  parseUsdc,
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
