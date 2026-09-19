//! NOXFUNDS — a decentralized prop firm whose rules are checked before the fill.
//!
//! # The one sentence this program exists for
//!
//! Every other prop firm watches trades after they execute and punishes violations, because
//! their contracts cannot see an order before the venue fills it. NOXFUNDS owns the venue. So
//! a trade that breaks the rules is **not detected and punished — it fails as a transaction.
//! It never existed, and the investor never took the loss.**
//!
//! # Shape
//!
//! An investor funds a `Mandate`: capital, plus a rule set fixed at funding and never mutable.
//! A trader signs NOXFUNDS; a dataless PDA signs SolFX. The trader can open and close
//! positions and can never withdraw, because they are not the authority.
//!
//! # What is built
//!
//! Stages 1, 2 and 4–6 of `docs/NOXFUNDS-PLAN.md` Part 10: the mandate primitive and the CPI
//! wrappers, the full rulebook, tiers and the track record, investor settlement, and the
//! permissionless cranks. **Stage 3, the evaluation engine (`Evaluation`, `VirtualPosition`),
//! is not written** — a mandate is funded directly today rather than earned by passing a
//! simulated phase. Stage 7, the marketplace, is frontend work and lives in `app/`.
//!
//! # Two structural facts, both found by measurement rather than assumed
//!
//! - **The `Mandate` account cannot be the SolFX authority.** Both CPI targets are
//!   `init, payer = authority`, which Anchor services through the System Program, which
//!   refuses a `from` that carries data. The authority is a separate dataless PDA.
//! - **Each CPI is issued from its own `#[inline(never)]` function**, so the two `CpiContext`
//!   values — 840 and 552 bytes, held by value — cannot share a stack frame against the
//!   4,096-byte limit.
//!
//! `solfx-core` is untouched. The dependency points one way.
use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod settlement;
pub mod state;

use instructions::*;

declare_id!("9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx");

/// The callee, as a typed `Program<'info, _>` rather than a bare `AccountInfo`.
///
/// A CPI target passed as an unchecked account is an arbitrary-CPI hole: whatever address the
/// caller supplies gets invoked with this program's PDA signature attached. `NoxConfig` pins
/// the address as well, so both the type and the instance are checked.
#[derive(Clone)]
pub struct SolfxCore;

impl anchor_lang::Id for SolfxCore {
    fn id() -> Pubkey {
        solfx_core::ID
    }
}

#[program]
pub mod noxfunds {
    use super::*;

    // --- admin ---------------------------------------------------------------------------

    pub fn initialize_config(
        ctx: Context<InitializeConfig>,
        guardian: Pubkey,
        treasury: Pubkey,
        usdc_mint: Pubkey,
    ) -> Result<()> {
        instructions::admin::initialize_config(ctx, guardian, treasury, usdc_mint)
    }

    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::admin::set_paused(ctx, paused)
    }

    // --- the track record ------------------------------------------------------------------

    /// Open a trader's record. Anyone may pay for it; the profile confers nothing until trades
    /// land on it, and a mandate cannot be funded for a trader without one.
    pub fn initialize_trader_profile(ctx: Context<InitializeTraderProfile>) -> Result<()> {
        instructions::profile::initialize_trader_profile(ctx)
    }

    /// Set a trader's tier to whatever their record currently earns — **in both directions**.
    /// Permissionless, because an investor should not have to ask a trader to refresh a
    /// listing that flatters them.
    pub fn recompute_tier(ctx: Context<RecomputeTier>) -> Result<()> {
        instructions::profile::recompute_tier(ctx)
    }

    // --- investor ------------------------------------------------------------------------

    /// Fix the rules and create the mandate. The trader does not sign — an investor funds a
    /// trader without needing their cooperation.
    pub fn fund_mandate(
        ctx: Context<FundMandate>,
        seq: u8,
        principal: u64,
        rules: MandateRules,
    ) -> Result<()> {
        instructions::investor::fund_mandate(ctx, seq, principal, rules)
    }

    /// Create the SolFX `UserAccount` this mandate trades through, authorised by its signer.
    pub fn create_solfx_account(ctx: Context<CreateSolfxAccount>) -> Result<()> {
        instructions::investor::create_solfx_account(ctx)
    }

    /// Move the mandate's USDC into SolFX's collateral vault.
    pub fn fund_solfx_collateral(ctx: Context<FundSolfxCollateral>, amount: u64) -> Result<()> {
        instructions::investor::fund_solfx_collateral(ctx, amount)
    }

    // --- funded trading ----------------------------------------------------------------

    /// Open a SolFX position with its stop-loss attached **atomically**.
    ///
    /// Every rule that can be judged from this trade and the current state is checked before
    /// either CPI runs. `stop_loss_price` is mandatory and not an argument the trader may omit.
    #[allow(clippy::too_many_arguments)]
    pub fn funded_open_position(
        ctx: Context<FundedOpenPosition>,
        market_index: u16,
        nonce: u8,
        direction: solfx_core::state::Direction,
        size_base: u64,
        collateral: u64,
        price_limit: i64,
        order_id: u8,
        stop_loss_price: i64,
    ) -> Result<()> {
        instructions::trading::funded_open_position(
            ctx,
            market_index,
            nonce,
            direction,
            size_base,
            collateral,
            price_limit,
            order_id,
            stop_loss_price,
        )
    }

    pub fn funded_close_position(
        ctx: Context<FundedClosePosition>,
        market_index: u16,
        nonce: u8,
        price_limit: i64,
    ) -> Result<()> {
        instructions::trading::funded_close_position(ctx, market_index, nonce, price_limit)
    }

    // --- settlement ------------------------------------------------------------------------

    /// The investor ends the mandate. New trades stop; open positions may still be closed.
    pub fn request_settlement(ctx: Context<RequestSettlement>) -> Result<()> {
        instructions::settlement::request_settlement(ctx)
    }

    /// Withdraw the mandate's collateral, take the 5% fee off gross profit, split the rest
    /// 70/30, and return principal. **Permissionless** — the investor never waits on anyone.
    pub fn claim_settlement(ctx: Context<ClaimSettlement>) -> Result<()> {
        instructions::settlement::claim_settlement(ctx)
    }

    // --- keeper ----------------------------------------------------------------------------

    /// Observe a mandate's equity, update its high-water mark, and breach it if it has fallen
    /// too far. **Permissionless** — investor capital is never hostage to an absent trader or
    /// an absent operator.
    ///
    /// `remaining_accounts` carries (position, market, price_update) triples, one per open
    /// position.
    pub fn observe_mandate_equity(ctx: Context<ObserveMandateEquity>) -> Result<()> {
        instructions::keeper::observe_mandate_equity(ctx)
    }

    /// Close one position of a breached or winding-down mandate. **Permissionless**, with the
    /// slippage bound derived from the position rather than chosen by the caller.
    pub fn wind_down_position(ctx: Context<WindDownPosition>) -> Result<()> {
        instructions::keeper::wind_down_position(ctx)
    }

    /// Release a slot whose position closed without NOXFUNDS seeing it — a stop-out.
    /// **Permissionless**, and only if the derived position account is genuinely gone.
    pub fn reconcile_position(
        ctx: Context<ReconcilePosition>,
        market_index: u16,
        nonce: u8,
    ) -> Result<()> {
        instructions::keeper::reconcile_position(ctx, market_index, nonce)
    }

    /// Cancel a resting stop on a stopped mandate. **Permissionless.**
    pub fn wind_down_cancel_stop(ctx: Context<WindDownCancelStop>) -> Result<()> {
        instructions::keeper::wind_down_cancel_stop(ctx)
    }

    /// Reclaim a resting stop's rent after a voluntary close. Without this a mandate bleeds
    /// ~0.002 SOL per closed trade with no recovery path.
    pub fn funded_cancel_stop(
        ctx: Context<FundedCancelStop>,
        market_index: u16,
        nonce: u8,
        order_id: u8,
    ) -> Result<()> {
        instructions::trading::funded_cancel_stop(ctx, market_index, nonce, order_id)
    }

    // --- the marketplace -------------------------------------------------------------------
    //
    // Where the two sides find each other. The binding step is an escrow, not a conversation:
    // an investor publishes terms and locks the capital behind them, and the trader's signature
    // on `accept_offer` creates the mandate and moves the money in the same transaction.

    /// Advertise for capital. Carries no money and takes no custody.
    pub fn post_listing(ctx: Context<PostListing>, terms: ListingTerms) -> Result<()> {
        instructions::marketplace::post_listing(ctx, terms)
    }

    /// Edit an advertisement, or open and close it.
    pub fn update_listing(
        ctx: Context<UpdateListing>,
        terms: ListingTerms,
        open: bool,
    ) -> Result<()> {
        instructions::marketplace::update_listing(ctx, terms, open)
    }

    /// Escrow capital and publish a rule set to one trader.
    pub fn post_offer(
        ctx: Context<PostOffer>,
        seq: u8,
        principal: u64,
        rules: MandateRules,
        trader_split_bps: u16,
        expires_at: i64,
        note: String,
    ) -> Result<()> {
        instructions::marketplace::post_offer(
            ctx,
            seq,
            principal,
            rules,
            trader_split_bps,
            expires_at,
            note,
        )
    }

    /// Take the escrowed capital back. Available while nobody has accepted, expired or not.
    pub fn revoke_offer(ctx: Context<RevokeOffer>) -> Result<()> {
        instructions::marketplace::revoke_offer(ctx)
    }

    /// Accept an offer. Creates the mandate and moves the capital in one transaction.
    pub fn accept_offer(ctx: Context<AcceptOffer>) -> Result<()> {
        instructions::marketplace::accept_offer(ctx)
    }

    /// Say no to an offer, with a reason. Moves no money; the investor revokes to reclaim it.
    pub fn decline_offer(ctx: Context<DeclineOffer>, reason: String) -> Result<()> {
        instructions::marketplace::decline_offer(ctx, reason)
    }

    /// Advertise capital, so traders can find an investor and send a request.
    pub fn post_investor_listing(
        ctx: Context<PostInvestorListing>,
        terms: InvestorListingTerms,
    ) -> Result<()> {
        instructions::marketplace::post_investor_listing(ctx, terms)
    }

    /// Edit an investor's advertisement, or open and close it.
    pub fn update_investor_listing(
        ctx: Context<UpdateInvestorListing>,
        terms: InvestorListingTerms,
        open: bool,
    ) -> Result<()> {
        instructions::marketplace::update_investor_listing(ctx, terms, open)
    }

    /// Ask an investor with an open listing for capital. Carries no money.
    pub fn post_request(
        ctx: Context<PostRequest>,
        wanted_principal: u64,
        wanted_split_bps: u16,
        note: String,
    ) -> Result<()> {
        instructions::marketplace::post_request(ctx, wanted_principal, wanted_split_bps, note)
    }

    /// Withdraw (trader) or dismiss (investor) a request. The rent goes back to the trader.
    pub fn close_request(ctx: Context<CloseRequest>) -> Result<()> {
        instructions::marketplace::close_request(ctx)
    }
    // --- the evaluation (Stage 3) ----------------------------------------------------------
    //
    // A simulated account, priced by SolFX's own functions. No token moves except the stake.

    /// Stake $50 and start Phase 1 on a simulated balance of `account_size` USDC.
    pub fn start_evaluation(
        ctx: Context<StartEvaluation>,
        seq: u8,
        account_size: u64,
    ) -> Result<()> {
        instructions::evaluation::start_evaluation(ctx, seq, account_size)
    }

    /// Open a simulated position, with its stop, at exactly the price a real fill would get.
    pub fn eval_open_position(
        ctx: Context<EvalOpenPosition>,
        market_index: u16,
        nonce: u8,
        direction: solfx_core::state::Direction,
        size_base: u64,
        stop_loss_price: i64,
    ) -> Result<()> {
        instructions::evaluation::eval_open_position(
            ctx,
            market_index,
            nonce,
            direction,
            size_base,
            stop_loss_price,
        )
    }

    /// Close a simulated position by choice. Subject to the ten-minute minimum hold.
    pub fn eval_close_position(ctx: Context<EvalClosePosition>) -> Result<()> {
        instructions::evaluation::eval_close_position(ctx)
    }

    /// Fire a simulated stop the oracle has reached. **Permissionless.**
    pub fn eval_trigger_stop(ctx: Context<EvalTriggerStop>) -> Result<()> {
        instructions::evaluation::eval_trigger_stop(ctx)
    }

    /// Mark every open simulated position and judge the loss limits. **Permissionless.**
    pub fn eval_observe_equity(ctx: Context<EvalObserveEquity>) -> Result<()> {
        instructions::evaluation::eval_observe_equity(ctx)
    }

    /// Verify the current stage and advance; Phase 2 passed refunds the stake.
    pub fn claim_stage_pass(ctx: Context<ClaimStagePass>) -> Result<()> {
        instructions::evaluation::claim_stage_pass(ctx)
    }

    /// Send a failed evaluation's stake to the treasury. **Permissionless.**
    pub fn forfeit_stake(ctx: Context<ForfeitStake>) -> Result<()> {
        instructions::evaluation::forfeit_stake(ctx)
    }

    /// Walk away from an evaluation. The stake is forfeit.
    pub fn abandon_evaluation(ctx: Context<AbandonEvaluation>) -> Result<()> {
        instructions::evaluation::abandon_evaluation(ctx)
    }
}
