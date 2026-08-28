//! Liquidation and the bad-debt waterfall (`ARCHITECTURE.md` § 6.8, § 6.9).
//!
//! # Why this instruction is permissionless and generously paid
//!
//! § 6.8 is blunt about it: *an unprofitable liquidation is an unliquidated position, and
//! unliquidated positions are how vaults die.* Liquidations compete for blockspace during
//! exactly the congestion spikes that cause them, so the reward has to be worth paying a
//! priority fee for. A 40% share of a 0.5% penalty on a $10,000 position is $20 against a
//! transaction costing fractions of a cent. That margin is the point, not an oversight.
//!
//! # The waterfall
//!
//! When a position closes owing more than its collateral covers (§ 6.9):
//!
//! 1. **Insurance fund** covers the shortfall.
//! 2. **ADL** — force-close the most profitable opposing positions. *(Separate instruction:
//!    it cannot be done inside a liquidation, because the winners to deleverage are accounts
//!    this transaction does not carry.)*
//! 3. **LP vault NAV** absorbs the remainder pro rata.
//!
//! Phase 3 could only reach step 3. This adds step 1 and records what step 2 must clear.

use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_math::fees;

use crate::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, MIN_LIQUIDATOR_REWARD, POSITION_SEED, PROTOCOL_SEED, USER_SEED,
};
use crate::errors::{IntoProgramResult, SolfxError};
use crate::events::{BadDebtIncurred, PositionLiquidated};
use crate::instructions::trader::flows::flows_for_allocation;
use crate::instructions::trader::{settle, SettlementInput, VaultTransfer};
use crate::oracle::load_price_for_liquidation;
use crate::risk;
use crate::state::{InsuranceFund, LpPool, Market, Position, Protocol, UserAccount};

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    /// Anyone. Publishing the liquidator source and encouraging third parties is the
    /// resilient design (§ 9.2) — a permissionless liquidation market does not have an
    /// on-call rota.
    #[account(mut)]
    pub liquidator: Signer<'info>,

    #[account(mut, seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    /// The position's owner. Any residual equity is returned to them — a liquidation takes
    /// the penalty, not the account.
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

    /// Closed and its rent returned to the position owner, not to the liquidator. The
    /// liquidator is paid from the penalty; taking the rent as well would be taking the
    /// trader's deposit.
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

    /// CHECK: must be the position owner's wallet; enforced below.
    #[account(mut, address = user_account.authority @ SolfxError::AuthorityMismatch)]
    pub rent_destination: UncheckedAccount<'info>,

    /// The liquidator's share is credited here, so they need no prior account with us.
    #[account(
        mut,
        constraint = liquidator_token_account.mint == protocol.usdc_mint @ SolfxError::WrongCollateralMint,
        constraint = liquidator_token_account.owner == liquidator.key() @ SolfxError::AuthorityMismatch,
    )]
    pub liquidator_token_account: Box<Account<'info, TokenAccount>>,

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

/// Close a position whose equity has fallen below its maintenance margin.
///
/// # Order, and why
///
/// 1. **Settle funding and carry first.** They are already owed; a position must be judged on
///    what it is worth *after* its accrued costs, or a position kept alive by unbilled carry
///    would be liquidated late and at a worse price.
/// 2. **Assess.** The § 6.8 equity excludes the close fee, because the penalty replaces it.
/// 3. **Require `equity < maintenance_margin`, strictly.** A loose comparison is liquidation
///    griefing (threat T7): an attacker could liquidate positions sitting exactly on the
///    boundary, collecting a penalty from a solvent trader.
/// 4. **Penalty**, capped at whatever positive equity remains — never manufacture a debt in
///    order to pay a reward.
/// 5. **Settle**, then return any residue to the owner.
///
/// # A liquidation is not a punishment
///
/// Whatever survives the penalty goes back to the trader's free collateral, and the position
/// account's rent goes back to their wallet. The protocol takes what it is owed and nothing
/// more.
pub fn liquidate_position(ctx: Context<LiquidatePosition>) -> Result<()> {
    let clock = Clock::get()?;

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

    // 1. Accrued costs are settled before the position is judged.
    let carry = risk::settle_carry(&mut ctx.accounts.position, &ctx.accounts.market)?;
    risk::settle_funding(&mut ctx.accounts.position, &mut ctx.accounts.market)?;

    // 2 and 3. Assess against the now-current collateral.
    let health = risk::assess(&ctx.accounts.position, &ctx.accounts.market, &price)?;
    require!(health.is_liquidatable(), SolfxError::NotLiquidatable);

    let position = &ctx.accounts.position;
    let market = &ctx.accounts.market;
    let size = position.size_base;
    let exit_price = health.mark;

    // 4. Penalty, and the split of everything the position gives up.
    //
    // # The cap that matters
    //
    // A liquidated position can only ever give up **what it holds**. Carry and funding were
    // already settled out of `collateral` above, so:
    //
    //     equity = collateral + uPnL
    //
    // When that is negative the trader owes more than they posted. The pool receives the
    // collateral and no more — taking the difference would be taking it out of *other
    // traders'* collateral, which sits in the same vault. The shortfall is bad debt, and the
    // waterfall below is what covers it.
    // `settle_carry` above removed the accrued carry from `position.collateral`, but it moved
    // no tokens — they are still sitting in the collateral vault, and they are still counted
    // in `Protocol::total_user_collateral`. The distribution below must therefore be based on
    // the **pre-carry** balance, or exactly `carry` units are stranded in the vault owned by
    // nobody, and invariant I1 drifts by that amount on every liquidation.
    //
    // `equity` deliberately stays on the post-carry figure: carry is a real cost the position
    // has borne, so it must reduce what the trader gets back. The two are not in tension —
    // the carry leaves trader ownership here, and `routed_fee` below is where it lands.
    let collateral = position
        .collateral
        .checked_add(carry)
        .ok_or(SolfxError::MathOverflow)?;
    let equity = health.liquidation_equity;

    let full_penalty = fees::fee_amount(
        health.notional,
        u64::from(market.liquidation_fee_bps).saturating_mul(100_000),
    )
    .or_program_err()?;

    // A penalty can only come out of positive equity. Charging one against a shortfall would
    // manufacture debt in order to pay a reward.
    let penalty = full_penalty.min(u64::try_from(equity.max(0)).unwrap_or(0));
    let split = fees::split_liquidation_penalty(penalty).or_program_err()?;

    // Whatever survives goes back to the trader. A liquidation takes what it is owed, not
    // the account.
    let credit = u64::try_from(
        equity
            .saturating_sub(i64::try_from(penalty).map_err(|_| SolfxError::MathOverflow)?)
            .max(0),
    )
    .map_err(|_| SolfxError::MathOverflow)?;

    let bad_debt =
        u64::try_from((-i128::from(equity)).max(0)).map_err(|_| SolfxError::MathOverflow)?;

    // Everything the position gives up, split three ways in priority order: the liquidator
    // is paid first (or nobody liquidates), then the fee vaults, and the pool takes the
    // remainder as the counterparty's claim.
    let given_up = collateral
        .checked_sub(credit)
        .ok_or(SolfxError::CollateralAccountingUnderflow)?;

    let liquidator_out = split.liquidator.min(given_up);
    let after_liquidator = given_up.saturating_sub(liquidator_out);

    // Carry is ordinary revenue and takes the standard four-way split. The penalty's
    // non-liquidator shares are not: § 6.8 fixes them at insurance 40% / treasury 20% of the
    // penalty, and passing them through `split_fee` would re-divide them by the trading
    // schedule — which is how the insurance fund ends up with 6% of a penalty instead of 40%.
    //
    // Carry is paid first when the position cannot cover everything: it is a debt already
    // incurred, where the penalty is a charge levied now.
    let carry_routed = carry.min(after_liquidator);
    let penalty_budget = after_liquidator.saturating_sub(carry_routed);
    let insurance_share = split.insurance.min(penalty_budget);
    let treasury_share = split
        .treasury
        .min(penalty_budget.saturating_sub(insurance_share));

    let routed_fee = carry_routed
        .checked_add(insurance_share)
        .ok_or(SolfxError::MathOverflow)?
        .checked_add(treasury_share)
        .ok_or(SolfxError::MathOverflow)?;

    let pool_in = after_liquidator.saturating_sub(routed_fee);

    // The insurance fund covers the shortfall — and, if the position could not pay the
    // liquidator, the reward as well.
    //
    // A liquidator paid nothing does not run, and a position nobody liquidates keeps
    // falling. Paying a dollar out of the fund to close a position now, rather than letting
    // its shortfall grow, is not a close call.
    let reward_top_up = if liquidator_out < MIN_LIQUIDATOR_REWARD {
        MIN_LIQUIDATOR_REWARD.saturating_sub(liquidator_out)
    } else {
        0
    };
    let insurance_available = ctx.accounts.insurance_fund.balance;
    let insurance_draw = bad_debt.min(insurance_available);
    let reward_from_insurance =
        reward_top_up.min(insurance_available.saturating_sub(insurance_draw));
    let uncovered = bad_debt.saturating_sub(insurance_draw);
    let liquidator_total = liquidator_out
        .checked_add(reward_from_insurance)
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

    // `settle` takes the trader's PnL, so the pool's receipt is its negation.
    let settlement_pnl = -i64::try_from(pool_in).map_err(|_| SolfxError::MathOverflow)?;
    // The carry portion splits the normal way; the penalty portion goes where § 6.8 sends it.
    let carry_alloc =
        fees::split_fee(carry_routed, ctx.accounts.protocol.fee_split()).or_program_err()?;
    let alloc = fees::FeeAllocation {
        lp: carry_alloc.lp,
        treasury: carry_alloc
            .treasury
            .checked_add(treasury_share)
            .ok_or(SolfxError::MathOverflow)?,
        insurance: carry_alloc
            .insurance
            .checked_add(insurance_share)
            .ok_or(SolfxError::MathOverflow)?,
        referral: carry_alloc.referral,
    };
    let flows = flows_for_allocation(alloc, settlement_pnl)?;

    let lp_balance = ctx.accounts.lp_vault.amount;
    let market_index = ctx.accounts.market.market_index;
    let user_key = ctx.accounts.user_account.key();
    let referrer = ctx.accounts.position.referrer;
    let position_key = ctx.accounts.position.key();
    let direction = ctx.accounts.position.direction;
    let entry_notional = ctx.accounts.position.entry_notional;

    settle(
        SettlementInput {
            fee: routed_fee,
            pnl: settlement_pnl,
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

    // The liquidator's reward leaves the protocol entirely — the only USDC in this
    // instruction that does. Paid straight from the collateral vault to their wallet, so
    // they need no prior account with us.
    pay_liquidator(&ctx, liquidator_out)?;
    if liquidator_out > 0 {
        let protocol = &mut ctx.accounts.protocol;
        protocol.total_user_collateral = protocol
            .total_user_collateral
            .checked_sub(liquidator_out)
            .ok_or(SolfxError::CollateralAccountingUnderflow)?;
    }
    // The top-up comes from the insurance vault, not the collateral vault: it is not the
    // trader's money and must not be taken from anyone else's either.
    if reward_from_insurance > 0 {
        pay_liquidator_from_insurance(&ctx, reward_from_insurance)?;
        let insurance = &mut ctx.accounts.insurance_fund;
        insurance.balance = insurance
            .balance
            .checked_sub(reward_from_insurance)
            .ok_or(SolfxError::MathOverflow)?;
    }
    if liquidator_total > 0 {
        let protocol = &mut ctx.accounts.protocol;
        protocol.total_liquidator_paid = protocol
            .total_liquidator_paid
            .checked_add(liquidator_total)
            .ok_or(SolfxError::MathOverflow)?;
    }

    // Step 1 of the waterfall: insurance absorbs what it can.
    if insurance_draw > 0 {
        cover_from_insurance(&ctx, insurance_draw)?;
        let insurance = &mut ctx.accounts.insurance_fund;
        insurance.balance = insurance
            .balance
            .checked_sub(insurance_draw)
            .ok_or(SolfxError::MathOverflow)?;
        insurance.total_bad_debt_covered = insurance
            .total_bad_debt_covered
            .checked_add(insurance_draw)
            .ok_or(SolfxError::MathOverflow)?;
        let lp = &mut ctx.accounts.lp_pool;
        lp.aum = lp
            .aum
            .checked_add(insurance_draw)
            .ok_or(SolfxError::MathOverflow)?;
    }

    // Step 2's queue: what insurance could not cover, ADL must clear.
    if uncovered > 0 {
        let market = &mut ctx.accounts.market;
        market.pending_adl_debt = market
            .pending_adl_debt
            .checked_add(uncovered)
            .ok_or(SolfxError::MathOverflow)?;
    }

    if bad_debt > 0 {
        let protocol = &mut ctx.accounts.protocol;
        protocol.total_bad_debt = protocol
            .total_bad_debt
            .checked_add(bad_debt)
            .ok_or(SolfxError::MathOverflow)?;
        emit!(BadDebtIncurred {
            market_index,
            position: position_key,
            amount: bad_debt,
            ts: clock.unix_timestamp,
        });
    }

    // Book-keeping.
    let market = &mut ctx.accounts.market;
    market.remove_open_interest(direction, entry_notional, size)?;
    market.open_position_count = market.open_position_count.saturating_sub(1);

    let user = &mut ctx.accounts.user_account;
    user.credit(credit)?;
    user.open_positions = user.open_positions.saturating_sub(1);

    let position = &mut ctx.accounts.position;
    position.size_base = 0;
    position.collateral = 0;
    position.entry_notional = 0;

    emit!(PositionLiquidated {
        position: position_key,
        user_account: user_key,
        liquidator: ctx.accounts.liquidator.key(),
        market_index,
        size_base: size,
        exit_price,
        equity: health.liquidation_equity,
        maintenance_margin: health.maintenance_margin,
        penalty,
        liquidator_reward: liquidator_total,
        residual_to_owner: credit,
        bad_debt,
        insurance_drawn: insurance_draw,
        uncovered_bad_debt: uncovered,
        ts: clock.unix_timestamp,
    });

    Ok(())
}

fn pay_liquidator(ctx: &Context<LiquidatePosition>, amount: u64) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let seeds: &[&[&[u8]]] = &[&[PROTOCOL_SEED, &[ctx.accounts.protocol.bump]]];
    anchor_spl::token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            anchor_spl::token::Transfer {
                from: ctx.accounts.collateral_vault.to_account_info(),
                to: ctx.accounts.liquidator_token_account.to_account_info(),
                authority: ctx.accounts.protocol.to_account_info(),
            },
            seeds,
        ),
        amount,
    )
}

/// Pay a liquidator out of the insurance fund when the position could not.
fn pay_liquidator_from_insurance(ctx: &Context<LiquidatePosition>, amount: u64) -> Result<()> {
    let seeds: &[&[&[u8]]] = &[&[PROTOCOL_SEED, &[ctx.accounts.protocol.bump]]];
    anchor_spl::token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            anchor_spl::token::Transfer {
                from: ctx.accounts.insurance_vault.to_account_info(),
                to: ctx.accounts.liquidator_token_account.to_account_info(),
                authority: ctx.accounts.protocol.to_account_info(),
            },
            seeds,
        ),
        amount,
    )
}

/// Move USDC from the insurance vault into the LP vault, making the pool whole.
fn cover_from_insurance(ctx: &Context<LiquidatePosition>, amount: u64) -> Result<()> {
    let seeds: &[&[&[u8]]] = &[&[PROTOCOL_SEED, &[ctx.accounts.protocol.bump]]];
    anchor_spl::token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            anchor_spl::token::Transfer {
                from: ctx.accounts.insurance_vault.to_account_info(),
                to: ctx.accounts.lp_vault.to_account_info(),
                authority: ctx.accounts.protocol.to_account_info(),
            },
            seeds,
        ),
        amount,
    )
}

/// Seed or top up the insurance fund (admin).
///
/// § 6.9: target 2% of maximum OI at launch, funded from the operator's own capital, then
/// grown from the 10% insurance share of fees. **Do not launch with an empty insurance
/// fund** — the first gap event hits LPs directly and they do not come back.
#[derive(Accounts)]
pub struct DepositInsuranceFund<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(mut, seeds = [INSURANCE_FUND_SEED], bump = insurance_fund.bump)]
    pub insurance_fund: Box<Account<'info, InsuranceFund>>,

    #[account(mut, seeds = [INSURANCE_VAULT_SEED], bump = insurance_fund.vault_bump)]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = payer_token_account.mint == protocol.usdc_mint @ SolfxError::WrongCollateralMint,
        constraint = payer_token_account.owner == payer.key() @ SolfxError::AuthorityMismatch,
    )]
    pub payer_token_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

/// Anyone may top up the insurance fund. There is nothing to gain by doing so and the
/// protocol is strictly safer for it, so requiring an authority would only add a way for the
/// fund to be under-capitalised while someone waits for a multisig.
pub fn deposit_insurance_fund(ctx: Context<DepositInsuranceFund>, amount: u64) -> Result<()> {
    require!(amount > 0, SolfxError::ZeroAmount);

    anchor_spl::token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            anchor_spl::token::Transfer {
                from: ctx.accounts.payer_token_account.to_account_info(),
                to: ctx.accounts.insurance_vault.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            },
        ),
        amount,
    )?;

    let fund = &mut ctx.accounts.insurance_fund;
    fund.balance = fund
        .balance
        .checked_add(amount)
        .ok_or(SolfxError::MathOverflow)?;

    Ok(())
}
