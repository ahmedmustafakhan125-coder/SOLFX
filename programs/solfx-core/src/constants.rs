//! PDA seeds and protocol-wide limits.
//!
//! Seeds are `&[u8]` constants rather than string literals at the call site so a typo is a
//! compile error rather than a silently different address.

/// `["protocol"]` — the singleton config account.
pub const PROTOCOL_SEED: &[u8] = b"protocol";
/// `["market", market_index: u16]`
pub const MARKET_SEED: &[u8] = b"market";
/// `["user", authority]`
pub const USER_SEED: &[u8] = b"user";
/// `["position", user_account, market_index: u16, nonce: u8]`
///
/// The nonce lets one trader hold several positions in the same market — isolated margin
/// (ADR-004) means each is a separate risk, so they must be separate accounts.
pub const POSITION_SEED: &[u8] = b"position";
/// `["collateral_vault"]` — holds every user's free collateral and (from Phase 3) every
/// position's isolated margin.
pub const COLLATERAL_VAULT_SEED: &[u8] = b"collateral_vault";
/// `["fee_vault"]` — protocol treasury.
pub const FEE_VAULT_SEED: &[u8] = b"fee_vault";
/// `["insurance_fund"]`
pub const INSURANCE_FUND_SEED: &[u8] = b"insurance_fund";
/// `["insurance_vault"]`
pub const INSURANCE_VAULT_SEED: &[u8] = b"insurance_vault";
/// `["lp_pool"]`
pub const LP_POOL_SEED: &[u8] = b"lp_pool";
/// `["lp_vault"]`
pub const LP_VAULT_SEED: &[u8] = b"lp_vault";
/// `["lp_mint"]` — the `slpUSD` mint.
pub const LP_MINT_SEED: &[u8] = b"lp_mint";

/// USDC has six decimals, matching `solfx_math::constants::QUOTE_PRECISION`.
///
/// Enforced at `initialize_protocol`. If the collateral mint had a different scale every
/// quote-denominated figure in the engine would be off by a power of ten, and nothing
/// downstream would notice.
pub const USDC_DECIMALS: u8 = 6;

/// `slpUSD` shares the collateral scale so the LP NAV per share reads as a dollar figure.
pub const LP_MINT_DECIMALS: u8 = 6;

/// Ceiling on `Market.max_staleness_seconds`.
///
/// `oracle-feasibility.md` measured every listable feed publishing at a 1 s cadence, so a
/// window beyond a minute is not tolerance for a slow feed — it is tolerance for a *closed*
/// one, which is the C-1 exploit.
pub const MAX_ALLOWED_STALENESS_SECONDS: u32 = 60;

/// Tolerance for disagreement between a Pyth publisher's clock and the cluster's.
///
/// Guards the upper end of the freshness window (`solfx_math::oracle::validate_publish_time`).
/// Small but non-zero: zero would reject on ordinary clock jitter.
pub const MAX_FUTURE_DRIFT_SECONDS: u32 = 5;

/// Ceiling on `Market.max_conf_bps`.
///
/// The widest feed that survived Phase 0b is XPD/USD at 28.66 bps p95, and USD/IDR was
/// excluded at 30.11. A market configured above this would be quoting a band wider than
/// anything we measured as tradeable.
pub const MAX_ALLOWED_CONF_BPS: u16 = 3_000;

/// Ceiling on `Market.max_leverage`. § 6.3 launches at 50x; the hard cap sits above it so
/// the launch parameter can be tuned without an upgrade, but 500x can never be set by
/// accident.
pub const MAX_ALLOWED_LEVERAGE: u16 = 100;

/// Longest permitted market symbol, in bytes. Fits `USDCLP`, `XAUUSD`, `EURUSD`.
pub const MAX_SYMBOL_LEN: usize = 16;
