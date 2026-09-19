//! Seeds and fixed parameters.
//!
//! Every seed is a constant here and never a retyped byte string at a call site — the same
//! rule `solfx-core`'s `constants.rs` follows, for the same reason: a mistyped seed derives a
//! different account and fails at runtime rather than at compile time.

/// `NoxConfig` — one per deployment.
pub const CONFIG_SEED: &[u8] = b"config";
/// `Mandate` — `["mandate", investor, trader, seq]`.
pub const MANDATE_SEED: &[u8] = b"mandate";
/// The dataless PDA that is the SolFX authority — `["signer", mandate]`.
///
/// Derived from the mandate rather than the trader: one trader may hold several mandates from
/// different investors, and each must custody its capital separately.
pub const MANDATE_SIGNER_SEED: &[u8] = b"signer";
/// The mandate's USDC token account — `["vault", mandate]`.
pub const MANDATE_VAULT_SEED: &[u8] = b"vault";

/// Basis-point scale, matching `solfx-math`.
pub const BPS: u64 = 10_000;

/// The protocol's share of gross profit, in bps. **5%**, flat.
///
/// §5.1's worked example and the Decisions table both say 5% of gross, leaving the 70/30 split
/// untouched on what remains. §4.2 once carried a tiered 10/8/6/4 column; it was withdrawn on
/// 2026-09-15 as a leftover from a pre-5% draft. Stored on `NoxConfig` so a tiered schedule can
/// be introduced later without a program upgrade.
pub const DEFAULT_PROTOCOL_FEE_BPS: u16 = 500;

/// The trader's share of net profit, in bps. **70%**, leaving 30% to the investor.
pub const DEFAULT_TRADER_SPLIT_BPS: u16 = 7_000;

/// `TraderProfile` — `["trader", authority]`. One per trader, across every mandate they hold.
pub const TRADER_SEED: &[u8] = b"trader";

// --- tier thresholds (Part 4.1) -----------------------------------------------------------
//
// Constants rather than `NoxConfig` fields, deliberately. A threshold an admin can move is a
// threshold an admin can move *after* seeing who it would promote, and the whole claim of this
// programme is that the track record is not curated. Changing one is a program upgrade, which
// is public and versioned. The plan floated storing them on `NoxConfig`; this is the stricter
// reading of the same intent.
//
// **Win rate never determines a tier on its own.** A 90% win rate against a 0.6 profit factor
// is a trader taking tiny wins and enormous losses — the single most common way a track record
// lies. Profit factor and max drawdown are load-bearing; win rate only ever adds a condition.

/// Profit factor is `gross_profit / gross_loss`, carried at `BPS`. 12_000 = 1.2×.
pub const SILVER_MIN_TRADES: u32 = 20;
pub const SILVER_MIN_WIN_RATE_BPS: u64 = 4_500;
pub const SILVER_MIN_PROFIT_FACTOR_BPS: u64 = 12_000;

pub const GOLD_MIN_TRADES: u32 = 50;
pub const GOLD_MIN_PROFIT_FACTOR_BPS: u64 = 15_000;
/// Gold and above require a *shallow* worst drawdown, not merely a profitable one.
pub const GOLD_MAX_DRAWDOWN_BPS: u16 = 400;

pub const PLATINUM_MIN_TRADES: u32 = 100;
pub const PLATINUM_MIN_PROFIT_FACTOR_BPS: u64 = 18_000;
/// Two mandates that were **settled** in profit — a live result, not a statistic derived from
/// the trades inside one still-open mandate.
pub const PLATINUM_MIN_PROFITABLE_MANDATES: u32 = 2;

// --- the marketplace ------------------------------------------------------------------------

/// `TraderListing` — `["listing", trader]`. A trader advertising for capital.
pub const LISTING_SEED: &[u8] = b"listing";
/// `MandateOffer` — `["offer", investor, trader, seq]`. An investor's escrowed proposal.
pub const OFFER_SEED: &[u8] = b"offer";
/// The offer's USDC escrow — `["offer_vault", offer]`.
pub const OFFER_VAULT_SEED: &[u8] = b"offer_vault";

/// How long a note on a listing or an offer may be, in bytes.
///
/// Bounded, and deliberately short. A note is the one place the two sides put words on chain, and
/// 180 bytes is enough for "EUR/JPY session trader, London hours, happy to start smaller" and far
/// too little to be a chat log. Rent on a loader-v3 account is `(128 + len) × 6,960` lamports, so
/// every byte offered here is a byte someone pays for permanently.
pub const MAX_NOTE_LEN: usize = 180;
