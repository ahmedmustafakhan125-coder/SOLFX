import {
  getAddressEncoder,
  getBooleanEncoder,
  getI64Encoder,
  getStructEncoder,
  getU128Encoder,
  getU16Encoder,
  getU32Encoder,
  getU64Encoder,
  getU8Encoder,
  type Address,
  type Encoder,
} from "@solana/kit";
import { describe, expect, it } from "vitest";

import {
  EQUITY_OBSERVED_DISCRIMINATOR,
  MANDATE_FUNDED_DISCRIMINATOR,
  MANDATE_SETTLED_DISCRIMINATOR,
  POSITION_RECONCILED_DISCRIMINATOR,
  TRADE_RECORDED_DISCRIMINATOR,
} from "../nox/events.generated.js";
import { NOXFUNDS_PROGRAM_ADDRESS } from "../generated-noxfunds/programs/noxfunds.js";
import { compareProfile, rederiveProfile, type ProfileTx } from "../nox/verify.js";

const PROFILE = "5JomF1SJUawqdLV6tWMFwoLuLKjbM6h67jGB3FrVf83w" as Address;
const TRADER = "2CdttmCi65rJb5UcWR9zCfwocYjcoYBpcWWVpJYhu4oF" as Address;
const MANDATE = "SysvarRent111111111111111111111111111111111" as Address;
const OTHER = "11111111111111111111111111111111" as Address;

const a = getAddressEncoder();
function event(disc: Uint8Array, enc: Encoder<object>, value: object): string {
  const body = enc.encode(value);
  const bytes = new Uint8Array(8 + body.length);
  bytes.set(disc);
  bytes.set(body, 8);
  return `Program data: ${Buffer.from(bytes).toString("base64")}`;
}

// Totals in the event are the program's running figures after the trade; tests pass them in
// so a wrong one can be caught.
function trade(pnl: bigint, hold: bigint, totals: [number, number, number, bigint, bigint]) {
  return event(
    TRADE_RECORDED_DISCRIMINATOR,
    getStructEncoder([
      ["profile", a],
      ["mandate", a],
      ["trader", a],
      ["marketIndex", getU16Encoder()],
      ["nonce", getU8Encoder()],
      ["realizedPnl", getI64Encoder()],
      ["holdSlots", getU64Encoder()],
      ["trades", getU32Encoder()],
      ["wins", getU32Encoder()],
      ["losses", getU32Encoder()],
      ["grossProfit", getU64Encoder()],
      ["grossLoss", getU64Encoder()],
      ["ts", getI64Encoder()],
    ]) as Encoder<object>,
    {
      profile: PROFILE,
      mandate: MANDATE,
      trader: TRADER,
      marketIndex: 3,
      nonce: 0,
      realizedPnl: pnl,
      holdSlots: hold,
      trades: totals[0],
      wins: totals[1],
      losses: totals[2],
      grossProfit: totals[3],
      grossLoss: totals[4],
      ts: 0n,
    },
  );
}

const reconciled = event(
  POSITION_RECONCILED_DISCRIMINATOR,
  getStructEncoder([
    ["mandate", a],
    ["marketIndex", getU16Encoder()],
    ["nonce", getU8Encoder()],
    ["notionalReleased", getU64Encoder()],
    ["openPositions", getU8Encoder()],
    ["caller", a],
    ["ts", getI64Encoder()],
  ]) as Encoder<object>,
  { mandate: MANDATE, marketIndex: 3, nonce: 0, notionalReleased: 0n, openPositions: 0, caller: OTHER, ts: 0n },
);

const funded = (trader: Address, mandate: Address) =>
  event(
    MANDATE_FUNDED_DISCRIMINATOR,
    getStructEncoder([
      ["mandate", a],
      ["investor", a],
      ["trader", a],
      ["principal", getU64Encoder()],
      ["maxTradeNotional", getU64Encoder()],
      ["maxDrawdownBps", getU16Encoder()],
      ["allowedMarkets", getU128Encoder()],
      ["traderSplitBps", getU16Encoder()],
      ["ts", getI64Encoder()],
    ]) as Encoder<object>,
    {
      mandate,
      investor: OTHER,
      trader,
      principal: 500_000_000n,
      maxTradeNotional: 0n,
      maxDrawdownBps: 600,
      allowedMarkets: 0n,
      traderSplitBps: 7000,
      ts: 0n,
    },
  );

const settled = event(
  MANDATE_SETTLED_DISCRIMINATOR,
  getStructEncoder([
    ["mandate", a],
    ["investor", a],
    ["trader", a],
    ["principal", getU64Encoder()],
    ["finalEquity", getU64Encoder()],
    ["grossProfit", getU64Encoder()],
    ["protocolFee", getU64Encoder()],
    ["traderShare", getU64Encoder()],
    ["investorShare", getU64Encoder()],
    ["wasBreached", getBooleanEncoder()],
    ["settledBy", a],
    ["ts", getI64Encoder()],
  ]) as Encoder<object>,
  {
    mandate: MANDATE,
    investor: OTHER,
    trader: TRADER,
    principal: 0n,
    finalEquity: 0n,
    grossProfit: 0n,
    protocolFee: 0n,
    traderShare: 0n,
    investorShare: 0n,
    wasBreached: false,
    settledBy: OTHER,
    ts: 0n,
  },
);

const observed = (mandate: Address, drawdownBps: bigint) =>
  event(
    EQUITY_OBSERVED_DISCRIMINATOR,
    getStructEncoder([
      ["mandate", a],
      ["equity", getU64Encoder()],
      ["peakEquity", getU64Encoder()],
      ["drawdownBps", getU64Encoder()],
      ["openPositions", getU8Encoder()],
      ["observer", a],
      ["ts", getI64Encoder()],
    ]) as Encoder<object>,
    { mandate, equity: 0n, peakEquity: 0n, drawdownBps, openPositions: 0, observer: OTHER, ts: 0n },
  );

let n = 0;
function tx(lines: string[], failed = false): ProfileTx {
  return {
    signature: `sig${n++}`,
    slot: BigInt(n),
    blockTime: null,
    failed,
    logs: [
      `Program ${NOXFUNDS_PROGRAM_ADDRESS} invoke [1]`,
      ...lines,
      `Program ${NOXFUNDS_PROGRAM_ADDRESS} success`,
    ],
  };
}

describe("rederiveProfile folds events as record_trade does", () => {
  it("a win and a loss land in their own columns, and zero counts as a loss", () => {
    const r = rederiveProfile(PROFILE, TRADER, [
      tx([funded(TRADER, MANDATE)]),
      tx([trade(2_000_000n, 500n, [1, 1, 0, 2_000_000n, 0n])]),
      tx([trade(-750_000n, 300n, [2, 1, 1, 2_000_000n, 750_000n])]),
      tx([trade(0n, 100n, [3, 1, 2, 2_000_000n, 750_000n])]),
    ]);
    expect(r.derived).toMatchObject({
      trades: 3,
      wins: 1,
      losses: 2,
      grossProfit: 2_000_000n,
      grossLoss: 750_000n,
      largestWin: 2_000_000n,
      largestLoss: 750_000n,
      totalHoldSlots: 900n,
      mandatesFunded: 1,
      activeMandates: 1,
    });
    expect(r.firstDisagreement).toBeNull();
  });

  it("a failed transaction's events are not counted", () => {
    const r = rederiveProfile(PROFILE, TRADER, [
      tx([trade(2_000_000n, 500n, [1, 1, 0, 2_000_000n, 0n])], true),
    ]);
    expect(r.derived.trades).toBe(0);
    expect(r.failedSkipped).toBe(1);
  });

  it("a reconciled trade is untimed; two reconciled together are both ambiguous", () => {
    const one = rederiveProfile(PROFILE, TRADER, [
      tx([trade(-1n, 0n, [1, 0, 1, 0n, 1n]), reconciled]),
    ]);
    expect(one.derived).toMatchObject({ untimedTrades: 1, ambiguousTrades: 0 });

    const two = rederiveProfile(PROFILE, TRADER, [
      tx([
        trade(-5n, 0n, [1, 0, 1, 0n, 5n]),
        reconciled,
        trade(0n, 0n, [2, 0, 2, 0n, 5n]),
        reconciled,
      ]),
    ]);
    expect(two.derived).toMatchObject({ untimedTrades: 2, ambiguousTrades: 2 });
    expect(two.unprovable).toEqual([]);
  });

  /** The program records an ambiguous profitable batch at zero; its true total is in no event. */
  it("an ambiguous batch recorded at zero is reported as unprovable, not guessed", () => {
    const r = rederiveProfile(PROFILE, TRADER, [
      tx([trade(0n, 0n, [1, 0, 1, 0n, 0n]), reconciled, trade(0n, 0n, [2, 0, 2, 0n, 0n]), reconciled]),
    ]);
    expect(r.unprovable).toEqual([MANDATE]);
  });

  it("the worst drawdown counts only the trader's own mandates", () => {
    const r = rederiveProfile(PROFILE, TRADER, [
      tx([funded(TRADER, MANDATE)]),
      tx([observed(MANDATE, 40n)]),
      tx([observed(OTHER, 900n)]),
      tx([observed(MANDATE, 12n)]),
    ]);
    expect(r.derived.maxDrawdownBps).toBe(40);
  });

  it("a mandate settles in profit only when its trades summed above zero", () => {
    const winning = rederiveProfile(PROFILE, TRADER, [
      tx([funded(TRADER, MANDATE)]),
      tx([trade(10n, 1n, [1, 1, 0, 10n, 0n])]),
      tx([settled]),
    ]);
    expect(winning.derived).toMatchObject({ mandatesSettledInProfit: 1, activeMandates: 0 });

    const losing = rederiveProfile(PROFILE, TRADER, [
      tx([funded(TRADER, MANDATE)]),
      tx([trade(-10n, 1n, [1, 0, 1, 0n, 10n])]),
      tx([settled]),
    ]);
    expect(losing.derived.mandatesSettledInProfit).toBe(0);
  });

  it("a running total that disagrees is pinned to the transaction where it began", () => {
    const r = rederiveProfile(PROFILE, TRADER, [
      tx([trade(1n, 1n, [1, 1, 0, 1n, 0n])]),
      tx([trade(1n, 1n, [2, 2, 0, 3n, 0n])]), // the event claims 3; the replay has 2
    ]);
    expect(r.firstDisagreement).toMatchObject({ field: "grossProfit", event: "3", replay: "2" });
  });

  it("events emitted by another program in the same transaction are ignored", () => {
    const r = rederiveProfile(PROFILE, TRADER, [
      {
        signature: "x",
        slot: 1n,
        blockTime: null,
        failed: false,
        logs: [
          `Program ${OTHER} invoke [1]`,
          trade(1n, 1n, [1, 1, 0, 1n, 0n]),
          `Program ${OTHER} success`,
        ],
      },
    ]);
    expect(r.derived.trades).toBe(0);
  });

  it("compareProfile flags exactly the fields that differ", () => {
    const r = rederiveProfile(PROFILE, TRADER, [tx([trade(-5n, 7n, [1, 0, 1, 0n, 5n])])]);
    const rows = compareProfile(
      {
        tier: 0,
        trades: 1,
        wins: 0,
        losses: 1,
        grossProfit: 0n,
        grossLoss: 6n,
        largestWin: 0n,
        largestLoss: 5n,
        totalHoldSlots: 7n,
        maxDrawdownBps: 0,
        mandatesFunded: 0,
        activeMandates: 0,
        mandatesSettledInProfit: 0,
        untimedTrades: 0,
        ambiguousTrades: 0,
      },
      r.derived,
    );
    expect(rows.filter((x) => !x.agrees).map((x) => x.field)).toEqual(["grossLoss"]);
  });
});
