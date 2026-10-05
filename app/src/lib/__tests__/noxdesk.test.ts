import { describe, expect, it } from "vitest";
import { nox } from "@solfx/client";
import type { Address } from "@solana/kit";

import type { Marketplace, Row } from "@/lib/nox";
import {
  composeSynthetic,
  costToUsd,
  marketQuote,
  pnlToUsd,
  usdToQuote,
  type Conversion,
  bpsFromStop,
  chartLevels,
  estimateEvaluation,
  estimatePnl,
  evalTicketRefusal,
  isVerified,
  parsePrice,
  passedEvaluations,
  riskAtStop,
  riskBps,
  sizeFor,
  stopFromBps,
  stopOnLosingSide,
} from "@/lib/noxdesk";

const LONG = nox.Direction.Long;
const SHORT = nox.Direction.Short;
/** BTC at $80,000.00, at PRICE_PRECISION 1e9. */
const BTC = 80_000_000_000_000n;

describe("stopFromBps — adverse to the trader on both sides", () => {
  it("puts a long's stop below and a short's above", () => {
    expect(stopFromBps(LONG, BTC, 100)).toBe(79_200_000_000_000n);
    expect(stopFromBps(SHORT, BTC, 100)).toBe(80_800_000_000_000n);
  });

  it("floors a long's stop and ceils a short's when the division is inexact", () => {
    // 1_000_000_001 × 9_900 / 10_000 = 990_000_000.99 → floor
    expect(stopFromBps(LONG, 1_000_000_001n, 100)).toBe(990_000_000n);
    // 1_000_000_001 × 10_100 / 10_000 = 1_010_000_001.01 → ceil
    expect(stopFromBps(SHORT, 1_000_000_001n, 100)).toBe(1_010_000_002n);
  });

  it.each([0, -5, 10_000, 1.5, Number.NaN])("refuses %s bps", (bps) => {
    expect(stopFromBps(LONG, BTC, bps)).toBe(0n);
  });

  it("refuses a missing price", () => {
    expect(stopFromBps(LONG, 0n, 100)).toBe(0n);
  });
});

describe("bpsFromStop — measured the way a limit is", () => {
  it("round-trips an exact stop", () => {
    expect(bpsFromStop(BTC, stopFromBps(LONG, BTC, 150))).toBe(150);
    expect(bpsFromStop(BTC, stopFromBps(SHORT, BTC, 150))).toBe(150);
  });

  it("rounds a fractional distance up, never down", () => {
    // 1 unit off a price of 3 is 3333.33… bps → 3334
    expect(bpsFromStop(3n, 2n)).toBe(3_334);
  });
});

describe("stopOnLosingSide — StopOnWrongSide before the program says it", () => {
  it("accepts below a long and above a short, strictly", () => {
    expect(stopOnLosingSide(LONG, BTC, BTC - 1n)).toBe(true);
    expect(stopOnLosingSide(SHORT, BTC, BTC + 1n)).toBe(true);
    expect(stopOnLosingSide(LONG, BTC, BTC)).toBe(false);
    expect(stopOnLosingSide(SHORT, BTC, BTC)).toBe(false);
    expect(stopOnLosingSide(LONG, BTC, BTC + 1n)).toBe(false);
    expect(stopOnLosingSide(SHORT, BTC, BTC - 1n)).toBe(false);
  });
});

describe("riskAtStop and riskBps — the 1% rule, measured as the program does", () => {
  it("is size × distance / 1e12, ceiled", () => {
    // 0.0125 BTC with a stop $800 away = $10.00 exactly
    const size = 12_500_000n;
    expect(riskAtStop(size, BTC, stopFromBps(LONG, BTC, 100))).toBe(
      10_000_000n
    );
    // One base unit at a one-unit distance is 1e-12 USDC → ceils to the smallest unit
    expect(riskAtStop(1n, 2n, 1n)).toBe(1n);
  });

  it("is the same whichever side the stop is on", () => {
    const size = 12_500_000n;
    expect(riskAtStop(size, BTC, BTC - 800_000_000_000n)).toBe(
      riskAtStop(size, BTC, BTC + 800_000_000_000n)
    );
  });

  it("measures exactly 1% of a $1,000 balance as 100 bps, and one unit more as 101", () => {
    expect(riskBps(10_000_000n, 1_000_000_000n)).toBe(100n);
    expect(riskBps(10_000_001n, 1_000_000_000n)).toBe(101n);
  });

  it("has nothing to measure against with no balance", () => {
    expect(riskBps(1n, 0n)).toBeNull();
  });
});

describe("parsePrice — a price is never guessed at", () => {
  it.each([
    ["80000", 80_000_000_000_000n],
    ["80,000.5", 80_000_500_000_000n],
    ["1.08123", 1_081_230_000n],
    ["0.000000001", 1n],
  ])("reads %s", (input, want) => {
    expect(parsePrice(input)).toBe(want);
  });

  it.each(["", "0", "-1", "1e5", "1.0000000001", "abc", "1.2.3"])(
    "refuses %s",
    (input) => {
      expect(parsePrice(input)).toBeNull();
    }
  );
});

describe("evalTicketRefusal — the program's order of refusals", () => {
  const size = 12_500_000n; // $1,000 of BTC at $80,000
  const ok = {
    direction: LONG,
    price: BTC,
    stale: false,
    sizeBase: size,
    notional: 1_000_000_000n,
    stop: stopFromBps(LONG, BTC, 100), // $10 at risk
    balance: 10_000_000_000n, // $10,000 → 0.10%
    maxLeverage: 50,
    openPositions: 0,
    maxOpen: 5,
    maxRiskBps: 100,
  } as const;

  it("passes a ticket inside every rule, long and short", () => {
    expect(evalTicketRefusal(ok)).toBeNull();
    expect(
      evalTicketRefusal({
        ...ok,
        direction: SHORT,
        stop: stopFromBps(SHORT, BTC, 100),
      })
    ).toBeNull();
  });

  it("refuses a stop on the winning side", () => {
    expect(
      evalTicketRefusal({ ...ok, stop: stopFromBps(SHORT, BTC, 100) })
    ).toMatch(/below/);
  });

  it("refuses risk one unit past 1%, and accepts exactly 1%", () => {
    // $10,000 balance → $100 is exactly 100 bps. 0.125 BTC × $800 = $100.
    const exactly = {
      ...ok,
      sizeBase: 125_000_000n,
      notional: 10_000_000_000n,
    };
    expect(evalTicketRefusal(exactly)).toBeNull();
    expect(evalTicketRefusal({ ...exactly, sizeBase: 125_000_001n })).toMatch(
      /above the 1% limit/
    );
  });

  it("refuses leverage past the market's cap, ceiled", () => {
    expect(
      evalTicketRefusal({ ...ok, notional: 500_000_000_001n, maxLeverage: 50 })
    ).toMatch(/51x/);
  });

  it("refuses a full book before anything else about the trade", () => {
    expect(evalTicketRefusal({ ...ok, openPositions: 5, stop: 0n })).toMatch(
      /already open/
    );
  });

  it("refuses a stale price and a missing one", () => {
    expect(evalTicketRefusal({ ...ok, stale: true })).toMatch(/stale/);
    expect(evalTicketRefusal({ ...ok, price: 0n })).toMatch(/waiting/);
  });
});

describe("sizeFor", () => {
  it("never sizes more than the notional asked for", () => {
    const size = sizeFor(1_000_000_000n, BTC); // $1,000 of BTC
    expect(size).toBe(12_500_000n);
    expect((size * BTC) / 1_000_000_000_000n).toBeLessThanOrEqual(
      1_000_000_000n
    );
  });
  it("is zero with no price", () => {
    expect(sizeFor(1n, 0n)).toBe(0n);
  });
});

describe("estimatePnl — upnl_in_quote, floored toward −∞", () => {
  const size = 12_500_000n; // 0.0125 BTC

  it("is the mirror image for a long and a short", () => {
    const up = BTC + 800_000_000_000n;
    expect(estimatePnl(LONG, size, BTC, up)).toBe(10_000_000n);
    expect(estimatePnl(SHORT, size, BTC, up)).toBe(-10_000_000n);
  });

  it("rounds a fractional gain down and a fractional loss further down", () => {
    expect(estimatePnl(LONG, 1n, 2n, 3n)).toBe(0n); // +1e-12 → 0
    expect(estimatePnl(LONG, 1n, 3n, 2n)).toBe(-1n); // −1e-12 → −1, not 0
  });

  it("is zero without a price rather than a loss of everything", () => {
    expect(estimatePnl(LONG, size, BTC, 0n)).toBe(0n);
  });
});

// --- fixtures for the account-level functions -----------------------------------------------

const TRADER = "Trader11111111111111111111111111111111111111" as Address;
const OTHER = "Other111111111111111111111111111111111111111" as Address;

function evaluation(
  trader: Address,
  state: number,
  balance = 10_000_000_000n
): Row<nox.Evaluation> {
  return {
    address: `E${trader.slice(1)}` as Address,
    data: { trader, state, balance } as unknown as nox.Evaluation,
  };
}

function vpos(
  address: string,
  marketIndex: number,
  direction: nox.Direction,
  entryPrice: bigint,
  sizeBase: bigint
): Row<nox.VirtualPosition> {
  return {
    address: address as Address,
    data: {
      marketIndex,
      direction,
      entryPrice,
      sizeBase,
    } as nox.VirtualPosition,
  };
}

function market(
  evaluations: Row<nox.Evaluation>[],
  profilePassed: Record<string, number> = {}
): Marketplace {
  const profiles = new Map(
    Object.entries(profilePassed).map(([a, n]) => [
      a,
      { address: a, data: { evaluationsPassed: n } },
    ])
  );
  return { evaluations, profiles } as unknown as Marketplace;
}

describe("isVerified — the program's statement, not ours", () => {
  it("marks a trader with a passed evaluation", () => {
    expect(isVerified(market([evaluation(TRADER, 1)]), TRADER)).toBe(true);
  });

  it("does not mark an active or failed evaluation", () => {
    expect(isVerified(market([evaluation(TRADER, 0)]), TRADER)).toBe(false);
    expect(isVerified(market([evaluation(TRADER, 2)]), TRADER)).toBe(false);
  });

  it("does not lend one trader's pass to another", () => {
    expect(isVerified(market([evaluation(OTHER, 1)]), TRADER)).toBe(false);
  });

  it("reads the profile's own counter, and never counts one pass twice", () => {
    expect(isVerified(market([], { [TRADER]: 1 }), TRADER)).toBe(true);
    expect(
      passedEvaluations(
        market([evaluation(TRADER, 1)], { [TRADER]: 1 }),
        TRADER
      )
    ).toBe(1);
    expect(passedEvaluations(market([], { [OTHER]: 3 }), TRADER)).toBe(0);
  });

  it("counts every pass", () => {
    const m = market([
      evaluation(TRADER, 1),
      evaluation(TRADER, 2),
      evaluation(TRADER, 1),
    ]);
    expect(passedEvaluations(m, TRADER)).toBe(2);
  });
});

describe("estimateEvaluation", () => {
  const e = evaluation(TRADER, 0).data;
  const btc = vpos("P1", 5, nox.Direction.Long, BTC, 12_500_000n);

  it("adds each position's mark to the balance", () => {
    const est = estimateEvaluation(e, [btc], { 5: BTC + 800_000_000_000n });
    expect(est?.equity).toBe(10_010_000_000n);
    expect(est?.pnl.get("P1" as Address)).toBe(10_000_000n);
  });

  it("refuses to estimate when one position has no price", () => {
    const eth = vpos("P2", 6, nox.Direction.Short, 3_000_000_000_000n, 1n);
    expect(estimateEvaluation(e, [btc, eth], { 5: BTC })).toBeNull();
  });

  it("is the balance itself when flat", () => {
    expect(estimateEvaluation(e, [], {})?.equity).toBe(10_000_000_000n);
  });
});

describe("chartLevels", () => {
  it("draws entry, stop and target for this market only", () => {
    const levels = chartLevels(5, [
      {
        marketIndex: 5,
        direction: LONG,
        entryPrice: BTC,
        stopPrice: 79_000_000_000_000n,
        takeProfitPrice: 82_000_000_000_000n,
        book: "Eval",
      },
      { marketIndex: 6, direction: SHORT, entryPrice: 1n, book: "Funded" },
    ]);
    expect(levels.map((l) => l.kind)).toEqual([
      "long",
      "stop-loss",
      "take-profit",
    ]);
  });

  it("draws nothing for an absent target", () => {
    const levels = chartLevels(5, [
      {
        marketIndex: 5,
        direction: SHORT,
        entryPrice: BTC,
        takeProfitPrice: 0n,
        book: "Funded",
      },
    ]);
    expect(levels.map((l) => l.kind)).toEqual(["short"]);
  });
});

// --- markets not quoted in USD, and synthetics ----------------------------------------------

/** USD/INR at 88.00: quote units per USD, at PRICE_PRECISION. */
const INR: Conversion = { kind: "quotePerUsd", rate: 88_000_000_000n };

describe("conversion — the program's rounding, both ways", () => {
  it("converts a cost up and a P&L toward −∞", () => {
    expect(costToUsd(8_800_000_000n, INR)).toBe(100_000_000n); // ₹8,800 → $100
    expect(costToUsd(1n, INR)).toBe(1n); // a cost never rounds to nothing
    expect(pnlToUsd(1n, INR)).toBe(0n); // a gain shrinks
    expect(pnlToUsd(-1n, INR)).toBe(-1n); // a loss grows
  });

  it("converts USD-per-quote the other way round", () => {
    const eur: Conversion = { kind: "usdPerQuote", rate: 1_085_430_000n };
    expect(costToUsd(1_000_000_000n, eur)).toBe(1_085_430_000n); // €1,000 → $1,085.43
  });

  it("sizes from dollars without ever exceeding them", () => {
    expect(usdToQuote(100_000_000n, INR)).toBe(8_800_000_000n);
    // $1,000 of USD/INR at 88 is 1,000 USD of base.
    expect(sizeFor(1_000_000_000n, 88_000_000_000n, INR)).toBe(
      1_000_000_000_000n
    );
  });

  /** The same trade the program books in `tests/stage11.rs`: a 1% rise on a $1,000 long. */
  it("marks a rupee-quoted long in dollars, not rupees", () => {
    const pnl = estimatePnl(
      LONG,
      1_000_000_000_000n,
      88_000_000_000n,
      88_880_000_000n,
      // USD/INR converts at its own price of the moment, as the program converts at close.
      { kind: "quotePerUsd", rate: 88_880_000_000n }
    );
    expect(pnl).toBe(9_900_990n); // ₹880 at 88.88 ≈ $9.90, floored
  });

  it("measures risk at the stop in dollars", () => {
    // 1,000 USD of base, stop ₹0.88 away: ₹880 at risk, $10 at 88.
    expect(
      riskAtStop(1_000_000_000_000n, 88_000_000_000n, 87_120_000_000n, INR)
    ).toBe(10_000_000n);
  });
});

describe("composeSynthetic", () => {
  it("is EUR/USD ÷ GBP/USD, floored — the program's figure in stage11.rs", () => {
    expect(composeSynthetic(1_085_430_000n, 1_270_000_000n, true)).toBe(
      854_669_291n
    );
  });
  it("multiplies when the quote is not inverted", () => {
    expect(composeSynthetic(2_000_000_000n, 3_000_000_000n, false)).toBe(
      6_000_000_000n
    );
  });
});

describe("marketQuote — a market is priced from every leg it names, or not at all", () => {
  const hex = (b: number) => b.toString(16).padStart(2, "0").repeat(32);
  const bytes = (b: number) => new Uint8Array(32).fill(b);
  const live = (price: bigint, stale = false) => ({
    price,
    conf: 0n,
    publishTime: 0n,
    ageSeconds: stale ? 120 : 3,
    stale,
  });
  const market = (data: object) =>
    ({
      index: 1,
      data: {
        pythFeedId: bytes(1),
        secondaryFeedId: bytes(2),
        quoteConversionFeed: bytes(3),
        ...data,
      },
    }) as never;
  const accounts = {
    [hex(1)]: "A1" as Address,
    [hex(2)]: "A2" as Address,
    [hex(3)]: "A3" as Address,
  };

  it("composes a synthetic from both legs and names both accounts", () => {
    const m = market({
      priceSource: { __kind: "Synthetic", invertQuote: true },
      quoteConversionKind: 0,
    });
    const q = marketQuote(
      m,
      { [hex(1)]: live(1_085_430_000n), [hex(2)]: live(1_270_000_000n) },
      accounts
    );
    expect(q?.price).toBe(854_669_291n);
    expect(q?.legs).toEqual({ priceUpdate: "A1", secondaryPriceUpdate: "A2" });
    expect(q?.conversion).toEqual({ kind: "none" });
  });

  it("carries the conversion feed's price as the rate", () => {
    const m = market({
      priceSource: { __kind: "Direct" },
      quoteConversionKind: 1,
    });
    const q = marketQuote(
      m,
      { [hex(1)]: live(88_000_000_000n), [hex(3)]: live(88_000_000_000n) },
      accounts
    );
    expect(q?.conversion).toEqual({
      kind: "quotePerUsd",
      rate: 88_000_000_000n,
    });
    expect(q?.legs.quoteConversionPriceUpdate).toBe("A3");
  });

  it("is stale if any leg is, and missing if any leg is", () => {
    const m = market({
      priceSource: { __kind: "Synthetic", invertQuote: true },
      quoteConversionKind: 0,
    });
    expect(
      marketQuote(m, { [hex(1)]: live(1n), [hex(2)]: live(1n, true) }, accounts)
        ?.stale
    ).toBe(true);
    expect(marketQuote(m, { [hex(1)]: live(1n) }, accounts)).toBeUndefined();
  });
});
