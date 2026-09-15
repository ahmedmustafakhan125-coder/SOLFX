//! NOXFUNDS — Stage 0 feasibility spike.
//!
//! # What this is, and what it is not
//!
//! This is the spike `docs/NOXFUNDS-PLAN.md` Part 10 Stage 0 asks for: **one throwaway
//! instruction that proves a PDA-authority CPI into `open_position` fits**, measured rather
//! than argued. It carries no rules, no state and no economics. Stage 1 replaces it.
//!
//! It exists because the plan called R1 — the BPF stack frame at ~21 accounts — "the one that
//! could force a redesign". The derivation in `docs/program-upgrade-plan.md` narrowed that to
//! a single unknown: whether LLVM reuses one stack slot between the two `CpiContext` values.
//! Everything else was settled on paper. This instruction settles the last part on hardware.
//!
//! # The correction that shaped it
//!
//! Both CPI targets are `init, payer = authority`. Anchor services that through the System
//! Program, which refuses a `from` that carries data — so **the authority cannot be a
//! data-bearing `#[account]` PDA**. It is `mandate_signer`: a dataless, system-owned
//! `SystemAccount` that signs both CPIs and pays both rents. That is why there are two PDAs
//! here where the plan had one.
//!
//! # Deliberately not mitigated yet
//!
//! Each CPI is issued from its own `#[inline(never)]` function, so the two `CpiContext`s
//! (840 and 552 bytes by value) cannot share a frame. This is the pre-committed remedy from
//! the plan, applied up front rather than after a failure — it costs two function boundaries
//! and removes the only unknown. If a later measurement shows the frame has room to spare,
//! the attribute can come off; the reverse is a debugging session.
use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_core::state::{Direction, TriggerKind};

declare_id!("9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx");

/// Seed for the dataless PDA that is the SolFX authority.
///
/// Derived from the mandate rather than the trader: one trader may hold several mandates from
/// different investors, and each must custody its capital separately.
pub const MANDATE_SIGNER_SEED: &[u8] = b"signer";

#[program]
pub mod noxfunds {
    use super::*;

    /// Open a SolFX position and attach its stop-loss, both signed by a dataless PDA.
    ///
    /// Stage 0 measures three things this instruction is otherwise indifferent to: the account
    /// count (22), the serialized transaction size, and the peak stack frame. It performs no
    /// rule checks — that is Stage 1 — so nothing here should be read as a policy decision.
    // Nine arguments because it is two instructions' worth: everything `open_position` takes,
    // plus the order id and stop price `place_trigger_order` needs. Stage 1 collapses these
    // into a params struct; the spike keeps them flat so the instruction data — and therefore
    // the measured packet size — matches what a real client would send.
    #[allow(clippy::too_many_arguments)]
    pub fn spike_open_with_stop(
        ctx: Context<SpikeOpenWithStop>,
        market_index: u16,
        nonce: u8,
        direction: Direction,
        size_base: u64,
        collateral: u64,
        price_limit: i64,
        order_id: u8,
        stop_loss_price: i64,
    ) -> Result<()> {
        let mandate = ctx.accounts.mandate.key();
        let bump = ctx.bumps.mandate_signer;
        let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate.as_ref(), &[bump]]];

        cpi_open_position(
            &ctx,
            seeds,
            market_index,
            nonce,
            direction,
            size_base,
            collateral,
            price_limit,
        )?;

        // The stop is placed in the **same instruction** as the open. That is the whole point:
        // SolFX has no `Position -> TriggerOrder` link, so "every funded trade carries a stop"
        // is only true if both land atomically. A second transaction could fail.
        cpi_place_stop(&ctx, seeds, order_id, stop_loss_price, size_base)?;

        Ok(())
    }
}

/// `#[inline(never)]` so this `CpiContext` (16 × 48 + 72 = 840 bytes, held by value) lives in
/// its own frame and cannot coexist with the one below.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn cpi_open_position(
    ctx: &Context<SpikeOpenWithStop>,
    seeds: &[&[&[u8]]],
    market_index: u16,
    nonce: u8,
    direction: Direction,
    size_base: u64,
    collateral: u64,
    price_limit: i64,
) -> Result<()> {
    solfx_core::cpi::open_position(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::OpenPosition {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                market: ctx.accounts.market.to_account_info(),
                position: ctx.accounts.position.to_account_info(),
                collateral_vault: ctx.accounts.collateral_vault.to_account_info(),
                lp_pool: ctx.accounts.lp_pool.to_account_info(),
                lp_vault: ctx.accounts.lp_vault.to_account_info(),
                insurance_fund: ctx.accounts.insurance_fund.to_account_info(),
                insurance_vault: ctx.accounts.insurance_vault.to_account_info(),
                fee_vault: ctx.accounts.fee_vault.to_account_info(),
                price_update: ctx.accounts.price_update.to_account_info(),
                // An absent optional leg is `Some(program id)`, **never** `None`.
                // `ToAccountMetas for Option<T>` emits `crate::ID` as the meta while
                // `ToAccountInfos` returns nothing for `None` — so `None` produces a meta with
                // no backing `AccountInfo` in the `invoke_signed` slice. `solfx-core` resolves
                // either encoding to `None` on its side.
                secondary_price_update: Some(optional_leg(
                    &ctx.accounts.secondary_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                quote_conversion_price_update: Some(optional_leg(
                    &ctx.accounts.quote_conversion_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                token_program: ctx.accounts.token_program.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
            },
            seeds,
        ),
        market_index,
        nonce,
        direction,
        size_base,
        collateral,
        price_limit,
    )
}

/// `#[inline(never)]` for the same reason as above — this context is 10 × 48 + 72 = 552 bytes.
#[inline(never)]
fn cpi_place_stop(
    ctx: &Context<SpikeOpenWithStop>,
    seeds: &[&[&[u8]]],
    order_id: u8,
    stop_loss_price: i64,
    size_base: u64,
) -> Result<()> {
    solfx_core::cpi::place_trigger_order(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::PlaceTriggerOrder {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                market: ctx.accounts.market.to_account_info(),
                // Created by the CPI above, in this same instruction, and read here.
                position: ctx.accounts.position.to_account_info(),
                trigger_order: ctx.accounts.trigger_order.to_account_info(),
                price_update: ctx.accounts.price_update.to_account_info(),
                secondary_price_update: Some(optional_leg(
                    &ctx.accounts.secondary_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                quote_conversion_price_update: Some(optional_leg(
                    &ctx.accounts.quote_conversion_price_update,
                    &ctx.accounts.solfx_core_program,
                )),
                system_program: ctx.accounts.system_program.to_account_info(),
            },
            seeds,
        ),
        order_id,
        TriggerKind::StopLoss,
        stop_loss_price,
        size_base,
    )
}

/// Resolve an optional oracle leg to the `AccountInfo` the CPI should carry.
///
/// Present: the leg itself. Absent: the callee's own program id, which is the sentinel Anchor
/// uses for a missing optional account and which `solfx-core` decodes back to `None`.
fn optional_leg<'info>(
    leg: &Option<Box<Account<'info, PriceUpdateV2>>>,
    solfx_core_program: &Program<'info, SolfxCore>,
) -> AccountInfo<'info> {
    match leg {
        Some(a) => a.to_account_info(),
        None => solfx_core_program.to_account_info(),
    }
}

/// The callee, as a typed `Program<'info, _>` rather than a bare `AccountInfo`.
///
/// A CPI target passed as an unchecked account is an arbitrary-CPI hole: whatever address the
/// caller supplies gets invoked with this program's PDA signature attached.
#[derive(Clone)]
pub struct SolfxCore;

impl anchor_lang::Id for SolfxCore {
    fn id() -> Pubkey {
        solfx_core::ID
    }
}

/// Twenty-two accounts. The count is asserted in `tests/spike.rs`, because it is the number
/// Stage 0 exists to pin.
#[derive(Accounts)]
pub struct SpikeOpenWithStop<'info> {
    /// The trader. Pays the transaction fee; pays no rent.
    #[account(mut)]
    pub trader: Signer<'info>,

    /// CHECK: Stage 0 placeholder for `NoxConfig`. Stage 1 gives it a type and seeds; it is
    /// carried here only so the account count and packet size are measured at their real size.
    pub nox_config: UncheckedAccount<'info>,

    /// CHECK: Stage 0 placeholder for `Mandate`. Its key is a seed of `mandate_signer` below,
    /// which is the only thing the spike needs from it.
    pub mandate: UncheckedAccount<'info>,

    /// CHECK: Stage 0 placeholder for `TraderProfile`.
    #[account(mut)]
    pub trader_profile: UncheckedAccount<'info>,

    /// The SolFX authority: dataless, system-owned, signs both CPIs and pays both rents.
    ///
    /// `SystemAccount` rather than an `#[account]` struct because `payer = authority` is
    /// serviced by the System Program, which refuses a `from` that carries data.
    #[account(mut, seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()], bump)]
    pub mandate_signer: SystemAccount<'info>,

    // --- passed through to solfx-core, which re-validates every one by seeds and constraints
    /// CHECK: validated by `solfx-core`'s own `seeds` + `bump` on the callee side.
    #[account(mut)]
    pub protocol: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub user_account: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub market: UncheckedAccount<'info>,
    /// CHECK: created by CPI #1 and read by CPI #2; validated by `solfx-core` both times.
    #[account(mut)]
    pub position: UncheckedAccount<'info>,
    /// CHECK: created by CPI #2; validated by `solfx-core`.
    #[account(mut)]
    pub trigger_order: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub collateral_vault: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub lp_pool: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub lp_vault: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub insurance_fund: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub insurance_vault: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub fee_vault: UncheckedAccount<'info>,

    /// Deserialized, not passed through: Stage 1 prices the trade here before the CPI, and
    /// `load_validated_price` takes `&PriceUpdateV2`. Costs 8 bytes of stack, same as an
    /// `UncheckedAccount`.
    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub solfx_core_program: Program<'info, SolfxCore>,
}
