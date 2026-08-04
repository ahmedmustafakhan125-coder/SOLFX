use anchor_lang::prelude::*;

use crate::constants::{MARKET_SEED, PROTOCOL_SEED};
use crate::errors::SolfxError;
use crate::events::{MarketFeeParamsUpdated, MarketRiskParamsUpdated, MarketStatusChanged};
use crate::state::{Market, MarketStatus, Protocol};

/// Shared account set for every admin mutation of a single market.
#[derive(Accounts)]
pub struct AdminMarket<'info> {
    pub admin: Signer<'info>,

    #[account(
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        has_one = admin @ SolfxError::NotAdmin,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct UpdateRiskParams {
    pub max_leverage: u16,
    pub imr_bps: u16,
    pub mmr_bps: u16,
    pub liquidation_fee_bps: u16,
    pub max_oi_long: u64,
    pub max_oi_short: u64,
    pub max_position_size: u64,
    pub min_position_size: u64,
    pub max_staleness_seconds: u32,
    pub max_conf_bps: u16,
    pub liquidation_max_conf_bps: u16,
    pub max_deviation_bps: u16,
    pub weekend_max_leverage: u16,
    pub weekend_oi_cap_bps: u16,
    pub weekend_max_conf_bps: u16,
}

/// Retune a market's risk envelope without an upgrade.
///
/// Risk parameters are per-market **data**, never constants — that is one of the § 5.6
/// generic-engine constraints, and it is what lets EM leverage sit at 10–20x while majors
/// sit at 50x with the same binary.
///
/// The whole set is revalidated afterwards, so a partial update cannot leave the market in a
/// configuration nobody ever checked (e.g. lowering `max_leverage` below what `imr_bps`
/// implies).
pub fn update_risk_params(ctx: Context<AdminMarket>, params: UpdateRiskParams) -> Result<()> {
    let market = &mut ctx.accounts.market;

    market.max_leverage = params.max_leverage;
    market.imr_bps = params.imr_bps;
    market.mmr_bps = params.mmr_bps;
    market.liquidation_fee_bps = params.liquidation_fee_bps;
    market.max_oi_long = params.max_oi_long;
    market.max_oi_short = params.max_oi_short;
    market.max_position_size = params.max_position_size;
    market.min_position_size = params.min_position_size;
    market.max_staleness_seconds = params.max_staleness_seconds;
    market.max_conf_bps = params.max_conf_bps;
    market.liquidation_max_conf_bps = params.liquidation_max_conf_bps;
    market.max_deviation_bps = params.max_deviation_bps;
    market.weekend_max_leverage = params.weekend_max_leverage;
    market.weekend_oi_cap_bps = params.weekend_oi_cap_bps;
    market.weekend_max_conf_bps = params.weekend_max_conf_bps;

    market.validate_risk_params()?;

    emit!(MarketRiskParamsUpdated {
        market_index: market.market_index,
        max_leverage: market.max_leverage,
        imr_bps: market.imr_bps,
        mmr_bps: market.mmr_bps,
        max_conf_bps: market.max_conf_bps,
        max_deviation_bps: market.max_deviation_bps,
        max_staleness_seconds: market.max_staleness_seconds,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct UpdateFeeParams {
    pub open_fee_rate: u64,
    pub close_fee_rate: u64,
    pub base_spread_bps: u16,
    pub conf_spread_multiplier_bps: u16,
    pub skew_impact_bps_per_unit: u32,
    pub weekend_spread_bps: u16,
    pub funding_rate_cap_per_hour: i64,
    pub carry_rate_per_hour: i64,
}

/// Retune fees, spreads and carry.
///
/// `carry_rate_per_hour` is the field that carries the interest-rate differential plus the
/// protocol markup (§ 6.7). Publishing it on chain, where a trader can read the exact swap
/// rate instead of discovering it after rollover, is a concrete improvement over XM and
/// Exness and one of the few claims in this project that is checkable by anyone.
pub fn update_fee_params(ctx: Context<AdminMarket>, params: UpdateFeeParams) -> Result<()> {
    let market = &mut ctx.accounts.market;

    market.open_fee_rate = params.open_fee_rate;
    market.close_fee_rate = params.close_fee_rate;
    market.base_spread_bps = params.base_spread_bps;
    market.conf_spread_multiplier_bps = params.conf_spread_multiplier_bps;
    market.skew_impact_bps_per_unit = params.skew_impact_bps_per_unit;
    market.weekend_spread_bps = params.weekend_spread_bps;
    market.funding_rate_cap_per_hour = params.funding_rate_cap_per_hour;
    market.carry_rate_per_hour = params.carry_rate_per_hour;

    emit!(MarketFeeParamsUpdated {
        market_index: market.market_index,
        open_fee_rate: market.open_fee_rate,
        close_fee_rate: market.close_fee_rate,
        base_spread_bps: market.base_spread_bps,
        conf_spread_multiplier_bps: market.conf_spread_multiplier_bps,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

/// Admin override of the regime state machine.
///
/// `WeekendMode` and `PreOpenWindow` are rejected: both belong to the continuous regime,
/// which no market can currently occupy because `FeedKind::ContinuousIndex` has no reachable
/// feed. Allowing them by hand would let an operator derate a session-bound market into a
/// state whose transitions Phase 4 never wrote, which is how a market gets stuck.
pub fn set_market_status(ctx: Context<AdminMarket>, new_status: MarketStatus) -> Result<()> {
    let market = &mut ctx.accounts.market;
    let old_status = market.status;

    require!(
        !matches!(
            new_status,
            MarketStatus::WeekendMode | MarketStatus::PreOpenWindow
        ),
        SolfxError::InvalidStatusTransition
    );

    // Delisting is terminal. A market that has been wound down must not silently reopen
    // with stale open interest still recorded against it.
    require!(
        old_status != MarketStatus::Delisted,
        SolfxError::InvalidStatusTransition
    );

    market.status = new_status;

    emit!(MarketStatusChanged {
        market_index: market.market_index,
        old_status: old_status as u8,
        new_status: new_status as u8,
        actor: 0, // admin
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
