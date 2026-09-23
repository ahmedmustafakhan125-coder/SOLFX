import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";
import { nox } from "@solfx/client";
import type { Address } from "@solana/kit";

import {
  EVAL,
  evalProgress,
  fmtFactorBps,
  fundedPriceLimit,
  fmtPctBps,
  fmtSlots,
  marketBitmap,
  marketIndices,
  NOTIONAL_DIVISOR,
  noteText,
  parseUsdc,
  ruleRefusal,
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

describe("EVAL mirrors the program's rulebook", () => {
  // `EVAL` is a copy of thirteen constants the program enforces. A copy that drifts shows a
  // trader one target and judges them against another, and the first they would know is a
  // refusal on chain — so the copy is read back from `constants.rs` here rather than trusted.
  const rust = readFileSync(
    new URL("../../../../programs/noxfunds/src/constants.rs", import.meta.url),
    "utf8"
  );
  const value = (name: string): string => {
    const m = rust.match(
      new RegExp(`pub const ${name}:[^=]+=\\s*([^;]+);`, "m")
    );
    if (!m) throw new Error(`${name} is not in constants.rs`);
    return m[1].replace(/_/g, "").trim();
  };
  const num = (name: string) => Number(value(name));

  it.each([
    ["EVAL_STAKE", () => Number(EVAL.stake)],
    ["EVAL_MIN_ACCOUNT", () => Number(EVAL.minAccount)],
    ["EVAL_MAX_ACCOUNT", () => Number(EVAL.maxAccount)],
    ["EVAL_MAX_DAILY_LOSS_BPS", () => EVAL.maxDailyLossBps],
    ["EVAL_MAX_DRAWDOWN_BPS", () => EVAL.maxDrawdownBps],
    ["EVAL_MAX_RISK_BPS", () => EVAL.maxRiskBps],
    ["EVAL_MIN_TRADES", () => EVAL.minTrades],
    ["EVAL_MIN_DAYS", () => EVAL.minDays],
    ["EVAL_MIN_HOLD_SECS", () => EVAL.minHoldSecs],
    ["EVAL_MIN_AVG_HOLD_SECS", () => EVAL.minAvgHoldSecs],
    ["EVAL_CONSISTENCY_BPS", () => EVAL.consistencyBps],
    ["EVAL_MAX_OPEN", () => EVAL.maxOpen],
  ])("%s", (name, ours) => {
    expect(ours()).toBe(num(name));
  });

  it("EVAL_TARGET_BPS", () => {
    expect(value("EVAL_TARGET_BPS")).toBe(`[${EVAL.targetBps.join(", ")}]`);
  });
});

describe("evaluation sizing is the program's own relation", () => {
  // notional = size x price / NOTIONAL_DIVISOR, the identity `notional_in_collateral` uses.
  const size = (notionalQuote: bigint, price: bigint) =>
    (notionalQuote * NOTIONAL_DIVISOR) / price;

  it("a $1,000 ticket on a $50,000 market is 0.02 of the base asset", () => {
    const px = 50_000n * 1_000_000_000n;
    expect(size(1_000_000_000n, px)).toBe(20_000_000n); // 0.02e9
  });

  it("round-trips back to the notional asked for", () => {
    const px = 77_485_880_000_000n; // BTC/USD, the price of the first devnet trade
    for (const usd of [1n, 37n, 1_000n, 199_999n]) {
      const q = usd * 1_000_000n;
      const s = size(q, px);
      expect((s * px) / NOTIONAL_DIVISOR).toBeLessThanOrEqual(q);
    }
  });

  it("floors, so a ticket never exceeds the notional the trader typed", () => {
    const px = 3n * 1_000_000_000n; // a price that does not divide evenly
    const s = size(1_000_000n, px);
    expect((s * px) / NOTIONAL_DIVISOR).toBeLessThanOrEqual(1_000_000n);
  });
});

describe("ruleRefusal mirrors check_rules", () => {
  // The panel shows a trader why a funded trade would be refused *before* they sign it. The
  // value of that depends entirely on it agreeing with `trading.rs::check_rules` — a mirror
  // that drifts is worse than no mirror, because it either promises a fill the program refuses
  // or refuses one the program would take. Every case below is one `require!` in that function,
  // in the order the program reaches it.
  const A = "11111111111111111111111111111112" as Address;
  const PX = 1_085_430_000n; // EUR/USD at PRICE_PRECISION, the fixture's price
  const base = {
    discriminator: new Uint8Array(8),
    investor: A,
    trader: A,
    seq: 0,
    solfxUserAccount: A,
    signerBump: 255,
    principal: 10_000_000_000n, // $10,000
    peakEquity: 10_000_000_000n,
    maxTradeNotional: 5_000_000_000n, // $5,000
    maxTotalNotional: 10_000_000_000n, // $10,000
    maxDrawdownBps: 600,
    maxDailyLossBps: 300,
    maxRiskPerTradeBps: 100, // 1%
    maxStopDistanceBps: 500, // 5%
    maxConcurrentPositions: 3,
    allowedMarkets: 1n, // market 0 only
    minHoldSlots: 0n,
    traderSplitBps: 7_000,
    state: 0,
    openPositions: 0,
    openNotional: 0n,
    lastEquity: 10_000_000_000n,
    lastObservedAt: 0n,
    slots: [],
    openedAt: 0n,
    bump: 255,
  } as unknown as nox.Mandate;

  /** $1,000 of notional at PX, sized the way the ticket sizes it. */
  const size = (usd: bigint) => (usd * 1_000_000n * NOTIONAL_DIVISOR) / PX;

  const ask = (over: Partial<Parameters<typeof ruleRefusal>[0]> = {}) =>
    ruleRefusal({
      mandate: base,
      marketIndex: 0,
      direction: nox.Direction.Long,
      price: PX,
      sizeBase: size(1_000n),
      stopPrice: (PX * 9_900n) / 10_000n, // 1% below
      openPositions: 0,
      ...over,
    });

  it("permits a trade inside every limit", () => {
    expect(ask()).toBeNull();
  });

  it("MandateNotActive", () => {
    expect(ask({ mandate: { ...base, state: 1 } as nox.Mandate })).toMatch(
      /no longer active/
    );
  });

  it("MarketNotPermitted", () => {
    expect(ask({ marketIndex: 1 })).toMatch(/did not permit/);
  });

  it("TooManyOpenPositions", () => {
    expect(ask({ openPositions: 3 })).toMatch(/limit/);
    expect(ask({ openPositions: 2 })).toBeNull();
  });

  it("StopLossRequired", () => {
    expect(ask({ stopPrice: 0n })).toMatch(/stop is mandatory/);
  });

  it("StopOnWrongSide, both directions", () => {
    expect(ask({ stopPrice: PX + 1n })).toMatch(/below/);
    expect(ask({ direction: nox.Direction.Short, stopPrice: PX - 1n })).toMatch(
      /above/
    );
    expect(
      ask({
        direction: nox.Direction.Short,
        stopPrice: (PX * 10_100n) / 10_000n,
      })
    ).toBeNull();
  });

  it("StopTooFar, and the boundary is inclusive as the program's <= is", () => {
    const at = (bps: bigint) => (PX * (10_000n - bps)) / 10_000n;
    expect(ask({ stopPrice: at(500n) })).toBeNull();
    expect(ask({ stopPrice: at(501n) })).toMatch(/the mandate allows 5%/);
  });

  it("TradeExceedsMandate", () => {
    expect(ask({ sizeBase: size(5_001n) })).toMatch(/per-trade ceiling/);
    expect(ask({ sizeBase: size(5_000n) })).toBeNull();
  });

  it("TotalNotionalExceeded", () => {
    const loaded = { ...base, openNotional: 9_500_000_000n } as nox.Mandate;
    expect(ask({ mandate: loaded, sizeBase: size(1_000n) })).toMatch(
      /book limit/
    );
    expect(ask({ mandate: loaded, sizeBase: size(400n) })).toBeNull();
  });

  it("RiskPerTradeExceeded — size x distance, not size alone", () => {
    // 1% of $10,000 equity is $100 of risk. At a 1% stop that is $10,000 of notional, which
    // the per-trade ceiling refuses first — so the case is made with a wider stop and a
    // smaller size, which is exactly the trade-off the rule is designed to allow.
    const wide = (PX * 9_600n) / 10_000n; // 4% away, inside the 5% distance limit
    expect(ask({ sizeBase: size(2_500n), stopPrice: wide })).toBeNull(); // $100 risk
    expect(ask({ sizeBase: size(2_600n), stopPrice: wide })).toMatch(
      /risking .* at the stop/
    );
  });

  it("measures risk against the lower of peak and last equity, as check_rules does since L-1", () => {
    // $100 of risk is 1% of the $10,000 peak but 1.11% of a mandate now worth $9,000.
    const wide = (PX * 9_600n) / 10_000n;
    const down = { ...base, lastEquity: 9_000_000_000n } as nox.Mandate;
    expect(ask({ sizeBase: size(2_500n), stopPrice: wide })).toBeNull();
    expect(
      ask({ mandate: down, sizeBase: size(2_500n), stopPrice: wide })
    ).toMatch(/risking .* at the stop/);
  });

  it("measures the notional with a ceiling, as notional_in_quote does", () => {
    // The one size where floor and ceil disagree about whether the trade fits. `mul_div_ceil`
    // is what the program uses, so this must be refused; a mirror that floored would call it
    // compliant and promise a fill that fails.
    const exact = (base.maxTradeNotional * NOTIONAL_DIVISOR) / PX;
    const justOver = exact + 1n;
    expect((justOver * PX) / NOTIONAL_DIVISOR).toBe(base.maxTradeNotional); // floors to the limit
    expect(ask({ sizeBase: justOver })).toMatch(/per-trade ceiling/);
    expect(ask({ sizeBase: exact })).toBeNull();
  });

  it("measures the stop distance with a ceiling, as the program does", () => {
    // A price where the distance does not divide evenly. Rounding down here would admit a stop
    // fractionally wider than the mandate allows; the program rounds up for that reason.
    const price = 10_007n;
    const stop = 9_506n; // 501/10007 = 500.6 bps
    expect(
      ruleRefusal({
        mandate: base,
        marketIndex: 0,
        direction: nox.Direction.Long,
        price,
        sizeBase: 1n,
        stopPrice: stop,
        openPositions: 0,
      })
    ).toMatch(/5\.01%/);
  });
});

describe("fundedPriceLimit", () => {
  const PX = 1_000_000_000n;

  it("is a maximum buying and a minimum selling", () => {
    expect(fundedPriceLimit(nox.Direction.Long, PX, 50)).toBe(1_005_000_000n);
    expect(fundedPriceLimit(nox.Direction.Short, PX, 50)).toBe(995_000_000n);
  });

  it("zero slippage is a bound at the price, not an opt-out", () => {
    expect(fundedPriceLimit(nox.Direction.Long, PX, 0)).toBe(PX);
  });
});

describe("evalProgress names every reason claim_stage_pass would refuse", () => {
  // The panel enables "Claim Phase N" when `blocker` is null. Every `require!` in
  // `claim_stage_pass` must therefore be represented, or the button is live on a stage the
  // program refuses and the trader pays a fee to read the error.
  const SIZE = 10_000_000_000n; // $10,000
  const TARGET = (SIZE * 800n) / 10_000n; // 8% — Phase 1
  const passing = {
    discriminator: new Uint8Array(8),
    trader: "11111111111111111111111111111112" as Address,
    seq: 0,
    stage: 1,
    state: 0,
    failedRule: 0,
    accountSize: SIZE,
    balance: SIZE + TARGET,
    peakEquity: SIZE + TARGET,
    day: 0n,
    dayStartEquity: SIZE,
    dayPnl: 0n,
    bestDayPnl: TARGET / 2n, // exactly the consistency cap, which is <= and so allowed
    trades: 10,
    wins: 7,
    losses: 3,
    grossProfit: TARGET,
    grossLoss: 0n,
    voluntaryCloses: 10,
    voluntaryHoldSecs: 30_000n, // 3,000s average, past the 2,700s floor
    tradingDays: 5,
    lastTradeDay: 0n,
    openPositions: 0,
    lastEquity: SIZE + TARGET,
    lastObservedAt: 0n,
    startedAt: 0n,
    bump: 255,
    vaultBump: 255,
    reserved: new Uint8Array(16),
  } as unknown as nox.Evaluation;

  const blocker = (over: Partial<nox.Evaluation> = {}) =>
    evalProgress({ ...passing, ...over } as nox.Evaluation).blocker;

  it("passes when every rule is met", () => {
    expect(blocker()).toBeNull();
  });

  it.each([
    ["EvaluationNotActive", { state: 2 }, /Failed/],
    ["PositionsStillOpen", { openPositions: 1 }, /passes flat/],
    ["DrawdownExceeded", { peakEquity: SIZE * 2n }, /past the 6% limit/],
    [
      "EvaluationIncomplete — target",
      { balance: SIZE + TARGET - 1n },
      /target/,
    ],
    ["EvaluationIncomplete — trades", { trades: 9 }, /9 of 10 trades/],
    ["EvaluationIncomplete — days", { tradingDays: 4 }, /4 of 5 trading days/],
    [
      "EvaluationIncomplete — average hold",
      { voluntaryHoldSecs: 26_990n },
      /Average hold/,
    ],
    [
      "ConsistencyRuleViolated",
      { bestDayPnl: TARGET / 2n + 1n },
      /no day may carry more than 50%/,
    ],
  ])("%s", (_name, over, pattern) => {
    expect(blocker(over as Partial<nox.Evaluation>)).toMatch(pattern as RegExp);
  });

  it("the target is a ceiling, so a stage cannot pass fractionally short", () => {
    // An account size where 8% does not divide evenly: 12,345 x 800 / 10,000 = 987.6.
    const odd = { ...passing, accountSize: 12_345n } as nox.Evaluation;
    const p = evalProgress(odd);
    expect(p.targetEquity).toBe(12_345n + 988n); // ceiling, not 987
  });

  it("stage 2's target is 5%, not 8%", () => {
    const p = evalProgress({ ...passing, stage: 2 } as nox.Evaluation);
    expect(p.targetBps).toBe(500);
    expect(p.targetEquity).toBe(SIZE + (SIZE * 500n) / 10_000n);
  });
});
