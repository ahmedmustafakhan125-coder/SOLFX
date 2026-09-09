/**
 * Reading the introducing-broker programme.
 *
 * # Two programs, and only one of them has to be live
 *
 * The referral *pool* lives in `solfx-core`: `Protocol.total_referral_accrued`,
 * `total_referral_claimed`, `fee_split_referral_bps` and `referral_authority` are all real
 * today, and a trader's own `referrer` and `referral_fees_generated` sit on their
 * `UserAccount`. The referral *programme* — tiers, IB accounts, claims — is a separate
 * program that must be initialised before any of it exists.
 *
 * So this reads both and reports them separately, rather than showing an empty dashboard
 * when the truth is "the ledger is accruing, the programme that pays it out is not deployed
 * on this cluster yet". `Protocol.referral_authority` being the default pubkey is that fact,
 * stated by the protocol itself.
 */
import {
  fetchProtocol,
  findProtocolPda,
  findUserAccountPda,
  referral,
} from "@solfx/client";
import type { Address, Rpc, SolanaRpcApi } from "@solana/kit";

/** `Pubkey::default()` — what `referral_authority` holds while payouts are disabled. */
const DEFAULT_PUBKEY = "11111111111111111111111111111111" as Address;

/** The referral pool as `solfx-core` sees it. Always readable. */
export type ReferralPool = {
  /** Share of every fee earmarked for referrals, in bps. */
  readonly splitBps: number;
  readonly totalAccrued: bigint;
  readonly totalClaimed: bigint;
  /** The referral programme PDA the admin registered, or null while payouts are disabled. */
  readonly authority: Address | null;
};

/** The referral programme's own state, or null when it has never been initialised. */
export type ProgrammeState = {
  readonly admin: Address;
  readonly overrideBps: number;
  readonly poolSplitBps: number;
  readonly totalAccrued: bigint;
  readonly totalClaimed: bigint;
  readonly ibCount: number;
};

/** What the connected wallet is, on each side of the programme. */
export type MyStanding = {
  /** The IB who introduced this wallet, or null. Bound once at account creation. */
  readonly referrer: Address | null;
  /** Referral-pool money this wallet's trading has generated for whoever introduced them. */
  readonly feesGenerated: bigint;
  readonly lifetimeVolume: bigint;
  readonly thirtyDayVolume: bigint;
  /** Present only once this wallet has registered as an IB. */
  readonly ib: referral.IbAccount | null;
};

export type PartnersStatus = {
  readonly pool: ReferralPool;
  readonly programme: ProgrammeState | null;
  readonly me: MyStanding | null;
};

export async function readPartnersStatus(
  rpc: Rpc<SolanaRpcApi>,
  owner: Address | null
): Promise<PartnersStatus> {
  const [protocolPda] = await findProtocolPda();
  const [configPda] = await referral.findReferralConfigPda();
  const protocol = await fetchProtocol(rpc, protocolPda);

  const authority =
    protocol.data.referralAuthority === DEFAULT_PUBKEY
      ? null
      : protocol.data.referralAuthority;

  const pool: ReferralPool = {
    splitBps: protocol.data.feeSplitReferralBps,
    totalAccrued: protocol.data.totalReferralAccrued,
    totalClaimed: protocol.data.totalReferralClaimed,
    authority,
  };

  // The config and, when a wallet is connected, that wallet's user and IB accounts. All are
  // legitimately absent, so they are read raw — a decoder that throws would turn "not
  // registered" into an error.
  const keys: Address[] = [configPda];
  if (owner) {
    const [userAccount] = await findUserAccountPda({ authority: owner });
    const [ibAccount] = await referral.findIbAccountPda({ authority: owner });
    keys.push(userAccount, ibAccount);
  }

  const { value: accounts } = await rpc
    .getMultipleAccounts(keys, { encoding: "base64" })
    .send();

  const configRaw = accounts[0];
  const programme: ProgrammeState | null = configRaw
    ? (() => {
        const c = referral
          .getReferralConfigDecoder()
          .decode(base64ToBytes(configRaw.data[0]));
        return {
          admin: c.admin,
          overrideBps: c.overrideBps,
          poolSplitBps: c.poolSplitBps,
          totalAccrued: c.totalAccrued,
          totalClaimed: c.totalClaimed,
          ibCount: c.ibCount,
        };
      })()
    : null;

  if (!owner) return { pool, programme, me: null };

  const userRaw = accounts[1];
  const ibRaw = accounts[2];

  let me: MyStanding = {
    referrer: null,
    feesGenerated: 0n,
    lifetimeVolume: 0n,
    thirtyDayVolume: 0n,
    ib: null,
  };

  if (userRaw) {
    const { getUserAccountDecoder } = await import("@solfx/client");
    const u = getUserAccountDecoder().decode(base64ToBytes(userRaw.data[0]));
    me = {
      ...me,
      referrer: u.referrer === DEFAULT_PUBKEY ? null : u.referrer,
      feesGenerated: u.referralFeesGenerated,
      lifetimeVolume: u.lifetimeVolume,
      thirtyDayVolume: u.thirtyDayVolume,
    };
  }

  if (ibRaw) {
    me = {
      ...me,
      ib: referral.getIbAccountDecoder().decode(base64ToBytes(ibRaw.data[0])),
    };
  }

  return { pool, programme, me };
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
