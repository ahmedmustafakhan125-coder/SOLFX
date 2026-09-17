//! Funding a mandate: the moment an investor fixes the rules and hands over capital.

use anchor_lang::prelude::*;

use crate::constants::{
    CONFIG_SEED, DEFAULT_TRADER_SPLIT_BPS, MANDATE_SEED, MANDATE_SIGNER_SEED, TRADER_SEED,
};
use crate::errors::NoxError;
use crate::events::MandateFunded;
use crate::state::{Mandate, MandateState, NoxConfig, TraderProfile};

/// The rules an investor sets. Passed as one struct because nine loose arguments at a call
/// site is how a `max_drawdown_bps` ends up in the `max_daily_loss_bps` slot.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct MandateRules {
    pub max_trade_notional: u64,
    pub max_total_notional: u64,
    pub max_drawdown_bps: u16,
    pub max_daily_loss_bps: u16,
    pub max_risk_per_trade_bps: u16,
    pub max_stop_distance_bps: u16,
    pub max_concurrent_positions: u8,
    pub allowed_markets: u128,
    pub min_hold_slots: u64,
}

impl MandateRules {
    /// Reject a rule set that cannot be satisfied, at funding rather than at the first trade.
    ///
    /// A mandate whose daily loss limit exceeds its total drawdown is not strict-then-lenient,
    /// it is incoherent: the looser rule can never bind. Catching it here means an investor
    /// finds out while they are still reading the form.
    pub fn validate(&self) -> Result<()> {
        require!(self.max_trade_notional > 0, NoxError::InvalidMandateRules);
        require!(
            self.max_total_notional >= self.max_trade_notional,
            NoxError::InvalidMandateRules
        );
        require!(
            self.max_drawdown_bps > 0 && self.max_drawdown_bps as u64 <= crate::constants::BPS,
            NoxError::InvalidMandateRules
        );
        require!(
            self.max_daily_loss_bps > 0 && self.max_daily_loss_bps <= self.max_drawdown_bps,
            NoxError::InvalidMandateRules
        );
        require!(
            self.max_risk_per_trade_bps > 0 && self.max_risk_per_trade_bps <= self.max_drawdown_bps,
            NoxError::InvalidMandateRules
        );
        require!(
            self.max_stop_distance_bps > 0
                && self.max_stop_distance_bps as u64 <= crate::constants::BPS,
            NoxError::InvalidMandateRules
        );
        // Capped at the slot count, so the rule can never promise more positions than the
        // mandate has room to track.
        require!(
            self.max_concurrent_positions > 0
                && usize::from(self.max_concurrent_positions) <= crate::state::MAX_SLOTS,
            NoxError::InvalidMandateRules
        );
        require!(self.allowed_markets != 0, NoxError::InvalidMandateRules);
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(seq: u8)]
pub struct FundMandate<'info> {
    #[account(mut)]
    pub investor: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    /// CHECK: the trader this mandate authorises. Only its address is stored; it signs
    /// nothing here, which is the point — an investor funds a trader without their cooperation.
    pub trader: UncheckedAccount<'info>,

    /// The trader's record. Required, so a mandate can never be opened against a trader with
    /// no tier — and so the tier's size and concurrency limits have something to bind to.
    #[account(
        mut,
        seeds = [TRADER_SEED, trader.key().as_ref()],
        bump = trader_profile.bump,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    #[account(
        init,
        payer = investor,
        space = 8 + Mandate::INIT_SPACE,
        seeds = [MANDATE_SEED, investor.key().as_ref(), trader.key().as_ref(), &[seq]],
        bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// The dataless PDA that will be the SolFX authority.
    ///
    /// Created here with zero data so the System Program will later accept it as the `from` of
    /// the rent transfers `open_position` and `place_trigger_order` perform. A data-bearing
    /// account cannot serve — see `Mandate`'s documentation.
    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: the SolFX `UserAccount` this mandate will trade through. Validated by
    /// `solfx-core` on every CPI; only its address is recorded here.
    pub solfx_user_account: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn fund_mandate(
    ctx: Context<FundMandate>,
    seq: u8,
    principal: u64,
    rules: MandateRules,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    require!(principal > 0, NoxError::ZeroAmount);
    rules.validate()?;

    // The two things a tier actually binds. Both are checked here rather than at trade time,
    // because both are properties of the *mandate* and an investor should be refused while
    // they are still filling in the form, not after their capital has moved.
    let tier = ctx.accounts.trader_profile.tier;
    require!(
        principal <= tier.max_mandate(),
        NoxError::MandateExceedsTierLimit
    );
    require!(
        ctx.accounts.trader_profile.active_mandates < tier.max_concurrent_mandates(),
        NoxError::TooManyActiveMandates
    );

    let m = &mut ctx.accounts.mandate;
    m.investor = ctx.accounts.investor.key();
    m.trader = ctx.accounts.trader.key();
    m.seq = seq;
    m.solfx_user_account = ctx.accounts.solfx_user_account.key();
    m.signer_bump = ctx.bumps.mandate_signer;
    m.principal = principal;
    // Peak starts at principal, so a mandate that loses money from the first trade is already
    // measured as in drawdown rather than waiting for a high-water mark to be set.
    m.peak_equity = principal;
    m.max_trade_notional = rules.max_trade_notional;
    m.max_total_notional = rules.max_total_notional;
    m.max_drawdown_bps = rules.max_drawdown_bps;
    m.max_daily_loss_bps = rules.max_daily_loss_bps;
    m.max_risk_per_trade_bps = rules.max_risk_per_trade_bps;
    m.max_stop_distance_bps = rules.max_stop_distance_bps;
    m.max_concurrent_positions = rules.max_concurrent_positions;
    m.allowed_markets = rules.allowed_markets;
    m.min_hold_slots = rules.min_hold_slots;
    m.trader_split_bps = DEFAULT_TRADER_SPLIT_BPS;
    m.state = MandateState::Active;
    m.open_positions = 0;
    m.open_notional = 0;
    m.slots = Default::default();
    m.opened_at = Clock::get()?.unix_timestamp;
    m.bump = ctx.bumps.mandate;

    let profile = &mut ctx.accounts.trader_profile;
    profile.mandates_funded = profile.mandates_funded.saturating_add(1);
    profile.active_mandates = profile.active_mandates.saturating_add(1);

    emit!(MandateFunded {
        mandate: m.key(),
        investor: m.investor,
        trader: m.trader,
        principal,
        max_trade_notional: m.max_trade_notional,
        max_drawdown_bps: m.max_drawdown_bps,
        allowed_markets: m.allowed_markets,
        trader_split_bps: m.trader_split_bps,
        ts: m.opened_at,
    });
    Ok(())
}

// --- giving the mandate a SolFX account, and funding it -------------------------------------

/// Create the SolFX `UserAccount` the mandate trades through.
///
/// Separate from `fund_mandate` because it is a CPI and `fund_mandate` is not — keeping the
/// account lists apart keeps both well inside the stack frame, and a failure here is
/// recoverable by retrying rather than by re-funding.
#[derive(Accounts)]
pub struct CreateSolfxAccount<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        seeds = [MANDATE_SEED, mandate.investor.as_ref(), mandate.trader.as_ref(), &[mandate.seq]],
        bump = mandate.bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// Pays the `UserAccount`'s rent and becomes its authority. Dataless, so the System
    /// Program will accept it as the `from` of that rent transfer.
    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump = mandate.signer_bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub protocol: UncheckedAccount<'info>,
    /// CHECK: created by the CPI; validated by `solfx-core`, and bound to this mandate.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, crate::SolfxCore>,
}

pub fn create_solfx_account(ctx: Context<CreateSolfxAccount>) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    solfx_core::cpi::initialize_user_account(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::InitializeUserAccount {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
            },
            seeds,
        ),
        // No referrer. A mandate's volume belongs to the investor who funded it, not to
        // whoever introduced the trader.
        Pubkey::default(),
    )
}

/// Move USDC from the mandate's vault into SolFX's collateral vault.
///
/// `deposit_collateral` requires `user_token_account.owner == authority`, which is why the
/// vault is owned by `mandate_signer` and not by the `Mandate` account.
#[derive(Accounts)]
pub struct FundSolfxCollateral<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
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

    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub protocol: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`, and bound to this mandate.
    #[account(mut, address = mandate.solfx_user_account)]
    pub user_account: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core` against the protocol's recorded mint.
    ///
    /// `mut` because `MoveCollateral` marks it so — a CPI cannot ask for a privilege the outer
    /// instruction did not grant, and the runtime calls that `PrivilegeEscalation` rather than
    /// a missing-attribute error.
    #[account(mut)]
    pub collateral_mint: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core`.
    #[account(mut)]
    pub collateral_vault: UncheckedAccount<'info>,
    /// CHECK: the mandate's USDC account. `solfx-core` requires its owner to be the
    /// authority, which is `mandate_signer`.
    #[account(mut)]
    pub mandate_vault: UncheckedAccount<'info>,

    pub token_program: Program<'info, anchor_spl::token::Token>,
    #[account(address = config.solfx_program)]
    pub solfx_core_program: Program<'info, crate::SolfxCore>,
}

pub fn fund_solfx_collateral(ctx: Context<FundSolfxCollateral>, amount: u64) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    require!(amount > 0, NoxError::ZeroAmount);

    let mandate_key = ctx.accounts.mandate.key();
    let bump = ctx.accounts.mandate.signer_bump;
    let seeds: &[&[&[u8]]] = &[&[MANDATE_SIGNER_SEED, mandate_key.as_ref(), &[bump]]];

    solfx_core::cpi::deposit_collateral(
        CpiContext::new_with_signer(
            ctx.accounts.solfx_core_program.key(),
            solfx_core::cpi::accounts::MoveCollateral {
                authority: ctx.accounts.mandate_signer.to_account_info(),
                protocol: ctx.accounts.protocol.to_account_info(),
                user_account: ctx.accounts.user_account.to_account_info(),
                collateral_mint: ctx.accounts.collateral_mint.to_account_info(),
                collateral_vault: ctx.accounts.collateral_vault.to_account_info(),
                user_token_account: ctx.accounts.mandate_vault.to_account_info(),
                token_program: ctx.accounts.token_program.to_account_info(),
            },
            seeds,
        ),
        amount,
    )
}
