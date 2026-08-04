//! # solfx-core
//!
//! The SolFX financial core. Phase 2 delivers protocol configuration, market listing, the
//! collateral vault, and the validated Pyth read path. Positions arrive in Phase 3, the risk
//! engine in Phase 4.
//!
//! ## What this program does not know
//!
//! It does not know that EUR/USD exists. Markets are rows created at runtime by
//! [`initialize_market`](instructions::admin::initialize_market); the program is the formula
//! (`ARCHITECTURE.md` § 5.6). That is what makes Tier 2–5 expansion a configuration change
//! rather than an upgrade, and it is enforced structurally: all financial arithmetic lives
//! in `solfx-math`, a crate that cannot reference a `Market` or a `Position` because it does
//! not depend on this one.
//!
//! ## Reading order
//!
//! [`oracle`] first — it is the single choke point every price passes through, and the place
//! where the two threats that actually kill a pool-backed venue (T2 latency arbitrage, T3
//! stale-price) are stopped. Then [`state`], then [`instructions`].
//!
//! ## Lints
//!
//! The workspace denies eleven lint groups. Four are relaxed here at crate level:
//! `arithmetic_side_effects`, `cast_possible_truncation`, `cast_sign_loss` and
//! `indexing_slicing`. Anchor's `#[program]`, `#[derive(Accounts)]` and `#[account]` macros
//! generate code that trips all four — discriminator slicing, borsh length arithmetic, space
//! computation — and there is no way to scope an allow to macro-generated code.
//!
//! The relaxation is affordable because the arithmetic that matters is not written here.
//! Every value that touches a trader's money is computed in `solfx-math`, which inherits the
//! full strict set and is exhaustively property-tested. What remains in this crate is
//! account plumbing. The lints that catch *silent value corruption* rather than macro noise
//! — `unwrap_used`, `expect_used`, `panic`, `float_arithmetic`, `unsafe_code`,
//! `integer_division`, `cast_possible_wrap` — stay denied in both crates.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::indexing_slicing
)]
// Anchor's generated `Result` carries a large error enum; this fires on every handler.
#![allow(clippy::result_large_err)]
// Unit tests inside this crate assert against known values and unwrap expected-Ok results.
// A panic in a *program* is a failed transaction with no named error; in a test it is the
// reporting mechanism. Same exemption `solfx-math` takes, for the same reason.
#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::integer_division,
        clippy::panic,
        clippy::unwrap_used
    )
)]

use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod oracle;
pub mod state;

use instructions::*;
use state::MarketStatus;

declare_id!("2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi");

#[program]
pub mod solfx_core {
    use super::*;

    // --- admin (§ 5.4) ---------------------------------------------------------------

    /// One-time setup: config, vaults, insurance fund, LP pool and mint.
    pub fn initialize_protocol(
        ctx: Context<InitializeProtocol>,
        params: InitializeProtocolParams,
    ) -> Result<()> {
        instructions::admin::initialize_protocol::init_protocol(ctx, params)
    }

    /// List a market. No redeploy, no downtime, no migration (§ 5.6).
    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_index: u16,
        params: InitializeMarketParams,
    ) -> Result<()> {
        instructions::admin::initialize_market::init_market(ctx, market_index, params)
    }

    pub fn update_market_risk_params(
        ctx: Context<AdminMarket>,
        params: UpdateRiskParams,
    ) -> Result<()> {
        instructions::admin::update_market::update_risk_params(ctx, params)
    }

    pub fn update_market_fee_params(
        ctx: Context<AdminMarket>,
        params: UpdateFeeParams,
    ) -> Result<()> {
        instructions::admin::update_market::update_fee_params(ctx, params)
    }

    pub fn set_market_status(ctx: Context<AdminMarket>, new_status: MarketStatus) -> Result<()> {
        instructions::admin::update_market::set_market_status(ctx, new_status)
    }

    pub fn update_fee_splits(ctx: Context<AdminOnly>, params: FeeSplitParams) -> Result<()> {
        instructions::admin::protocol_admin::update_fee_splits(ctx, params)
    }

    /// Step one of the two-step authority handover.
    pub fn transfer_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        instructions::admin::protocol_admin::transfer_admin(ctx, new_admin)
    }

    /// Step two: the nominated authority proves it can sign.
    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        instructions::admin::protocol_admin::accept_admin(ctx)
    }

    pub fn set_guardian(ctx: Context<AdminOnly>, new_guardian: Pubkey) -> Result<()> {
        instructions::admin::protocol_admin::set_guardian(ctx, new_guardian)
    }

    /// Resume trading. Admin only — the guardian can restrict but never release.
    pub fn unpause(ctx: Context<AdminUnpause>) -> Result<()> {
        instructions::guardian::unpause(ctx)
    }

    // --- guardian: restrict only, never move funds ------------------------------------

    /// Global kill switch. Withdrawals of free collateral stay live.
    pub fn emergency_pause(ctx: Context<GuardianPause>) -> Result<()> {
        instructions::guardian::emergency_pause(ctx)
    }

    pub fn halt_market(ctx: Context<GuardianHaltMarket>) -> Result<()> {
        instructions::guardian::halt_market(ctx)
    }

    // --- trader -----------------------------------------------------------------------

    /// Create a trader account and bind the referrer permanently (§ 8.5).
    pub fn initialize_user_account(
        ctx: Context<InitializeUserAccount>,
        referrer: Pubkey,
    ) -> Result<()> {
        instructions::user::initialize_user_account(ctx, referrer)
    }

    pub fn deposit_collateral(ctx: Context<MoveCollateral>, amount: u64) -> Result<()> {
        instructions::user::deposit_collateral(ctx, amount)
    }

    /// Withdraw free collateral. Works while paused, by design.
    pub fn withdraw_collateral(ctx: Context<MoveCollateral>, amount: u64) -> Result<()> {
        instructions::user::withdraw_collateral(ctx, amount)
    }

    // --- positions (Phase 3) ----------------------------------------------------------

    /// Open a leveraged position.
    ///
    /// `price_limit` is a maximum when buying and a minimum when selling — checked against
    /// the *filled* price, which already carries the adverse spread.
    #[allow(clippy::too_many_arguments)]
    pub fn open_position(
        ctx: Context<OpenPosition>,
        market_index: u16,
        nonce: u8,
        direction: state::Direction,
        size_base: u64,
        collateral: u64,
        price_limit: i64,
    ) -> Result<()> {
        instructions::trader::open_position::open_position(
            ctx,
            market_index,
            nonce,
            direction,
            size_base,
            collateral,
            price_limit,
        )
    }

    /// Add size, and optionally margin, to an existing position.
    pub fn increase_position(
        ctx: Context<DecreasePosition>,
        size_delta: u64,
        collateral_delta: u64,
        price_limit: i64,
    ) -> Result<()> {
        instructions::trader::increase_position::increase_position(
            ctx,
            size_delta,
            collateral_delta,
            price_limit,
        )
    }

    /// Partially close, realising a proportional share of PnL.
    pub fn decrease_position(
        ctx: Context<DecreasePosition>,
        size_delta: u64,
        price_limit: i64,
    ) -> Result<()> {
        instructions::trader::close_position::decrease_position(ctx, size_delta, price_limit)
    }

    /// Close the whole position and reclaim its rent.
    pub fn close_position(ctx: Context<ClosePosition>, price_limit: i64) -> Result<()> {
        instructions::trader::close_position::close_position(ctx, price_limit)
    }

    /// Move free collateral into a position's isolated margin.
    pub fn add_position_collateral(ctx: Context<DecreasePosition>, amount: u64) -> Result<()> {
        instructions::trader::adjust_collateral::add_position_collateral(ctx, amount)
    }

    /// Move margin out of a position, subject to the initial-margin requirement.
    pub fn remove_position_collateral(ctx: Context<DecreasePosition>, amount: u64) -> Result<()> {
        instructions::trader::adjust_collateral::remove_position_collateral(ctx, amount)
    }

    // --- liquidity --------------------------------------------------------------------

    /// Deposit USDC into the counterparty pool and receive `slpUSD`.
    ///
    /// Deposit only. Withdrawal ships in Phase 5 with the cooldown and exit fee that defend
    /// against the JIT attack — see `instructions::lp`.
    pub fn add_liquidity(ctx: Context<AddLiquidity>, amount: u64, min_lp_out: u64) -> Result<()> {
        instructions::lp::add_liquidity(ctx, amount, min_lp_out)
    }

    // --- keeper: permissionless -------------------------------------------------------

    /// Read the oracle, record the price, trip the deviation breaker if it fires.
    pub fn crank_market_price(ctx: Context<CrankMarketPrice>) -> Result<()> {
        instructions::keeper::crank_market_price(ctx)
    }
}
