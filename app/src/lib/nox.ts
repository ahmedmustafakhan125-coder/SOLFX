/**
 * Reading and writing the NOXFUNDS marketplace.
 *
 * # One read, not one per account
 *
 * The browser shares an RPC budget with the price poster and the keeper, and the poster
 * losing that race is how prices went stale before. So the whole marketplace is **one**
 * `getProgramAccounts` call: every account the program owns comes back in it, and each is
 * sorted into its type locally by its 8-byte discriminator. At devnet scale that is a handful
 * of accounts; at real scale it would want `dataSlice` and memcmp filters per view, and the
 * call site is the one place that would change.
 *
 * # Nothing here is a claim the chain did not make
 *
 * Every statistic is computed from `TraderProfile` fields the program maintains, with
 * integer arithmetic. Where a figure cannot be computed — a profit factor with no losses, a
 * win rate with no trades — it is `null`, and the UI says so rather than showing a number.
 */
import {
  findCollateralVaultPda,
  findFeeVaultPda,
  findInsuranceFundPda,
  findInsuranceVaultPda,
  findLpPoolPda,
  findLpVaultPda,
  findMarketPda,
  findPositionPda,
  findProtocolPda,
  findTriggerOrderPda,
  findUserAccountPda,
  nox,
  noxPdas,
} from "@solfx/client";
import {
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstructionAsync,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import type {
  Address,
  Instruction,
  Rpc,
  SolanaRpcApi,
  TransactionSigner,
} from "@solana/kit";

import { READ_COMMITMENT } from "@/lib/commitment";

export type Row<T> = { readonly address: Address; readonly data: T };

export type Marketplace = {
  readonly config: nox.NoxConfig | null;
  readonly profiles: ReadonlyMap<Address, Row<nox.TraderProfile>>;
  readonly traderListings: readonly Row<nox.TraderListing>[];
  readonly investorListings: readonly Row<nox.InvestorListing>[];
  readonly offers: readonly Row<nox.MandateOffer>[];
  readonly requests: readonly Row<nox.FundingRequest>[];
  readonly mandates: readonly Row<nox.Mandate>[];
  /** Evaluations, and the simulated positions inside them. */
  readonly evaluations: readonly Row<nox.Evaluation>[];
  readonly virtualPositions: readonly Row<nox.VirtualPosition>[];
  /** What each mandate's vault holds now, keyed by mandate address. */
  readonly vaults: ReadonlyMap<Address, bigint>;
  /** Cluster time when this was read, for judging offer expiry. */
  readonly now: bigint;
  /**
   * The slot this was read at. `min_hold_slots` is counted in slots, not seconds, so a hold
   * countdown built from wall time drifts against the rule the program applies.
   */
  readonly slot: bigint;
};

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function hasPrefix(bytes: Uint8Array, prefix: Uint8Array): boolean {
  if (bytes.length < prefix.length) return false;
  for (let i = 0; i < prefix.length; i++)
    if (bytes[i] !== prefix[i]) return false;
  return true;
}

export async function readMarketplace(
  rpc: Rpc<SolanaRpcApi>
): Promise<Marketplace> {
  const [accounts, slot] = await Promise.all([
    rpc
      .getProgramAccounts(noxPdas.NOXFUNDS_PROGRAM_ADDRESS, {
        encoding: "base64",
        commitment: READ_COMMITMENT,
      })
      .send(),
    rpc.getSlot({ commitment: READ_COMMITMENT }).send(),
  ]);
  const now = BigInt(
    (await rpc.getBlockTime(slot).send()) ?? Math.floor(Date.now() / 1000)
  );

  let config: nox.NoxConfig | null = null;
  const profiles = new Map<Address, Row<nox.TraderProfile>>();
  const traderListings: Row<nox.TraderListing>[] = [];
  const investorListings: Row<nox.InvestorListing>[] = [];
  const offers: Row<nox.MandateOffer>[] = [];
  const requests: Row<nox.FundingRequest>[] = [];
  const mandates: Row<nox.Mandate>[] = [];
  const evaluations: Row<nox.Evaluation>[] = [];
  const virtualPositions: Row<nox.VirtualPosition>[] = [];

  for (const { pubkey, account } of accounts) {
    const bytes = base64ToBytes(account.data[0]);
    // A decoder that throws on one malformed account must not blank the whole marketplace.
    try {
      if (hasPrefix(bytes, nox.NOX_CONFIG_DISCRIMINATOR)) {
        config = nox.getNoxConfigDecoder().decode(bytes);
      } else if (hasPrefix(bytes, nox.TRADER_PROFILE_DISCRIMINATOR)) {
        const data = nox.getTraderProfileDecoder().decode(bytes);
        profiles.set(data.authority, { address: pubkey, data });
      } else if (hasPrefix(bytes, nox.TRADER_LISTING_DISCRIMINATOR)) {
        traderListings.push({
          address: pubkey,
          data: nox.getTraderListingDecoder().decode(bytes),
        });
      } else if (hasPrefix(bytes, nox.INVESTOR_LISTING_DISCRIMINATOR)) {
        investorListings.push({
          address: pubkey,
          data: nox.getInvestorListingDecoder().decode(bytes),
        });
      } else if (hasPrefix(bytes, nox.MANDATE_OFFER_DISCRIMINATOR)) {
        offers.push({
          address: pubkey,
          data: nox.getMandateOfferDecoder().decode(bytes),
        });
      } else if (hasPrefix(bytes, nox.FUNDING_REQUEST_DISCRIMINATOR)) {
        requests.push({
          address: pubkey,
          data: nox.getFundingRequestDecoder().decode(bytes),
        });
      } else if (hasPrefix(bytes, nox.MANDATE_DISCRIMINATOR)) {
        mandates.push({
          address: pubkey,
          data: nox.getMandateDecoder().decode(bytes),
        });
      } else if (hasPrefix(bytes, nox.EVALUATION_DISCRIMINATOR)) {
        evaluations.push({
          address: pubkey,
          data: nox.getEvaluationDecoder().decode(bytes),
        });
      } else if (hasPrefix(bytes, nox.VIRTUAL_POSITION_DISCRIMINATOR)) {
        virtualPositions.push({
          address: pubkey,
          data: nox.getVirtualPositionDecoder().decode(bytes),
        });
      }
    } catch {
      // Skipped rather than fatal. See above.
    }
  }

  return {
    config,
    profiles,
    traderListings,
    investorListings,
    offers,
    requests,
    mandates,
    evaluations,
    virtualPositions,
    vaults: await readVaults(rpc, mandates),
    now,
    slot: BigInt(slot),
  };
}

/**
 * What each mandate's vault actually holds, keyed by mandate.
 *
 * Read rather than inferred. `principal` is what the investor committed at acceptance; the vault
 * is where it is *now*, and the two stop agreeing the moment the capital moves into SolFX to be
 * traded, or out again at settlement. Showing `principal` and labelling it the balance would be
 * the same mistake the program made before `fund_mandate` moved real money: a number that looks
 * like custody and is not.
 *
 * One `getMultipleAccounts` for every mandate read. The amount is a u64 at offset 64 of an SPL
 * token account; an absent or short account reads as zero, which is what a vault emptied into
 * SolFX or paid out genuinely holds.
 */
async function readVaults(
  rpc: Rpc<SolanaRpcApi>,
  mandates: readonly Row<nox.Mandate>[]
): Promise<ReadonlyMap<Address, bigint>> {
  const out = new Map<Address, bigint>();
  if (mandates.length === 0) return out;
  const vaults = await Promise.all(
    mandates.map((m) => noxPdas.findMandateVault(m.address))
  );
  const { value } = await rpc
    .getMultipleAccounts(vaults, {
      encoding: "base64",
      commitment: READ_COMMITMENT,
    })
    .send();
  mandates.forEach((m, i) => {
    const raw = value[i]?.data[0];
    const bytes = raw ? base64ToBytes(raw) : new Uint8Array();
    if (bytes.length < 72) {
      out.set(m.address, 0n);
      return;
    }
    let amount = 0n;
    for (let b = 7; b >= 0; b--)
      amount = (amount << 8n) | BigInt(bytes[64 + b]!);
    out.set(m.address, amount);
  });
  return out;
}

// --- statistics, in integers --------------------------------------------------------------

const BPS = 10_000n;

/** A trader's record as the marketplace shows it. `null` means "not computable yet". */
export type TraderStats = {
  readonly trades: number;
  readonly winRateBps: bigint | null;
  /** Gross profit over gross loss, times 10,000. `null` with no losses to divide by. */
  readonly profitFactorBps: bigint | null;
  readonly maxDrawdownBps: number;
  readonly avgHoldSlots: bigint | null;
  readonly settledInProfit: number;
  readonly mandatesFunded: number;
  readonly tier: nox.TraderTier;
};

export function traderStats(p: nox.TraderProfile): TraderStats {
  const trades = BigInt(p.trades);
  return {
    trades: p.trades,
    winRateBps: trades > 0n ? (BigInt(p.wins) * BPS) / trades : null,
    profitFactorBps:
      p.grossLoss > 0n ? (p.grossProfit * BPS) / p.grossLoss : null,
    maxDrawdownBps: p.maxDrawdownBps,
    avgHoldSlots: trades > 0n ? p.totalHoldSlots / trades : null,
    settledInProfit: p.mandatesSettledInProfit,
    mandatesFunded: p.mandatesFunded,
    tier: p.tier,
  };
}

/** `4500n` → `"45.00%"`. Integer arithmetic throughout. */
export function fmtPctBps(bps: bigint | number | null): string {
  if (bps === null) return "—";
  const v = BigInt(bps);
  const whole = v / 100n;
  const frac = (v % 100n).toString().padStart(2, "0");
  return `${whole}.${frac}%`;
}

/** `15000n` → `"1.50×"`. */
export function fmtFactorBps(bps: bigint | null): string {
  if (bps === null) return "—";
  const whole = bps / BPS;
  const frac = ((bps % BPS) / 100n).toString().padStart(2, "0");
  return `${whole}.${frac}×`;
}

/**
 * Slots as a human duration, **at the nominal 400 ms per slot**.
 *
 * Labelled approximate everywhere it is shown: slot time drifts with the cluster, and the
 * program records slots precisely because they are the honest unit.
 */
export function fmtSlots(slots: bigint | null): string {
  if (slots === null) return "—";
  const secs = (slots * 400n) / 1000n;
  if (secs < 60n) return `≈${secs}s`;
  const mins = secs / 60n;
  if (mins < 60n) return `≈${mins}m`;
  const hours = mins / 60n;
  return `≈${hours}h ${mins % 60n}m`;
}

export const TIER_NAME = ["Bronze", "Silver", "Gold", "Platinum"] as const;
/** What a mandate's state means for the money, in a sentence each. */
export const MANDATE_STATE = [
  {
    name: "Active",
    means: "Trading. The trader may open positions; neither side can withdraw.",
  },
  {
    name: "Breached",
    means:
      "Drawdown breached. No new trades. Anyone may close it out, then the investor is paid.",
  },
  {
    name: "Winding down",
    means: "Settlement requested. Once flat, anyone may run the payout.",
  },
  { name: "Settled", means: "Finished. The money has been paid out." },
] as const;

export const OFFER_STATE_NAME = [
  "Open",
  "Accepted",
  "Revoked",
  "Declined",
] as const;

/** A fixed-size note field, as text. Empty or invalid UTF-8 reads as "". */
export function noteText(
  note: Uint8Array | ArrayLike<number>,
  len: number
): string {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(
      Uint8Array.from(note).subarray(0, len)
    );
  } catch {
    return "";
  }
}

/**
 * `"5000"` or `"5000.25"` USDC → base units, or `null` if it is not a plain positive amount.
 *
 * String arithmetic rather than `parseFloat`: `0.1 + 0.2` is a bug report, and an escrowed
 * principal is exactly the number that must not carry one.
 */
export function parseUsdc(input: string): bigint | null {
  const s = input.trim().replace(/,/g, "");
  const m = /^(\d+)(?:\.(\d{1,6}))?$/.exec(s);
  if (!m) return null;
  const whole = BigInt(m[1] ?? "0");
  const frac = BigInt((m[2] ?? "").padEnd(6, "0"));
  const v = whole * 1_000_000n + frac;
  return v > 0n ? v : null;
}

/** Market bitmap → indices, e.g. `0b101n` → `[0, 2]`. */
export function marketIndices(bitmap: bigint): number[] {
  const out: number[] = [];
  for (let i = 0; i < 128; i++) if ((bitmap >> BigInt(i)) & 1n) out.push(i);
  return out;
}

export function marketBitmap(indices: readonly number[]): bigint {
  return indices.reduce((b, i) => b | (1n << BigInt(i)), 0n);
}

// --- instructions -------------------------------------------------------------------------

/** The rule set an offer carries. Mirrors `MandateRules` in `instructions/investor.rs`. */
export type OfferRules = {
  readonly maxTradeNotional: bigint;
  readonly maxTotalNotional: bigint;
  readonly maxDrawdownBps: number;
  readonly maxDailyLossBps: number;
  readonly maxRiskPerTradeBps: number;
  readonly maxStopDistanceBps: number;
  readonly maxConcurrentPositions: number;
  readonly allowedMarkets: bigint;
  readonly minHoldSlots: bigint;
};

export async function createProfileIx(
  signer: TransactionSigner
): Promise<Instruction> {
  return nox.getInitializeTraderProfileInstructionAsync({
    payer: signer,
    authority: signer.address,
  });
}

export async function postTraderListingIx(
  signer: TransactionSigner,
  terms: nox.ListingTermsArgs,
  existing: boolean,
  open = true
): Promise<Instruction> {
  return existing
    ? nox.getUpdateListingInstructionAsync({ trader: signer, terms, open })
    : nox.getPostListingInstructionAsync({ trader: signer, terms });
}

export async function postInvestorListingIx(
  signer: TransactionSigner,
  terms: nox.InvestorListingTermsArgs,
  existing: boolean,
  open = true
): Promise<Instruction> {
  return existing
    ? nox.getUpdateInvestorListingInstructionAsync({
        investor: signer,
        terms,
        open,
      })
    : nox.getPostInvestorListingInstructionAsync({ investor: signer, terms });
}

async function usdcAta(owner: Address, mint: Address): Promise<Address> {
  const [ata] = await findAssociatedTokenPda({
    owner,
    mint,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });
  return ata;
}

/**
 * The next free `seq` for an investor → trader pair.
 *
 * An offer and its mandate share the seq, and both addresses must be unused, so this skips
 * every seq either an offer or a mandate already occupies.
 */
export function nextSeq(
  m: Marketplace,
  investor: Address,
  trader: Address
): number {
  const used = new Set<number>();
  for (const o of m.offers)
    if (o.data.investor === investor && o.data.trader === trader)
      used.add(o.data.seq);
  for (const x of m.mandates)
    if (x.data.investor === investor && x.data.trader === trader)
      used.add(x.data.seq);
  for (let s = 0; s < 256; s++) if (!used.has(s)) return s;
  throw new Error("every seq for this pair is in use");
}

export async function postOfferIx(args: {
  signer: TransactionSigner;
  trader: Address;
  seq: number;
  principal: bigint;
  rules: OfferRules;
  traderSplitBps: number;
  expiresAt: bigint;
  note: string;
  usdcMint: Address;
}): Promise<Instruction> {
  const offer = await noxPdas.findOffer(
    args.signer.address,
    args.trader,
    args.seq
  );
  return nox.getPostOfferInstructionAsync({
    investor: args.signer,
    trader: args.trader,
    offer,
    usdcMint: args.usdcMint,
    investorToken: await usdcAta(args.signer.address, args.usdcMint),
    seq: args.seq,
    principal: args.principal,
    rules: args.rules,
    traderSplitBps: args.traderSplitBps,
    expiresAt: args.expiresAt,
    note: args.note,
  });
}

export async function revokeOfferIx(
  signer: TransactionSigner,
  offer: Row<nox.MandateOffer>,
  usdcMint: Address
): Promise<Instruction> {
  return nox.getRevokeOfferInstructionAsync({
    investor: signer,
    offer: offer.address,
    usdcMint,
    investorToken: await usdcAta(signer.address, usdcMint),
  });
}

export async function acceptOfferIx(
  signer: TransactionSigner,
  offer: Row<nox.MandateOffer>,
  usdcMint: Address
): Promise<Instruction> {
  const mandate = await noxPdas.findMandate(
    offer.data.investor,
    signer.address,
    offer.data.seq
  );
  const mandateSigner = await noxPdas.findMandateSigner(mandate);
  // The SolFX account the mandate will trade through: SolFX's own PDA for the mandate's
  // signer, derived with SolFX's own helper rather than retyped.
  const [solfxUserAccount] = await findUserAccountPda({
    authority: mandateSigner,
  });
  return nox.getAcceptOfferInstructionAsync({
    trader: signer,
    offer: offer.address,
    mandate,
    solfxUserAccount,
    usdcMint,
  });
}

export function declineOfferIx(
  signer: TransactionSigner,
  offer: Address,
  reason: string
): Instruction {
  return nox.getDeclineOfferInstruction({ trader: signer, offer, reason });
}

export async function postRequestIx(args: {
  signer: TransactionSigner;
  investor: Address;
  wantedPrincipal: bigint;
  wantedSplitBps: number;
  note: string;
}): Promise<Instruction> {
  return nox.getPostRequestInstructionAsync({
    trader: args.signer,
    investorListing: await noxPdas.findInvestorListing(args.investor),
    request: await noxPdas.findRequest(args.signer.address, args.investor),
    wantedPrincipal: args.wantedPrincipal,
    wantedSplitBps: args.wantedSplitBps,
    note: args.note,
  });
}

export function closeRequestIx(
  signer: TransactionSigner,
  request: Row<nox.FundingRequest>
): Instruction {
  return nox.getCloseRequestInstruction({
    closer: signer,
    trader: request.data.trader,
    request: request.address,
  });
}

/**
 * Compute limit for marketplace transactions.
 *
 * The heaviest is `accept_offer`: 35,927–44,927 CU measured over eight runs, with a ceiling of
 * 90,000 asserted in `programs/noxfunds/tests/stage7.rs` (the spread is the bump search on its
 * three new accounts). This sits above that asserted ceiling, so a request can never be the
 * reason a valid transaction fails.
 */
export const MARKET_CU = 100_000;

// --- settlement ------------------------------------------------------------------------------

/**
 * End the mandate. Only the investor may ask, and it stops new trades rather than moving money.
 *
 * Open positions can still be closed afterwards — by the trader, or by anyone once the mandate is
 * winding down — which is why this and the payout are two instructions and not one.
 */
export function requestSettlementIx(
  signer: TransactionSigner,
  mandate: Address
): Instruction {
  return nox.getRequestSettlementInstruction({ investor: signer, mandate });
}

/**
 * Pay everyone out. **Permissionless**: the signer is whoever runs it, not whoever is owed.
 *
 * The investor therefore never waits on the trader, and the trader never waits on the investor.
 * The three token accounts are the ones the program checks by authority, so they are derived
 * here rather than chosen, and each is created idempotently first — a payee with no USDC account
 * would otherwise make the payout impossible for everyone.
 */
export async function claimSettlementIxs(
  signer: TransactionSigner,
  m: Marketplace,
  mandate: Row<nox.Mandate>
): Promise<Instruction[]> {
  if (!m.config) throw new Error("NOXFUNDS' config has not been read yet.");
  const mint = m.config.usdcMint;
  const [protocol] = await findProtocolPda();
  const [collateralVault] = await findCollateralVaultPda();
  const [mandateSigner] = [await noxPdas.findMandateSigner(mandate.address)];
  const [mandateVault, profile] = await Promise.all([
    noxPdas.findMandateVault(mandate.address),
    noxPdas.findProfile(mandate.data.trader),
  ]);
  const owners = [
    mandate.data.investor,
    mandate.data.trader,
    m.config.treasury,
  ] as const;
  const atas = await Promise.all(
    owners.map(async (owner) => {
      const [ata] = await findAssociatedTokenPda({
        mint,
        owner,
        tokenProgram: TOKEN_PROGRAM_ADDRESS,
      });
      return ata;
    })
  );
  const create = await Promise.all(
    owners.map((owner) =>
      getCreateAssociatedTokenIdempotentInstructionAsync({
        payer: signer,
        mint,
        owner,
      })
    )
  );
  return [
    ...create,
    nox.getClaimSettlementInstruction({
      settler: signer,
      config: await noxPdas.findNoxConfig(),
      mandate: mandate.address,
      mandateSigner,
      protocol,
      userAccount: mandate.data.solfxUserAccount,
      usdcMint: mint,
      collateralVault,
      mandateVault,
      investorToken: atas[0]!,
      traderToken: atas[1]!,
      treasuryToken: atas[2]!,
      traderProfile: profile,
    }),
  ];
}

/**
 * What a payout would be, by the same rule the program applies.
 *
 * 5% of **gross** profit to the protocol first, then the trader's share of what remains, and the
 * investor takes the rest — principal included. The fee rounds up and the trader's share rounds
 * down, both against the party being paid, and the investor receives the remainder, so the three
 * always sum to the whole. On a loss there is no fee and no trader share.
 *
 * A preview, not a promise: the real split runs against the equity at the moment of the claim.
 */
export function previewSplit(
  finalEquity: bigint,
  principal: bigint,
  feeBps: number,
  traderBps: number
): { investor: bigint; trader: bigint; protocol: bigint; gross: bigint } {
  const gross = finalEquity > principal ? finalEquity - principal : 0n;
  if (gross === 0n) {
    return { investor: finalEquity, trader: 0n, protocol: 0n, gross: 0n };
  }
  const fee = (gross * BigInt(feeBps) + BPS - 1n) / BPS; // ceil, toward the charger
  const protocol = fee > gross ? gross : fee;
  const net = gross - protocol;
  const trader = (net * BigInt(traderBps)) / BPS; // floor, toward the payer
  return { investor: finalEquity - protocol - trader, trader, protocol, gross };
}

/**
 * The display name a wallet has given itself, or "" if it has never listed.
 *
 * Read from whichever listing the address owns — a trader's or an investor's. A name is a
 * convenience and never an identity: two wallets may pick the same one, and nothing on chain
 * stops them, which is exactly why the UI shows the address beside it rather than instead of it.
 */
export function nicknameOf(m: Marketplace, who: Address): string {
  const t = m.traderListings.find((l) => l.data.trader === who);
  if (t) return noteText(t.data.nickname, t.data.nicknameLen);
  const i = m.investorListings.find((l) => l.data.investor === who);
  return i ? noteText(i.data.nickname, i.data.nicknameLen) : "";
}

// --- the evaluation --------------------------------------------------------------------------

/**
 * The evaluation's fixed rules, from `programs/noxfunds/src/constants.rs`.
 *
 * Constants in the program rather than config fields, deliberately: a threshold an admin can move
 * is a threshold they can move after seeing who it would promote. Mirrored here so the dashboard
 * can show a trader what they are being judged against before they stake anything — and if the
 * program ever changes one, this copy is the thing that has to change with it.
 */
export const EVAL = {
  stake: 50_000_000n, // $50, the only real money in an evaluation
  minAccount: 10_000_000_000n, // $10,000
  maxAccount: 200_000_000_000n, // $200,000
  targetBps: [800, 500] as const, // Phase 1 then Phase 2
  maxDailyLossBps: 300,
  maxDrawdownBps: 600,
  maxRiskBps: 100,
  minTrades: 10,
  minDays: 5,
  minHoldSecs: 600,
  minAvgHoldSecs: 2_700,
  consistencyBps: 5_000, // no single day may be more than half the target
  maxOpen: 5,
} as const;

export const EVAL_STATE_NAME = ["Active", "Passed", "Failed"] as const;

/**
 * `notional = size x price / NOTIONAL_DIVISOR`, at BASE_PRECISION 1e9 x PRICE_PRECISION 1e9
 * against a quote at 1e6. The program's own relation — sizing a ticket any other way puts the
 * notional out by a factor of a thousand in whichever direction the mistake went.
 */
export const NOTIONAL_DIVISOR = 1_000_000_000_000n;

/** Where a trader is against the rules of their current stage. Every figure integer-only. */
export type EvalProgress = {
  readonly stage: number;
  readonly equity: bigint;
  readonly targetEquity: bigint;
  readonly profitBps: bigint;
  readonly targetBps: number;
  readonly drawdownBps: bigint;
  readonly dailyLossBps: bigint;
  readonly trades: number;
  readonly days: number;
  /** The single thing still standing between this trader and the next stage. */
  readonly blocker: string | null;
};

export function evalProgress(e: nox.Evaluation): EvalProgress {
  const size = e.accountSize === 0n ? 1n : e.accountSize;
  const equity = e.balance > 0n ? e.balance : 0n;
  const stage = e.stage < 1 ? 1 : e.stage;
  const targetBps = EVAL.targetBps[stage - 1] ?? EVAL.targetBps[1];
  // The program ceils the target: it is a threshold to reach, and flooring would pass a stage
  // fractionally short of it. `targetEquity` is the `goal` in `claim_stage_pass`.
  const target = ceilDiv(size * BigInt(targetBps), BPS);
  const targetEquity = size + target;
  const profitBps = equity > size ? ((equity - size) * BPS) / size : 0n;
  const drawdownBps =
    e.peakEquity > 0n && equity < e.peakEquity
      ? ((e.peakEquity - equity) * BPS) / e.peakEquity
      : 0n;
  const dailyLossBps =
    e.dayStartEquity > 0n && e.dayPnl < 0n
      ? (-e.dayPnl * BPS) / e.dayStartEquity
      : 0n;
  const avgHold =
    e.voluntaryCloses > 0
      ? e.voluntaryHoldSecs / BigInt(e.voluntaryCloses)
      : null;

  // One blocker, the nearest one — a list of nine numbers tells a trader nothing about what to
  // do. Ordered as `claim_stage_pass` checks, so the sentence names the refusal that would come
  // back, and every requirement it checks is represented: a "claim" button that is enabled on a
  // stage the program will not pass is worse than one that stays disabled with a reason.
  const consistencyCap = (target * BigInt(EVAL.consistencyBps)) / BPS;
  const blocker =
    e.state !== 0
      ? `This evaluation is ${EVAL_STATE_NAME[e.state] ?? "over"}`
      : e.openPositions > 0
        ? `Close ${e.openPositions} open position${e.openPositions > 1 ? "s" : ""}. A stage passes flat`
        : drawdownBps > BigInt(EVAL.maxDrawdownBps)
          ? `Drawdown ${Number(drawdownBps) / 100}% is past the ${EVAL.maxDrawdownBps / 100}% limit`
          : equity < targetEquity
            ? `Profit ${Number(profitBps) / 100}% of the ${targetBps / 100}% target`
            : e.trades < EVAL.minTrades
              ? `${e.trades} of ${EVAL.minTrades} trades`
              : e.tradingDays < EVAL.minDays
                ? `${e.tradingDays} of ${EVAL.minDays} trading days`
                : avgHold !== null && avgHold < BigInt(EVAL.minAvgHoldSecs)
                  ? `Average hold ${avgHold / 60n} min of ${EVAL.minAvgHoldSecs / 60} min`
                  : e.bestDayPnl > consistencyCap
                    ? `One day carried ${Number((e.bestDayPnl * BPS) / (target > 0n ? target : 1n)) / 100}% of the target; no day may carry more than ${EVAL.consistencyBps / 100}%`
                    : null;

  return {
    stage,
    equity,
    targetEquity,
    profitBps,
    targetBps,
    drawdownBps,
    dailyLossBps,
    trades: e.trades,
    days: e.tradingDays,
    blocker,
  };
}

export async function startEvaluationIx(
  signer: TransactionSigner,
  usdcMint: Address,
  seq: number,
  accountSize: bigint
): Promise<Instruction[]> {
  const [traderToken] = await findAssociatedTokenPda({
    mint: usdcMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });
  return [
    await getCreateAssociatedTokenIdempotentInstructionAsync({
      payer: signer,
      mint: usdcMint,
      owner: signer.address,
    }),
    await nox.getStartEvaluationInstructionAsync({
      trader: signer,
      evaluation: await noxPdas.findEvaluation(signer.address, seq),
      usdcMint,
      traderToken,
      seq,
      accountSize,
    }),
  ];
}

export async function evalOpenIx(args: {
  signer: TransactionSigner;
  evaluation: Address;
  marketIndex: number;
  market: Address;
  priceUpdate: Address;
  nonce: number;
  direction: nox.DirectionArgs;
  sizeBase: bigint;
  stopLossPrice: bigint;
}): Promise<Instruction> {
  return nox.getEvalOpenPositionInstructionAsync({
    trader: args.signer,
    evaluation: args.evaluation,
    virtualPosition: await noxPdas.findVirtualPosition(
      args.evaluation,
      args.marketIndex,
      args.nonce
    ),
    market: args.market,
    priceUpdate: args.priceUpdate,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    direction: args.direction,
    sizeBase: args.sizeBase,
    stopLossPrice: args.stopLossPrice,
  });
}

export function evalCloseIx(args: {
  signer: TransactionSigner;
  evaluation: Address;
  virtualPosition: Address;
  market: Address;
  priceUpdate: Address;
}): Instruction {
  return nox.getEvalClosePositionInstruction({
    trader: args.signer,
    evaluation: args.evaluation,
    virtualPosition: args.virtualPosition,
    market: args.market,
    priceUpdate: args.priceUpdate,
  });
}

/**
 * Mark every open simulated position and judge the loss limits. **Permissionless.**
 *
 * One `(position, market, price)` triple per open position, in `remaining_accounts` — the same
 * shape the funded crank takes. Evaluations only allow single-leg markets, so a triple is always
 * the whole group here.
 */
export function evalObserveIx(
  signer: TransactionSigner,
  evaluation: Address,
  legs: readonly {
    position: Address;
    market: Address;
    priceUpdate: Address;
  }[]
): Instruction {
  const ix = nox.getEvalObserveEquityInstruction({
    observer: signer,
    evaluation,
  });
  return {
    ...ix,
    accounts: [
      ...(ix.accounts ?? []),
      ...legs.flatMap((l) => [
        { address: l.position, role: 0 as const },
        { address: l.market, role: 0 as const },
        { address: l.priceUpdate, role: 0 as const },
      ]),
    ],
  };
}

export async function claimStagePassIx(
  signer: TransactionSigner,
  usdcMint: Address,
  evaluation: Address
): Promise<Instruction> {
  const [traderToken] = await findAssociatedTokenPda({
    mint: usdcMint,
    owner: signer.address,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  });
  return nox.getClaimStagePassInstructionAsync({
    trader: signer,
    evaluation,
    usdcMint,
    traderToken,
  });
}

export function abandonEvaluationIx(
  signer: TransactionSigner,
  evaluation: Address
): Instruction {
  return nox.getAbandonEvaluationInstruction({ trader: signer, evaluation });
}

// --- funded trading ---------------------------------------------------------------------------
//
// What a trader does with an investor's money, from the browser. The terminal's own ticket
// cannot do this: it signs as the wallet, and a funded position's authority is the mandate
// signer — a PDA with no private key, which only `noxfunds` can sign for.

/** `funded_open_position` runs two CPIs. The CLI budgets 200k and lands at well under it. */
export const FUNDED_OPEN_CU = 250_000;
export const FUNDED_CLOSE_CU = 200_000;
export const FUNDED_TRIGGER_CU = 120_000;

/**
 * Every reason `check_rules` would refuse this trade, in the order it checks them.
 *
 * Not a guess at the program's answer — the same arithmetic in the same units, so the panel
 * can name the refusal before a transaction is signed rather than after it fails. Where the
 * program rounds a measurement up because the trade must stay *under* a limit, so does this.
 *
 * It cannot replace the program's check and is not meant to: the price moves between this and
 * the fill, which is exactly why `NOTIONAL_HEADROOM_BPS` exists in the CLI.
 */
export function ruleRefusal(args: {
  mandate: nox.Mandate;
  marketIndex: number;
  direction: nox.DirectionArgs;
  /** Oracle price at PRICE_PRECISION. */
  price: bigint;
  sizeBase: bigint;
  stopPrice: bigint;
  /** Already-open positions on this mandate. */
  openPositions: number;
}): string | null {
  const m = args.mandate;
  if (m.state !== 0) return "this mandate is no longer active";
  if ((m.allowedMarkets & (1n << BigInt(args.marketIndex))) === 0n)
    return "the investor did not permit this market";
  if (args.openPositions >= m.maxConcurrentPositions)
    return `already holding ${m.maxConcurrentPositions} positions, the mandate's limit`;

  if (args.price <= 0n) return "no oracle price";
  if (args.stopPrice <= 0n) return "a stop is mandatory";
  const long = args.direction === nox.Direction.Long;
  if (long ? args.stopPrice >= args.price : args.stopPrice <= args.price)
    return `a ${long ? "long" : "short"}'s stop goes ${long ? "below" : "above"} the price`;

  const distance =
    args.stopPrice > args.price
      ? args.stopPrice - args.price
      : args.price - args.stopPrice;
  const distanceBps = ceilDiv(distance * 10_000n, args.price);
  if (distanceBps > BigInt(m.maxStopDistanceBps))
    return `the stop is ${Number(distanceBps) / 100}% away; the mandate allows ${m.maxStopDistanceBps / 100}%`;

  // Ceiling, because `notional_in_quote` ceils. Flooring here would call a trade one unit
  // over the ceiling compliant and promise a fill the program refuses — the exact failure a
  // mirror is supposed to prevent. Sizing floors; measuring against a limit ceils.
  const notional = ceilDiv(args.sizeBase * args.price, NOTIONAL_DIVISOR);
  if (notional > m.maxTradeNotional)
    return `$${fmtUnits(notional)} exceeds the $${fmtUnits(m.maxTradeNotional)} per-trade ceiling`;
  if (m.openNotional + notional > m.maxTotalNotional)
    return `$${fmtUnits(m.openNotional + notional)} open would exceed the $${fmtUnits(m.maxTotalNotional)} book limit`;

  // Risk at the stop, the rule no centralized firm can enforce before the fill.
  const risk = ceilDiv(args.sizeBase * distance, NOTIONAL_DIVISOR);
  const equity = m.peakEquity > 0n ? m.peakEquity : 1n;
  const riskBps = ceilDiv(risk * 10_000n, equity);
  if (riskBps > BigInt(m.maxRiskPerTradeBps))
    return `risking ${Number(riskBps) / 100}% at the stop; the mandate allows ${m.maxRiskPerTradeBps / 100}%`;

  return null;
}

/** Ceiling division on non-negative bigints. Matches `solfx_math::fixed::mul_div_ceil`. */
function ceilDiv(n: bigint, d: bigint): bigint {
  if (d === 0n) return 0n;
  return (n + d - 1n) / d;
}

/** A USDC amount, whole dollars, for a sentence rather than a table. */
function fmtUnits(amount: bigint): string {
  return (amount / 1_000_000n).toLocaleString();
}

/**
 * The slippage bound the program enforces: a maximum buying, a minimum selling.
 *
 * Reimplemented here rather than imported from `@/lib/trade` because that module is the
 * terminal's, and the shape it wants is a `Direction` from the SolFX client while this side
 * speaks NOXFUNDS'. The arithmetic is identical and `price_limit: 0` is a bound of zero, not
 * an opt-out, on both.
 */
export function fundedPriceLimit(
  direction: nox.DirectionArgs,
  price: bigint,
  slippageBps: number
): bigint {
  const delta = (price * BigInt(slippageBps)) / 10_000n;
  return direction === nox.Direction.Long ? price + delta : price - delta;
}

type SolfxLegs = {
  readonly priceUpdate: Address;
  readonly secondaryPriceUpdate?: Address;
  readonly quoteConversionPriceUpdate?: Address;
};

/** Every SolFX account a funded trade touches, derived once from the mandate. */
async function solfxAccounts(
  mandate: Address,
  marketIndex: number,
  nonce: number
) {
  const mandateSigner = await noxPdas.findMandateSigner(mandate);
  const [protocol] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({ authority: mandateSigner });
  const [market] = await findMarketPda({ marketIndex });
  const [position] = await findPositionPda({ userAccount, marketIndex, nonce });
  const [collateralVault] = await findCollateralVaultPda();
  const [lpPool] = await findLpPoolPda();
  const [lpVault] = await findLpVaultPda();
  const [insuranceFund] = await findInsuranceFundPda();
  const [insuranceVault] = await findInsuranceVaultPda();
  const [feeVault] = await findFeeVaultPda();
  return {
    mandateSigner,
    protocol,
    userAccount,
    market,
    position,
    collateralVault,
    lpPool,
    lpVault,
    insuranceFund,
    insuranceVault,
    feeVault,
  };
}

/**
 * The mandate's SolFX authority.
 *
 * A dataless, system-owned PDA with no private key. It is what `loadPositions` should be
 * pointed at to see a mandate's positions: the trader's own wallet holds none of them, and
 * reading the trader's account instead shows an empty book while the mandate is fully invested.
 */
export async function mandateAuthority(mandate: Address): Promise<Address> {
  return noxPdas.findMandateSigner(mandate);
}

export async function fundedOpenIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  marketIndex: number;
  nonce: number;
  direction: nox.DirectionArgs;
  sizeBase: bigint;
  collateral: bigint;
  priceLimit: bigint;
  stopLossPrice: bigint;
  legs: SolfxLegs;
}): Promise<Instruction> {
  const a = await solfxAccounts(args.mandate, args.marketIndex, args.nonce);
  // One order id per position is enough while the stop is the only order placed at open; a
  // take-profit added later takes a different one, and `init` refuses a collision anyway.
  const orderId = args.nonce;
  const [triggerOrder] = await findTriggerOrderPda({
    position: a.position,
    orderId,
  });
  return nox.getFundedOpenPositionInstructionAsync({
    trader: args.signer,
    mandate: args.mandate,
    protocol: a.protocol,
    userAccount: a.userAccount,
    market: a.market,
    position: a.position,
    triggerOrder,
    collateralVault: a.collateralVault,
    lpPool: a.lpPool,
    lpVault: a.lpVault,
    insuranceFund: a.insuranceFund,
    insuranceVault: a.insuranceVault,
    feeVault: a.feeVault,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    direction: args.direction,
    sizeBase: args.sizeBase,
    collateral: args.collateral,
    priceLimit: args.priceLimit,
    orderId,
    stopLossPrice: args.stopLossPrice,
  });
}

export async function fundedCloseIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  marketIndex: number;
  nonce: number;
  priceLimit: bigint;
  legs: SolfxLegs;
}): Promise<Instruction> {
  const a = await solfxAccounts(args.mandate, args.marketIndex, args.nonce);
  return nox.getFundedClosePositionInstructionAsync({
    trader: args.signer,
    // The close is what writes the trade onto the trader's permanent record, so the profile is
    // an account of the instruction rather than something an indexer reconstructs afterwards.
    traderProfile: await noxPdas.findProfile(args.signer.address),
    mandate: args.mandate,
    protocol: a.protocol,
    userAccount: a.userAccount,
    market: a.market,
    position: a.position,
    collateralVault: a.collateralVault,
    lpPool: a.lpPool,
    lpVault: a.lpVault,
    insuranceFund: a.insuranceFund,
    insuranceVault: a.insuranceVault,
    feeVault: a.feeVault,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    priceLimit: args.priceLimit,
  });
}

/**
 * A take-profit on an open funded position.
 *
 * Deliberately not part of the open: the stop must be atomic with the fill because a gap would
 * leave the mandate unprotected, and a target protects nobody. So this is a second transaction,
 * placeable and movable at any time.
 */
export async function fundedTakeProfitIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  marketIndex: number;
  nonce: number;
  orderId: number;
  triggerPrice: bigint;
  sizeBase: bigint;
  legs: SolfxLegs;
}): Promise<Instruction> {
  const a = await solfxAccounts(args.mandate, args.marketIndex, args.nonce);
  const [triggerOrder] = await findTriggerOrderPda({
    position: a.position,
    orderId: args.orderId,
  });
  return nox.getFundedPlaceTakeProfitInstructionAsync({
    trader: args.signer,
    mandate: args.mandate,
    protocol: a.protocol,
    userAccount: a.userAccount,
    market: a.market,
    position: a.position,
    triggerOrder,
    priceUpdate: args.legs.priceUpdate,
    secondaryPriceUpdate: args.legs.secondaryPriceUpdate,
    quoteConversionPriceUpdate: args.legs.quoteConversionPriceUpdate,
    orderId: args.orderId,
    triggerPrice: args.triggerPrice,
    sizeBase: args.sizeBase,
  });
}

/**
 * Cancel a resting order and reclaim its rent to the mandate signer.
 *
 * `funded_cancel_stop` is generic over `order_id`, so this cancels a take-profit as readily as
 * a stop. Without it a trader who moved a target twice would strand 2,039,280 lamports a time.
 */
export async function fundedCancelOrderIx(args: {
  signer: TransactionSigner;
  mandate: Address;
  marketIndex: number;
  nonce: number;
  orderId: number;
}): Promise<Instruction> {
  const a = await solfxAccounts(args.mandate, args.marketIndex, args.nonce);
  const [triggerOrder] = await findTriggerOrderPda({
    position: a.position,
    orderId: args.orderId,
  });
  return nox.getFundedCancelStopInstructionAsync({
    trader: args.signer,
    mandate: args.mandate,
    triggerOrder,
    marketIndex: args.marketIndex,
    nonce: args.nonce,
    orderId: args.orderId,
  });
}

/**
 * What a mandate needs before its first trade, and whether it has it.
 *
 * Three steps sit between `accept_offer` and `funded_open_position`, and the CLI's ten-step
 * lifecycle does all three without comment. Skipping them produces `AccountNotInitialized` on
 * `user_account` from inside `solfx-core` — an error that names an account the trader never
 * chose, four programs deep, on a mandate whose vault visibly holds their money. Measured on
 * devnet 2026-09-20: mandate `C5TjG5vv…` held $500 and could not open a position.
 */
export type MandateReadiness = {
  /** Lamports the mandate signer needs and does not have. */
  readonly needsLamports: bigint;
  /** True when the mandate has no SolFX `UserAccount` yet. */
  readonly needsSolfxAccount: boolean;
  /** USDC still sitting in the mandate vault rather than in SolFX collateral. */
  readonly idleVault: bigint;
  readonly ready: boolean;
};

/**
 * The mandate signer pays the `UserAccount`'s rent and each trigger order's, and is refunded
 * when they close — so it needs a working balance, not a one-off fee. `nox lifecycle` tops it
 * up to the same figure for the same reason.
 *
 * It is not recoverable: nothing in the program sweeps the signer, and a PDA has no key. That
 * is 0.02 SOL per mandate, said out loud rather than buried.
 */
export const MANDATE_SIGNER_LAMPORTS = 20_000_000n;

export async function readMandateReadiness(
  rpc: Rpc<SolanaRpcApi>,
  mandate: Address,
  vaultBalance: bigint
): Promise<MandateReadiness> {
  const signer = await noxPdas.findMandateSigner(mandate);
  const [userAccount] = await findUserAccountPda({ authority: signer });
  const { value } = await rpc
    .getMultipleAccounts([signer, userAccount], {
      commitment: READ_COMMITMENT,
      encoding: "base64",
    })
    .send();
  const have = value[0]?.lamports ?? 0n;
  const needsLamports =
    have >= MANDATE_SIGNER_LAMPORTS ? 0n : MANDATE_SIGNER_LAMPORTS - have;
  const needsSolfxAccount = !value[1];
  return {
    needsLamports,
    needsSolfxAccount,
    idleVault: vaultBalance,
    ready: needsLamports === 0n && !needsSolfxAccount && vaultBalance === 0n,
  };
}

/**
 * Everything missing, in one transaction.
 *
 * Ordered as the CPIs require: lamports before the rent they pay, the account before the
 * deposit into it. Each step is skipped when it is already done, so this is safe to re-send —
 * the same idempotence `init-protocol` and `price-poster` have, and for the same reason: an
 * operator step that cannot be repeated is one that cannot be recovered.
 */
export async function prepareMandateIxs(args: {
  signer: TransactionSigner;
  mandate: Address;
  usdcMint: Address;
  readiness: MandateReadiness;
}): Promise<Instruction[]> {
  const { readiness: r } = args;
  const mandateSigner = await noxPdas.findMandateSigner(args.mandate);
  const [protocol] = await findProtocolPda();
  const [userAccount] = await findUserAccountPda({ authority: mandateSigner });
  const out: Instruction[] = [];

  if (r.needsLamports > 0n) {
    out.push(transferSolIx(args.signer, mandateSigner, r.needsLamports));
  }
  if (r.needsSolfxAccount) {
    out.push(
      await nox.getCreateSolfxAccountInstructionAsync({
        payer: args.signer,
        mandate: args.mandate,
        protocol,
        userAccount,
      })
    );
  }
  if (r.idleVault > 0n) {
    const [collateralVault] = await findCollateralVaultPda();
    out.push(
      await nox.getFundSolfxCollateralInstructionAsync({
        payer: args.signer,
        mandate: args.mandate,
        protocol,
        userAccount,
        collateralMint: args.usdcMint,
        collateralVault,
        amount: r.idleVault,
      })
    );
  }
  return out;
}

/** `create_solfx_account` and `fund_solfx_collateral` each CPI into `solfx-core`. */
export const PREPARE_MANDATE_CU = 160_000;

const SYSTEM_PROGRAM = "11111111111111111111111111111111" as Address;

/**
 * A plain SOL transfer, encoded here rather than pulled from `@solana-program/system`.
 *
 * That package is not a declared dependency of this app — it resolves only because it is
 * hoisted under `@solana-program/token`, so a dependency bump would break the build with no
 * warning. The layout is `u32` instruction index 2 followed by the lamports little-endian, and
 * it is fixed by the runtime rather than by a library version, so there is nothing here to
 * drift against.
 */
function transferSolIx(
  from: TransactionSigner,
  to: Address,
  amount: bigint
): Instruction {
  const data = new Uint8Array(12);
  const view = new DataView(data.buffer);
  view.setUint32(0, 2, true);
  view.setBigUint64(4, amount, true);
  return {
    programAddress: SYSTEM_PROGRAM,
    accounts: [
      // The signer instance travels with the meta: kit refuses a transaction whose signers do
      // not match, and two instances for one address count as two signers.
      { address: from.address, role: 3 as const, ...{ signer: from } },
      { address: to, role: 1 as const },
    ],
    data,
  };
}
