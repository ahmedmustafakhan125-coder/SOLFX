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
