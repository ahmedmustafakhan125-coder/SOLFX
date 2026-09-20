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
import { findUserAccountPda, nox, noxPdas } from "@solfx/client";
import {
  findAssociatedTokenPda,
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

type Row<T> = { readonly address: Address; readonly data: T };

export type Marketplace = {
  readonly config: nox.NoxConfig | null;
  readonly profiles: ReadonlyMap<Address, Row<nox.TraderProfile>>;
  readonly traderListings: readonly Row<nox.TraderListing>[];
  readonly investorListings: readonly Row<nox.InvestorListing>[];
  readonly offers: readonly Row<nox.MandateOffer>[];
  readonly requests: readonly Row<nox.FundingRequest>[];
  readonly mandates: readonly Row<nox.Mandate>[];
  /** What each mandate's vault holds now, keyed by mandate address. */
  readonly vaults: ReadonlyMap<Address, bigint>;
  /** Cluster time when this was read, for judging offer expiry. */
  readonly now: bigint;
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
    vaults: await readVaults(rpc, mandates),
    now,
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
      "Drawdown breached — no new trades. Anyone may close it out, then the investor is paid.",
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
