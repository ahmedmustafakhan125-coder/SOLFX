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
//! Stage 1 of `docs/NOXFUNDS-PLAN.md` Part 10: the mandate primitive and the CPI wrappers.
//! Evaluation, tiers, investor settlement and the keeper are Stages 3–6.
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
}
