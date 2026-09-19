/**
 * NOXFUNDS addresses Codama cannot generate.
 *
 * The IDL describes a PDA only when every seed is an account or a constant. A `seq: u8` passed
 * as an instruction argument is neither, so `Mandate` and `MandateOffer` get no generated
 * helper, and anything added since the IDL was last published has none either.
 *
 * The seed strings are therefore written out here — once, in this one file, and each is pinned
 * against a real account on devnet in `__tests__/nox.test.ts`. A mistyped seed derives a
 * perfectly valid address that simply holds nothing, which reads in the UI as "no data" rather
 * than as a bug. The test is what turns that into a failure.
 */
import {
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
} from "@solana/kit";

export const NOXFUNDS_PROGRAM_ADDRESS =
  "9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx" as Address<"9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx">;

/** Byte-for-byte the constants in `programs/noxfunds/src/constants.rs`. */
export const NOX_SEEDS = {
  config: "config",
  mandate: "mandate",
  signer: "signer",
  vault: "vault",
  trader: "trader",
  listing: "listing",
  offer: "offer",
  offerVault: "offer_vault",
  investorListing: "inv_listing",
  request: "request",
} as const;

const utf8 = new TextEncoder();
const addr = getAddressEncoder();

async function pda(seeds: Uint8Array[]): Promise<Address> {
  const [address] = await getProgramDerivedAddress({
    programAddress: NOXFUNDS_PROGRAM_ADDRESS,
    seeds,
  });
  return address;
}

const tag = (s: string) => utf8.encode(s);
const seqByte = (seq: number) => {
  if (!Number.isInteger(seq) || seq < 0 || seq > 255) {
    throw new RangeError(`seq must be a u8, got ${seq}`);
  }
  return Uint8Array.of(seq);
};

export const findNoxConfig = async () => pda([tag(NOX_SEEDS.config)]);

export const findProfile = async (trader: Address) =>
  pda([tag(NOX_SEEDS.trader), addr.encode(trader) as Uint8Array]);

export const findMandate = async (
  investor: Address,
  trader: Address,
  seq: number,
) =>
  pda([
    tag(NOX_SEEDS.mandate),
    addr.encode(investor) as Uint8Array,
    addr.encode(trader) as Uint8Array,
    seqByte(seq),
  ]);

export const findMandateSigner = async (mandate: Address) =>
  pda([tag(NOX_SEEDS.signer), addr.encode(mandate) as Uint8Array]);

export const findMandateVault = async (mandate: Address) =>
  pda([tag(NOX_SEEDS.vault), addr.encode(mandate) as Uint8Array]);

export const findTraderListing = async (trader: Address) =>
  pda([tag(NOX_SEEDS.listing), addr.encode(trader) as Uint8Array]);

export const findOffer = async (
  investor: Address,
  trader: Address,
  seq: number,
) =>
  pda([
    tag(NOX_SEEDS.offer),
    addr.encode(investor) as Uint8Array,
    addr.encode(trader) as Uint8Array,
    seqByte(seq),
  ]);

export const findOfferVault = async (offer: Address) =>
  pda([tag(NOX_SEEDS.offerVault), addr.encode(offer) as Uint8Array]);

export const findInvestorListing = async (investor: Address) =>
  pda([tag(NOX_SEEDS.investorListing), addr.encode(investor) as Uint8Array]);

export const findRequest = async (trader: Address, investor: Address) =>
  pda([
    tag(NOX_SEEDS.request),
    addr.encode(trader) as Uint8Array,
    addr.encode(investor) as Uint8Array,
  ]);
