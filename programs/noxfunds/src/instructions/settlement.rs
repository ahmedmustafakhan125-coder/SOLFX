//! Ending a mandate and paying everyone.
//!
//! # Two steps, on purpose
//!
//! `request_settlement` stops new trades. `claim_settlement` moves money, and only once every
//! position is flat. Collapsing them would force the investor to wait on the trader closing
//! out, which is exactly the dependence R2 exists to remove: **the investor recovers principal
//! from a wound-down mandate without anyone else's cooperation.** So `claim_settlement` takes no
//! privileged signer at all.
//!
//! # What this does not yet do
//!
//! A mandate with positions still open cannot settle — someone has to close them first. The
//! trader can today; a permissionless wind-down that closes them for an absent trader is Stage 6.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use solfx_core::state::UserAccount;

use crate::constants::{
    CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED, MANDATE_VAULT_SEED, TRADER_SEED,
};
use crate::errors::NoxError;
use crate::events::{MandateSettled, SettlementRequested};
use crate::settlement::split;
use crate::state::{Mandate, MandateState, NoxConfig, TraderProfile};
use crate::SolfxCore;

#[derive(Accounts)]
pub struct RequestSettlement<'info> {
    pub investor: Signer<'info>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
        constraint = mandate.investor == investor.key() @ NoxError::NotTheInvestor,
    )]
    pub mandate: Box<Account<'info, Mandate>>,
}

/// The investor ends the mandate. `Active` → `WindingDown`; nothing else.
///
/// Not permitted from `Breached`: a breached mandate is already stopped, and moving it to
/// `WindingDown` would overwrite the record of *why* it ended — the one thing an investor
/// choosing their next trader needs to read.
pub fn request_settlement(ctx: Context<RequestSettlement>) -> Result<()> {
    let m = &mut ctx.accounts.mandate;
    require!(m.state == MandateState::Active, NoxError::MandateNotActive);
    m.state = MandateState::WindingDown;
    emit!(SettlementRequested {
        mandate: m.key(),
        investor: m.investor,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ClaimSettlement<'info> {
    /// **Anyone.** Pays the transaction fee and nothing else.
    pub settler: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    // --- the SolFX side of the withdrawal -------------------------------------------------
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub protocol: UncheckedAccount<'info>,
    /// Deserialized: settlement withdraws exactly the free collateral recorded here.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: Box<Account<'info, UserAccount>>,
    /// `mut` because `solfx-core`'s `MoveCollateral` marks it so — a CPI cannot ask for a
    /// privilege the outer instruction did not grant.
    #[account(mut, address = config.usdc_mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub collateral_vault: UncheckedAccount<'info>,

    // --- where the money goes -----------------------------------------------------------
    /// The mandate's own USDC account. Withdrawals land here first, then leave in three parts.
    ///
    /// Bound to `["vault", mandate]`. It used to be *any* token account the signer owned, and
    /// the settler — who is anyone — chose which: they could pass a fresh empty account, so the
    /// split was computed over a balance that excluded any principal still sitting in the real
    /// vault.
    #[account(
        mut,
        seeds = [MANDATE_VAULT_SEED, mandate.key().as_ref()],
        bump = mandate.vault_bump,
        token::mint = config.usdc_mint,
        token::authority = mandate_signer,
    )]
    pub mandate_vault: Box<Account<'info, TokenAccount>>,
    /// Constrained to the investor recorded at funding, not to whoever calls — a permissionless
    /// instruction must not let the caller choose where the principal goes.
    #[account(
        mut,
        token::mint = config.usdc_mint,
        token::authority = mandate.investor,
    )]
    pub investor_token: Box<Account<'info, TokenAccount>>,
    #[account(
        mut,
        token::mint = config.usdc_mint,
        token::authority = mandate.trader,
    )]
    pub trader_token: Box<Account<'info, TokenAccount>>,
    #[account(
        mut,
        token::mint = config.usdc_mint,
        token::authority = config.treasury,
    )]
    pub treasury_token: Box<Account<'info, TokenAccount>>,

    /// The trader's record. Settlement is where a mandate's outcome — not just its individual
    /// trades — lands on it: the active count comes down, and a mandate that finished above
    /// principal counts toward Platinum.
    #[account(
        mut,
        seeds = [TRADER_SEED, mandate.trader.as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == mandate.trader @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    pub token_program: Program<'info, Token>,
    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, SolfxCore>,
}

/// Withdraw everything from SolFX, divide it, pay it out, and close the mandate.
pub fn claim_settlement(ctx: Context<ClaimSettlement>) -> Result<()> {
    let state = ctx.accounts.mandate.state;
    require!(
        matches!(state, MandateState::WindingDown | MandateState::Breached),
        NoxError::MandateNotSettleable
    );
    // Free collateral is only the whole of the mandate's equity once nothing is open. Settling
    // earlier would pay out a figure that leaves position margin, and its PnL, behind.
    require!(
        ctx.accounts.mandate.open_positions == 0,
        NoxError::PositionsStillOpen
    );

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    let free = ctx.accounts.user_account.free_collateral;
    if free > 0 {
        cpi_withdraw(&ctx, seeds, free)?;
    }

    // Read the vault *after* the withdrawal. Anchor does not refresh a deserialized account
    // across a CPI, and the balance is the only honest measure of what there is to divide —
    // a stale read here would pay out the pre-withdrawal figure.
    ctx.accounts.mandate_vault.reload()?;
    let final_equity = ctx.accounts.mandate_vault.amount;

    let m = &ctx.accounts.mandate;
    let s = split(
        m.principal,
        final_equity,
        ctx.accounts.config.protocol_fee_bps,
        m.trader_split_bps,
    )
    .map_err(NoxError::from)?;

    let decimals = ctx.accounts.usdc_mint.decimals;
    pay(&ctx, seeds, PayTo::Investor, s.investor, decimals)?;
    pay(&ctx, seeds, PayTo::Trader, s.trader, decimals)?;
    pay(&ctx, seeds, PayTo::Treasury, s.protocol, decimals)?;

    let was_breached = state == MandateState::Breached;

    // The mandate is over either way, so the slot frees either way — a breach must not leave a
    // trader permanently one mandate below their tier's limit. Only the *profit* count is
    // conditional, and it is measured against principal rather than against the trader's payout,
    // because a mandate that made money for its investor is the claim the tier is about.
    let profile = &mut ctx.accounts.trader_profile;
    profile.active_mandates = profile.active_mandates.saturating_sub(1);
    if final_equity > ctx.accounts.mandate.principal {
        profile.mandates_settled_in_profit = profile.mandates_settled_in_profit.saturating_add(1);
    }

    let m = &mut ctx.accounts.mandate;
    m.state = MandateState::Settled;
    m.last_equity = final_equity;

    emit!(MandateSettled {
        mandate: mandate_key,
        investor: m.investor,
        trader: m.trader,
        principal: m.principal,
        final_equity,
        gross_profit: s.gross_profit,
        protocol_fee: s.protocol,
        trader_share: s.trader,
        investor_share: s.investor,
        was_breached,
        settled_by: ctx.accounts.settler.key(),
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[inline(never)]
fn cpi_withdraw(ctx: &Context<ClaimSettlement>, seeds: &[&[&[u8]]], amount: u64) -> Result<()> {
    solfx_core::cpi::withdraw_collateral(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::MoveCollateral {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                collateral_mint: ctx.accounts.usdc_mint.to_account_info(),
                collateral_vault: ctx.accounts.collateral_vault.to_account_info(),
                user_token_account: ctx.accounts.mandate_vault.to_account_info(),
                token_program: ctx.accounts.token_program.to_account_info(),
            },
            seeds,
        ),
        amount,
    )
}

#[derive(Clone, Copy)]
enum PayTo {
    Investor,
    Trader,
    Treasury,
}

/// `transfer_checked` rather than `transfer`, as the Solana MCP recommends for anything
/// user-facing: it carries the mint and verifies decimals, so a wrong-mint account fails in
/// the Token Program as well as at the constraint above.
#[inline(never)]
fn pay(
    ctx: &Context<ClaimSettlement>,
    seeds: &[&[&[u8]]],
    to: PayTo,
    amount: u64,
    decimals: u8,
) -> Result<()> {
    // A zero transfer is legal but pointless, and on a losing mandate two of the three are
    // zero. Skipping them is cheaper and keeps the transaction's log honest about what moved.
    if amount == 0 {
        return Ok(());
    }
    let destination = match to {
        PayTo::Investor => ctx.accounts.investor_token.to_account_info(),
        PayTo::Trader => ctx.accounts.trader_token.to_account_info(),
        PayTo::Treasury => ctx.accounts.treasury_token.to_account_info(),
    };
    token::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.mandate_vault.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: destination,
                authority: ctx.accounts.mandate_signer.to_account_info(),
            },
            seeds,
        ),
        amount,
        decimals,
    )
}
