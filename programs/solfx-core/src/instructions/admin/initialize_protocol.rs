use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};
use solfx_math::fees::FeeSplitBps;

use crate::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED,
    LP_MINT_DECIMALS, LP_MINT_SEED, LP_POOL_SEED, LP_VAULT_SEED, PROTOCOL_SEED, USDC_DECIMALS,
};
use crate::errors::SolfxError;
use crate::events::ProtocolInitialized;
use crate::state::{InsuranceFund, LpPool, Protocol};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct InitializeProtocolParams {
    /// Pause-only key (ADR-008). Separate from `admin` on purpose: it is the one authority
    /// safe to hold hot, because the worst it can do is stop trading.
    pub guardian: Pubkey,
    pub fee_split_lp_bps: u16,
    pub fee_split_treasury_bps: u16,
    pub fee_split_insurance_bps: u16,
    pub fee_split_referral_bps: u16,
    /// Anti-JIT delay on LP withdrawals (threat T6). Phase 5 enforces it.
    pub lp_withdrawal_cooldown_seconds: u32,
    pub lp_exit_fee_bps: u16,
    /// § 6.9 targets 2% of maximum OI. Below 25% of this, markets go `ReduceOnly`.
    pub insurance_target_balance: u64,
}

/// One-time setup: config, the three USDC vaults, the insurance fund, and the LP pool with
/// its mint.
///
/// The LP accounts are created here even though add/remove liquidity is Phase 5. This
/// instruction can only ever run once, so deferring them would need a second initialisation
/// path — and the mint authority and vault addresses are exactly the things that must be
/// fixed from the start and never move.
#[derive(Accounts)]
pub struct InitializeProtocol<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + Protocol::INIT_SPACE,
        seeds = [PROTOCOL_SEED],
        bump,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    /// Collateral mint. Must be 6-decimal to match `QUOTE_PRECISION`; checked below.
    pub usdc_mint: Box<Account<'info, Mint>>,

    /// Holds every user's free collateral and, from Phase 3, every position's margin.
    #[account(
        init,
        payer = admin,
        seeds = [COLLATERAL_VAULT_SEED],
        bump,
        token::mint = usdc_mint,
        token::authority = protocol,
    )]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = admin,
        seeds = [FEE_VAULT_SEED],
        bump,
        token::mint = usdc_mint,
        token::authority = protocol,
    )]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = admin,
        space = 8 + InsuranceFund::INIT_SPACE,
        seeds = [INSURANCE_FUND_SEED],
        bump,
    )]
    pub insurance_fund: Box<Account<'info, InsuranceFund>>,

    #[account(
        init,
        payer = admin,
        seeds = [INSURANCE_VAULT_SEED],
        bump,
        token::mint = usdc_mint,
        token::authority = protocol,
    )]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = admin,
        space = 8 + LpPool::INIT_SPACE,
        seeds = [LP_POOL_SEED],
        bump,
    )]
    pub lp_pool: Box<Account<'info, LpPool>>,

    #[account(
        init,
        payer = admin,
        seeds = [LP_VAULT_SEED],
        bump,
        token::mint = usdc_mint,
        token::authority = lp_pool,
    )]
    pub lp_vault: Box<Account<'info, TokenAccount>>,

    /// `slpUSD`. Mint authority is the LP pool PDA, so no key can inflate LP shares.
    #[account(
        init,
        payer = admin,
        seeds = [LP_MINT_SEED],
        bump,
        mint::decimals = LP_MINT_DECIMALS,
        mint::authority = lp_pool,
    )]
    pub lp_mint: Box<Account<'info, Mint>>,

    /// Classic SPL Token, hard-pinned by type (threat T9). Token-2022 is not accepted:
    /// a transfer-fee or transfer-hook extension on the collateral mint would mean the
    /// amount credited differs from the amount received, silently breaking invariant I1.
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn init_protocol(
    ctx: Context<InitializeProtocol>,
    params: InitializeProtocolParams,
) -> Result<()> {
    require!(
        ctx.accounts.usdc_mint.decimals == USDC_DECIMALS,
        SolfxError::InvalidCollateralMintDecimals
    );

    let split = FeeSplitBps {
        lp: params.fee_split_lp_bps,
        treasury: params.fee_split_treasury_bps,
        insurance: params.fee_split_insurance_bps,
        referral: params.fee_split_referral_bps,
    };
    split.validate().map_err(|_| SolfxError::InvalidFeeSplit)?;

    // Under-paying LPs is the single most common cause of death for pool-backed perp
    // protocols: no liquidity, no depth, no traders, no fees (§ 8.3). Encoding it as a
    // constraint rather than a convention means a future admin cannot quietly invert it.
    require!(split.lp > split.treasury, SolfxError::InvalidFeeSplit);

    let clock = Clock::get()?;
    let protocol = &mut ctx.accounts.protocol;

    protocol.admin = ctx.accounts.admin.key();
    protocol.pending_admin = Pubkey::default();
    protocol.guardian = params.guardian;
    protocol.usdc_mint = ctx.accounts.usdc_mint.key();
    protocol.num_markets = 0;
    protocol.paused = false;
    protocol.set_fee_split(split)?;
    protocol.total_user_collateral = 0;
    protocol.total_deposits = 0;
    protocol.total_withdrawals = 0;
    protocol.bump = ctx.bumps.protocol;
    protocol.collateral_vault_bump = ctx.bumps.collateral_vault;
    protocol.fee_vault_bump = ctx.bumps.fee_vault;

    let insurance = &mut ctx.accounts.insurance_fund;
    insurance.balance = 0;
    insurance.target_balance = params.insurance_target_balance;
    insurance.total_bad_debt_covered = 0;
    insurance.bump = ctx.bumps.insurance_fund;
    insurance.vault_bump = ctx.bumps.insurance_vault;

    let lp = &mut ctx.accounts.lp_pool;
    lp.lp_mint = ctx.accounts.lp_mint.key();
    lp.lp_vault = ctx.accounts.lp_vault.key();
    lp.aum = 0;
    lp.lp_token_supply = 0;
    lp.withdrawal_cooldown_seconds = params.lp_withdrawal_cooldown_seconds;
    lp.exit_fee_bps = params.lp_exit_fee_bps;
    lp.high_water_mark_per_share = 0;
    lp.bump = ctx.bumps.lp_pool;
    lp.lp_vault_bump = ctx.bumps.lp_vault;
    lp.lp_mint_bump = ctx.bumps.lp_mint;

    emit!(ProtocolInitialized {
        admin: protocol.admin,
        guardian: protocol.guardian,
        usdc_mint: protocol.usdc_mint,
        lp_mint: lp.lp_mint,
        ts: clock.unix_timestamp,
    });

    Ok(())
}
