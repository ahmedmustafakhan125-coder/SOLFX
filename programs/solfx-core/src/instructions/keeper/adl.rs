//! Auto-deleveraging — the last resort before LPs eat a shortfall (`ARCHITECTURE.md` § 6.9).
//!
//! # What it is, plainly
//!
//! When a position gaps through its liquidation price, the loss can exceed the collateral
//! backing it. The pool is owed money the trader does not have. The waterfall is:
//!
//! 1. Insurance fund covers it.
//! 2. **ADL** — the most profitable *opposing* positions are force-closed, and part of their
//!    profit is withheld to cover the shortfall.
//! 3. LP vault NAV absorbs whatever is left.
//!
//! Step 2 takes money from traders who did nothing wrong. That is the honest description, and
//! **§ 6.9 requires telling traders it exists, in plain language, before they open a
//! position.** Every venue that hid it and then used it was destroyed on social media.
//!
//! # Why it is a separate instruction
//!
//! A liquidation cannot deleverage in the same transaction: the winners to close are accounts
//! it does not carry, and there is no bound on how many would be needed. So a liquidation
//! records the uncovered shortfall on `Market::pending_adl_debt`, and this instruction draws
//! it down one position at a time — permissionlessly, so it cannot stall waiting for an
//! operator.

use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, POSITION_SEED, PROTOCOL_SEED, USER_SEED,
};
use crate::errors::SolfxError;
use crate::events::PositionAutoDeleveraged;
use crate::instructions::trader::flows::compute_flows;
use crate::instructions::trader::{settle, SettlementInput, VaultTransfer};
use crate::oracle::load_price_for_liquidation;
use crate::risk;
use crate::state::{InsuranceFund, LpPool, Market, Position, Protocol, UserAccount};

#[derive(Accounts)]
pub struct AutoDeleverage<'info> {
    pub keeper: Signer<'info>,

    #[account(mut, seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [USER_SEED, user_account.authority.as_ref()],
        bump = user_account.bump,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        close = rent_destination,
        seeds = [
            POSITION_SEED,
            user_account.key().as_ref(),
            &position.market_index.to_le_bytes(),
            &[position.nonce],
        ],
        bump = position.bump,
        constraint = position.user_account == user_account.key() @ SolfxError::AuthorityMismatch,
        constraint = position.market_index == market.market_index @ SolfxError::PositionMarketMismatch,
    )]
    pub position: Box<Account<'info, Position>>,

    /// CHECK: the position owner's wallet; the rent is theirs.
    #[account(mut, address = user_account.authority @ SolfxError::AuthorityMismatch)]
    pub rent_destination: UncheckedAccount<'info>,

    #[account(mut, seeds = [COLLATERAL_VAULT_SEED], bump = protocol.collateral_vault_bump)]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [LP_POOL_SEED], bump = lp_pool.bump)]
    pub lp_pool: Box<Account<'info, LpPool>>,
    #[account(mut, seeds = [LP_VAULT_SEED], bump = lp_pool.lp_vault_bump)]
    pub lp_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [INSURANCE_FUND_SEED], bump = insurance_fund.bump)]
    pub insurance_fund: Box<Account<'info, InsuranceFund>>,
    #[account(mut, seeds = [INSURANCE_VAULT_SEED], bump = insurance_fund.vault_bump)]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [FEE_VAULT_SEED], bump = protocol.fee_vault_bump)]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    pub price_update: Box<Account<'info, PriceUpdateV2>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    pub token_program: Program<'info, Token>,
}

/// Force-close a profitable position and withhold part of its profit to cover a shortfall.
///
/// # Eligibility
///
/// Two conditions, both required:
///
/// 1. **There is uncovered debt.** `market.pending_adl_debt > 0`. Without it there is nothing
///    to socialise and this instruction is just theft.
/// 2. **The position is profitable.** § 6.9 ranks candidates by `uPnL % of collateral`
///    descending — the most profitable first. Ranking is the keeper's job (it needs to see
///    every position, which no single instruction can), but the *eligibility floor* is
///    enforced here: a position that is not in profit cannot be deleveraged at all.
///
/// The keeper choosing a less-profitable position than it should is a fairness problem, not a
/// solvency one, and it is visible in the event stream. Enforcing a strict ordering on chain
/// would require loading every position in the market, which is not possible.
///
/// # What the trader gets
///
/// Their collateral back in full, plus whatever profit survives the withholding. They are
/// never worse off than flat — ADL takes profit, never principal. If the shortfall exceeds
/// this position's profit, the remainder stays on `pending_adl_debt` for the next one.
pub fn auto_deleverage(ctx: Context<AutoDeleverage>) -> Result<()> {
    let clock = Clock::get()?;

    require!(
        ctx.accounts.market.pending_adl_debt > 0,
        SolfxError::NoPendingAdlDebt
    );
    require!(
        ctx.accounts.market.status.allows_liquidation(),
        SolfxError::MarketNotActive
    );

    let price = load_price_for_liquidation(
        &ctx.accounts.market,
        &ctx.accounts.price_update,
        ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
        ctx.accounts
            .quote_conversion_price_update
            .as_deref()
            .map(|a| &**a),
        &clock,
    )?;

    let carry = risk::settle_carry(&mut ctx.accounts.position, &ctx.accounts.market)?;
    risk::settle_funding(&mut ctx.accounts.position, &mut ctx.accounts.market)?;

    let health = risk::assess(&ctx.accounts.position, &ctx.accounts.market, &price)?;
    require!(health.upnl > 0, SolfxError::NotAdlEligible);

    let gross_pnl = health.upnl;
    let pending = ctx.accounts.market.pending_adl_debt;

    // Withhold up to the whole profit, never more, and never any of the principal.
    let socialised = pending.min(u64::try_from(gross_pnl).unwrap_or(0));
    let paid_pnl = gross_pnl
        .checked_sub(i64::try_from(socialised).map_err(|_| SolfxError::MathOverflow)?)
        .ok_or(SolfxError::MathOverflow)?;

    let transfer = VaultTransfer {
        token_program: ctx.accounts.token_program.to_account_info(),
        collateral_vault: ctx.accounts.collateral_vault.to_account_info(),
        lp_vault: ctx.accounts.lp_vault.to_account_info(),
        insurance_vault: ctx.accounts.insurance_vault.to_account_info(),
        fee_vault: ctx.accounts.fee_vault.to_account_info(),
        protocol: ctx.accounts.protocol.to_account_info(),
        protocol_bump: ctx.accounts.protocol.bump,
        lp_pool: ctx.accounts.lp_pool.to_account_info(),
        lp_pool_bump: ctx.accounts.lp_pool.bump,
    };

    // No close fee: the trader did not choose to close. Charging one for a forced exit would
    // be charging them for the protocol's own solvency problem.
    let (alloc, flows) = compute_flows(carry, paid_pnl, ctx.accounts.protocol.fee_split())?;

    let lp_balance = ctx.accounts.lp_vault.amount;
    let market_index = ctx.accounts.market.market_index;
    let user_key = ctx.accounts.user_account.key();
    let referrer = ctx.accounts.position.referrer;
    let position_key = ctx.accounts.position.key();
    let direction = ctx.accounts.position.direction;
    let entry_notional = ctx.accounts.position.entry_notional;
    let size = ctx.accounts.position.size_base;
    let collateral = ctx.accounts.position.collateral;

    settle(
        SettlementInput {
            fee: carry,
            pnl: paid_pnl,
            market_index,
            user_account: user_key,
            referrer,
        },
        flows,
        alloc,
        &transfer,
        &mut ctx.accounts.protocol,
        &mut ctx.accounts.lp_pool,
        &mut ctx.accounts.insurance_fund,
        &mut ctx.accounts.market,
        lp_balance,
        clock.unix_timestamp,
    )?;

    let market = &mut ctx.accounts.market;
    market.pending_adl_debt = market.pending_adl_debt.saturating_sub(socialised);
    market.remove_open_interest(direction, entry_notional, size)?;
    market.open_position_count = market.open_position_count.saturating_sub(1);
    let remaining_debt = market.pending_adl_debt;

    // Principal plus whatever profit survived.
    let credit = u64::try_from(
        i128::from(collateral)
            .checked_add(i128::from(paid_pnl))
            .ok_or(SolfxError::MathOverflow)?,
    )
    .map_err(|_| SolfxError::MathOverflow)?;

    let user = &mut ctx.accounts.user_account;
    user.credit(credit)?;
    user.open_positions = user.open_positions.saturating_sub(1);

    let position = &mut ctx.accounts.position;
    position.size_base = 0;
    position.collateral = 0;
    position.entry_notional = 0;

    emit!(PositionAutoDeleveraged {
        position: position_key,
        user_account: user_key,
        market_index,
        size_base: size,
        exit_price: health.mark,
        gross_pnl,
        socialised,
        paid_out: credit,
        remaining_adl_debt: remaining_debt,
        ts: clock.unix_timestamp,
    });

    Ok(())
}
