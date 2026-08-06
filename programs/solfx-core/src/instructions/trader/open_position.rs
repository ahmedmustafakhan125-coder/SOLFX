use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solfx_math::{fees, margin, pnl, pricing, Side, TradeAction};

use crate::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, POSITION_SEED, PROTOCOL_SEED, USER_SEED,
};
use crate::errors::{IntoProgramResult, SolfxError};
use crate::events::PositionOpened;
use crate::oracle::{load_validated_price, MarketPrice};
use crate::state::{Direction, InsuranceFund, LpPool, Market, Position, Protocol, UserAccount};

use super::flows::compute_flows;
use super::{settle, SettlementInput, VaultTransfer};

#[derive(Accounts)]
#[instruction(market_index: u16, nonce: u8)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(mut, seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [USER_SEED, authority.key().as_ref()],
        bump = user_account.bump,
        has_one = authority @ SolfxError::AuthorityMismatch,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        init,
        payer = authority,
        space = 8 + Position::INIT_SPACE,
        seeds = [
            POSITION_SEED,
            user_account.key().as_ref(),
            &market_index.to_le_bytes(),
            &[nonce],
        ],
        bump,
    )]
    pub position: Box<Account<'info, Position>>,

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
    pub system_program: Program<'info, System>,
}

/// Open a leveraged position.
///
/// `price_limit` is the trader's slippage bound: a **maximum** when they are buying (opening
/// a long), a **minimum** when selling (opening a short). The fill is priced adverse to them
/// before the bound is checked, so the bound is tested against what they will actually get
/// rather than against the oracle mid.
///
/// # Order of operations, and why it is this order
///
/// 1. **Status.** A market that does not permit opens rejects before anything is computed.
/// 2. **Price.** Through the validated path, gates and all. A trade cannot be sized against
///    a price the protocol would not itself trade on.
/// 3. **Execution price.** Oracle mid, plus base spread, plus a confidence-scaled component,
///    plus skew impact — always against the trader (§ 6.6). This is the layer that makes
///    latency arbitrage unprofitable (T2), and it must be applied *before* notional is
///    computed so the fee and margin are both taken on the real fill.
/// 4. **Slippage bound.**
/// 5. **Size, notional and margin**, all in USDC after quote conversion (C-3).
/// 6. **Fee**, charged on top of the margin, from free collateral.
/// 7. **OI caps**, checked against the resulting book rather than the trade.
pub fn open_position(
    ctx: Context<OpenPosition>,
    market_index: u16,
    nonce: u8,
    direction: Direction,
    size_base: u64,
    collateral: u64,
    price_limit: i64,
) -> Result<()> {
    let clock = Clock::get()?;

    require!(!ctx.accounts.protocol.paused, SolfxError::ProtocolPaused);
    require!(
        ctx.accounts.market.status.allows_open(),
        SolfxError::MarketClosedForOpens
    );
    require!(size_base > 0, SolfxError::ZeroAmount);
    require!(collateral > 0, SolfxError::ZeroAmount);

    let market_ro = &ctx.accounts.market;
    let price: MarketPrice = load_validated_price(
        market_ro,
        &ctx.accounts.price_update,
        ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
        ctx.accounts
            .quote_conversion_price_update
            .as_deref()
            .map(|a| &**a),
        &clock,
    )?;

    let side = Side::resolve(direction.into(), TradeAction::Open);
    let exec_price = execution_price_for(market_ro, &price, size_base, side)?;
    pricing::validate_slippage(exec_price, price_limit, side)
        .map_err(|_| error!(SolfxError::SlippageExceeded))?;

    // Notional in the market's quote currency, then in USDC. For USD/INR these differ by a
    // factor of ~88; booking the first as the second is correction C-3's failure mode.
    let notional_quote = pnl::notional_in_quote(size_base, exec_price).or_program_err()?;
    let conversion_rate = price.quote_conversion_rate.unwrap_or(0);
    let notional = pnl::convert_cost_to_collateral(
        notional_quote,
        market_ro.quote_conversion(),
        conversion_rate,
    )
    .or_program_err()?;

    pricing::validate_notional(notional).or_program_err()?;
    require!(
        size_base >= market_ro.min_position_size && size_base <= market_ro.max_position_size,
        SolfxError::PositionSizeOutOfBounds
    );

    // Both the leverage cap and the initial-margin ratio are checked. They express the same
    // constraint and `validate_risk_params` keeps them consistent, but checking only one
    // would make the other decorative.
    let leverage = margin::effective_leverage(notional, collateral).or_program_err()?;
    require!(
        leverage <= u64::from(market_ro.effective_max_leverage()),
        SolfxError::LeverageTooHigh
    );
    let required = margin::initial_margin(notional, market_ro.imr_bps).or_program_err()?;
    require!(collateral >= required, SolfxError::InsufficientMargin);

    // Two sources of price, composed deliberately: the market's rate is a ceiling set by its
    // risk profile, and § 8.2's volume schedule is the published discount. A trader pays the
    // better of the two, so a small trader is capped by the market and a large one is
    // discounted by their volume.
    let tier_rate = fees::fee_rate_for_volume(ctx.accounts.user_account.thirty_day_volume);
    let fee =
        fees::fee_amount(notional, market_ro.open_fee_rate.min(tier_rate)).or_program_err()?;

    // Margin and fee both come out of free collateral; the margin moves into the position,
    // the fee leaves the trader entirely.
    let total_debit = collateral
        .checked_add(fee)
        .ok_or(SolfxError::MathOverflow)?;
    ctx.accounts.user_account.debit(total_debit)?;

    let (alloc, flows) = compute_flows(fee, 0, ctx.accounts.protocol.fee_split())?;

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
    let lp_balance = ctx.accounts.lp_vault.amount;
    let referrer = ctx.accounts.user_account.referrer;
    let user_key = ctx.accounts.user_account.key();

    settle(
        SettlementInput {
            fee,
            pnl: 0,
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
    market.add_open_interest(direction, notional, size_base)?;

    // The § 7.2 and § 7.4 vault controls, checked on the resulting book.
    //
    // All three are *pool-relative* rather than per-market, which is why they arrived with
    // Phase 5 rather than Phase 4: each one is a statement about whether the vault can still
    // stand behind the trade, and the vault's own instructions did not exist until now.
    market.check_oi_cap(direction)?;
    market.check_skew_cap(
        direction,
        ctx.accounts.protocol.max_skew_bps,
        ctx.accounts.lp_pool.aum,
    )?;

    check_utilisation(
        market,
        ctx.accounts.lp_pool.aum,
        ctx.accounts.protocol.max_utilisation_bps,
    )?;
    check_insurance_floor(
        ctx.accounts.insurance_fund.balance,
        ctx.accounts.insurance_fund.target_balance,
        ctx.accounts.protocol.min_insurance_ratio_bps,
    )?;

    market.open_position_count = market
        .open_position_count
        .checked_add(1)
        .ok_or(SolfxError::MathOverflow)?;

    let position = &mut ctx.accounts.position;
    position.user_account = user_key;
    position.market_index = market_index;
    position.nonce = nonce;
    position.direction = direction;
    position.size_base = size_base;
    position.entry_price = exec_price;
    position.collateral = collateral;
    position.entry_notional = notional;
    position.cum_funding_entry = match direction {
        Direction::Long => market.cum_funding_long,
        Direction::Short => market.cum_funding_short,
    };
    position.cum_borrow_entry = market.cum_borrow_index;
    position.realized_funding = 0;
    position.opened_at = clock.unix_timestamp;
    position.opened_at_slot = clock.slot;
    position.last_updated_at = clock.unix_timestamp;
    position.open_fee_paid = fee;
    position.referrer = referrer;
    position.bump = ctx.bumps.position;

    let user = &mut ctx.accounts.user_account;
    user.open_positions = user
        .open_positions
        .checked_add(1)
        .ok_or(SolfxError::MathOverflow)?;
    user.thirty_day_volume = user.thirty_day_volume.saturating_add(notional);

    emit!(PositionOpened {
        position: position.key(),
        user_account: user_key,
        market_index,
        direction: direction as u8,
        size_base,
        oracle_price: price.spot.price,
        exec_price,
        spread_bps: spread_of(price.spot.price, exec_price),
        notional_collateral: notional,
        collateral,
        fee,
        referrer,
        ts: clock.unix_timestamp,
    });

    Ok(())
}

/// Oracle mid plus every adverse component (§ 6.6).
///
/// Shared by open and close so the two can never disagree about how a fill is priced — which
/// is precisely the disagreement a round-trip exploit would live in.
pub fn execution_price_for(
    market: &Market,
    price: &MarketPrice,
    size_base: u64,
    side: Side,
) -> Result<i64> {
    let conf_spread =
        pricing::confidence_spread_bps(price.spot.conf_bps, market.conf_spread_multiplier_bps)
            .or_program_err()?;

    // Signed size delta: how this trade moves the book. A buy adds to longs, a sell to shorts.
    let signed_delta = match side {
        Side::Buy => i128::from(size_base),
        Side::Sell => -i128::from(size_base),
    };
    let skew = pricing::skew_delta(market.base_oi_long, market.base_oi_short, signed_delta)
        .or_program_err()?;
    let skew_impact =
        pricing::skew_impact_bps(skew, market.skew_impact_bps_per_unit).or_program_err()?;

    let total =
        pricing::total_spread_bps(market.effective_base_spread_bps(), conf_spread, skew_impact)
            .or_program_err()?;

    pricing::execution_price(price.spot.price, total, side).or_program_err()
}

/// Reconstruct the applied spread for the event, in bps.
///
/// Reuses `solfx_math::oracle::deviation_bps` rather than dividing here: it is the same
/// computation, and a second copy is a second place for the rounding direction to drift.
fn spread_of(oracle: i64, exec: i64) -> u64 {
    solfx_math::oracle::deviation_bps(exec, oracle).unwrap_or(0)
}

/// § 7.2's vault utilisation ceiling.
///
/// A pool cannot credibly stand behind unlimited open interest. Past the ceiling it stops
/// taking on more, whatever the per-market caps allow — the per-market caps bound one
/// instrument's risk, this bounds the *pool's*.
///
/// Measured against total notional rather than § 7.4's "max loss exposure", because max loss
/// is not knowable without walking every position. Notional is the conservative reading and
/// the only one an instruction can compute.
fn check_utilisation(market: &Market, aum: u64, max_utilisation_bps: u16) -> Result<()> {
    if max_utilisation_bps == 0 {
        return Ok(());
    }
    // An empty pool backs nothing. Rejecting here rather than dividing by zero also means a
    // market cannot be traded before anyone has provided liquidity.
    require!(aum > 0, SolfxError::UtilisationCapReached);

    let exposure = u128::from(market.oi_long)
        .checked_add(u128::from(market.oi_short))
        .ok_or(SolfxError::MathOverflow)?;
    let utilisation = exposure
        .checked_mul(10_000)
        .ok_or(SolfxError::MathOverflow)?
        .checked_div(u128::from(aum))
        .ok_or(SolfxError::DivideByZero)?;

    require!(
        utilisation <= u128::from(max_utilisation_bps),
        SolfxError::UtilisationCapReached
    );
    Ok(())
}

/// § 7.2: below a fraction of its target, the insurance fund can no longer absorb a gap, so
/// markets stop accepting new risk.
///
/// This is the control that makes the § 6.9 waterfall honest. Without it the protocol would
/// keep opening positions it has no reserve behind, and the first gap would go straight past
/// insurance to ADL and then to LPs — the two steps the fund exists to prevent reaching.
fn check_insurance_floor(balance: u64, target: u64, min_ratio_bps: u16) -> Result<()> {
    if min_ratio_bps == 0 || target == 0 {
        return Ok(());
    }
    let ratio = u128::from(balance)
        .checked_mul(10_000)
        .ok_or(SolfxError::MathOverflow)?
        .checked_div(u128::from(target))
        .ok_or(SolfxError::DivideByZero)?;

    require!(
        ratio >= u128::from(min_ratio_bps),
        SolfxError::InsuranceFundDepleted
    );
    Ok(())
}
