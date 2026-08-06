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
/// `["lp_withdraw", authority]`
pub const LP_WITHDRAW_SEED: &[u8] = b"lp_withdraw";

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

// --- session regimes (§ 7.3) --------------------------------------------------------------

/// How long before a session close a market stops accepting new positions.
///
/// § 7.3 specifies T−15min. The point is not the exact figure but that new risk stops being
/// taken while the feed is still live enough to manage it — a position opened in the final
/// seconds cannot be closed until the market reopens, and by then it has absorbed the whole
/// weekend.
pub const PRE_CLOSE_REDUCE_ONLY_SECONDS: i64 = 15 * 60;

/// How long a market stays in `GapWindow` after reopening.
///
/// § 7.3 suggests ~5 minutes. Closes and liquidations are permitted; new positions are not.
/// The first minutes after a session gap carry the whole weekend's news in one jump, and
/// letting someone open into that is letting them trade a price nobody had a chance to react
/// to.
pub const GAP_WINDOW_SECONDS: i64 = 5 * 60;

/// How long before a continuous market's underlying spot reopens that it stops accepting
/// new positions (§ 7.3, `PreOpenWindow`).
///
/// Dormant: no market can currently occupy the continuous regime. Kept so that resolving Q1b
/// is a configuration change rather than a code change.
pub const PRE_OPEN_WINDOW_SECONDS: i64 = 30 * 60;

/// The floor on a liquidator's reward, in USDC at `QUOTE_PRECISION`. $1.00.
///
/// # Why a floor exists at all
///
/// § 6.8's reward is a share of the penalty, and the penalty comes out of the position's
/// remaining equity. In a severe gap there **is no remaining equity** — the CHF-depeg replay
/// wipes it entirely — so the share is zero, and a liquidator who is paid nothing does not
/// show up. The position then stays open and keeps falling, which is how a bounded loss
/// becomes unbounded bad debt.
///
/// That is the exact failure § 6.8 warns about (*"an unprofitable liquidation is an
/// unliquidated position, and unliquidated positions are how vaults die"*), reached from the
/// opposite direction: not because the reward was set too low, but because there was nothing
/// left to take it from.
///
/// So when the position cannot pay, the **insurance fund** does. That is what it is for, and
/// the arithmetic is not close: a dollar to close a position now against a shortfall that
/// grows every minute it stays open.
///
/// § 6.8 budgets a liquidation transaction at ~$0.002 and suggests being willing to pay
/// $0.50 in priority fees during congestion. A dollar clears both with room.
pub const MIN_LIQUIDATOR_REWARD: u64 = 1_000_000;

/// The imbalance floor below which § 7.4's skew ratio is not applied, in bps of pool AUM.
///
/// # Why the ratio alone deadlocks a market
///
/// § 7.4 blocks opens on the heavy side once
/// `|oi_long − oi_short| / (oi_long + oi_short) > 0.6`. Taken literally that makes a market
/// **untradeable from empty**: the first position is 100% one-sided by definition, so it is
/// refused, so a book can never form. Same shape as the deviation-gate deadlock Phase 2
/// found — a control that is correct in steady state and impossible at the boundary.
///
/// The ratio is also the wrong question on a small book. What threatens the vault is the
/// *absolute* directional exposure it is carrying against its capital; a 100% skew on $100 of
/// open interest is noise, a 60% skew on $10M is not.
///
/// So both must be exceeded: the ratio from § 7.4, and an imbalance worth at least 10% of
/// AUM. That keeps the specified control and makes it well-defined at zero.
pub const SKEW_CAP_MIN_IMBALANCE_BPS_OF_AUM: u128 = 1_000;
