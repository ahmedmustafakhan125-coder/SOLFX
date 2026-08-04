use anchor_lang::prelude::*;

use crate::constants::{MARKET_SEED, MAX_SYMBOL_LEN, PROTOCOL_SEED};
use crate::errors::SolfxError;
use crate::events::MarketInitialized;
use crate::oracle::require_feed_id_set;
use crate::state::{FeedKind, Market, MarketStatus, PriceSource, Protocol, QuoteConversionKind};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct InitializeMarketParams {
    pub symbol: String,
    pub feed_kind: FeedKind,
    pub price_source: PriceSource,
    pub pyth_feed_id: [u8; 32],
    pub secondary_feed_id: [u8; 32],
    pub quote_conversion_feed: [u8; 32],
    pub quote_conversion_kind: QuoteConversionKind,

    pub session_open_dow: u8,
    pub session_open_seconds: u32,
    pub session_close_dow: u8,
    pub session_close_seconds: u32,

    pub weekend_max_leverage: u16,
    pub weekend_oi_cap_bps: u16,
    pub weekend_spread_bps: u16,
    pub weekend_max_conf_bps: u16,

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
    /// Must be >= `max_conf_bps`. See `Market::liquidation_max_conf_bps`.
    pub liquidation_max_conf_bps: u16,
    pub max_deviation_bps: u16,

    pub base_spread_bps: u16,
    pub conf_spread_multiplier_bps: u16,
    pub skew_impact_bps_per_unit: u32,

    /// At `RATE_PRECISION` = 1e9. One basis point is `100_000`.
    pub open_fee_rate: u64,
    pub close_fee_rate: u64,
    pub funding_rate_cap_per_hour: i64,
    pub carry_rate_per_hour: i64,
}

/// List a market. **No redeploy, no downtime, no migration** (`ARCHITECTURE.md` § 5.6).
///
/// The program is the formula; markets are rows. This instruction writes a row: one
/// transaction, roughly a second, a few thousandths of a SOL in rent, and existing positions
/// are untouched. Phase 9 proves the property empirically by listing a market on a running
/// devnet deployment and confirming the program binary hash is unchanged (§ 12.5).
///
/// The market is created `Initialized`, which is **not tradeable**. Activating it is a
/// separate `set_market_status` call, so a mis-typed feed id or risk parameter cannot be
/// traded against in the window between creation and review.
#[derive(Accounts)]
#[instruction(market_index: u16)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
        has_one = admin @ SolfxError::NotAdmin,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        init,
        payer = admin,
        space = 8 + Market::INIT_SPACE,
        seeds = [MARKET_SEED, &market_index.to_le_bytes()],
        bump,
    )]
    pub market: Box<Account<'info, Market>>,

    pub system_program: Program<'info, System>,
}

pub fn init_market(
    ctx: Context<InitializeMarket>,
    market_index: u16,
    params: InitializeMarketParams,
) -> Result<()> {
    let protocol = &mut ctx.accounts.protocol;

    // Indices are assigned sequentially. Allowing an arbitrary index would let two markets
    // be created that both believe they are canonical for a symbol, and would leave holes
    // that an indexer walking `0..num_markets` would silently skip.
    require!(
        market_index == protocol.num_markets,
        SolfxError::InvalidMarketIndex
    );

    let symbol_bytes = params.symbol.as_bytes();
    require!(
        !symbol_bytes.is_empty() && symbol_bytes.len() <= MAX_SYMBOL_LEN,
        SolfxError::InvalidSymbol
    );

    // A symbol appearing in Pyth's catalogue is not evidence that a feed exists: Phase 0b
    // found eight EM symbols listed with `publish_time == 0`, never having published a
    // price. This only catches the all-zero id; the live-data check belongs to the Phase 9
    // listing checklist and cannot be done on chain.
    require_feed_id_set(&params.pyth_feed_id)?;

    // `ContinuousIndex` has no reachable feed. Phase 0b measured all 137 Pyth Metal and
    // Commodities feeds frozen at the Friday close, and no metals index is catalogued under
    // any asset type. The variant is kept so the regime can be switched by config if Q1b
    // resolves favourably — but until it does, a market must not be listed against a
    // 24/7 promise the feed does not keep.
    require!(
        params.feed_kind != FeedKind::ContinuousIndex,
        SolfxError::ContinuousIndexUnavailable
    );

    if matches!(params.price_source, PriceSource::Synthetic { .. }) {
        require_feed_id_set(&params.secondary_feed_id)?;
    } else {
        require!(
            params.secondary_feed_id == [0u8; 32],
            SolfxError::InvalidFeedId
        );
    }

    if params.quote_conversion_kind == QuoteConversionKind::None {
        require!(
            params.quote_conversion_feed == [0u8; 32],
            SolfxError::InvalidFeedId
        );
    } else {
        require_feed_id_set(&params.quote_conversion_feed)?;
    }

    let clock = Clock::get()?;
    let market = &mut ctx.accounts.market;

    market.market_index = market_index;
    let mut symbol = [0u8; MAX_SYMBOL_LEN];
    symbol
        .get_mut(..symbol_bytes.len())
        .ok_or(SolfxError::InvalidSymbol)?
        .copy_from_slice(symbol_bytes);
    market.symbol = symbol;
    market.status = MarketStatus::Initialized;

    market.feed_kind = params.feed_kind;
    market.price_source = params.price_source;
    market.pyth_feed_id = params.pyth_feed_id;
    market.secondary_feed_id = params.secondary_feed_id;
    market.quote_conversion_feed = params.quote_conversion_feed;
    market.quote_conversion_kind = params.quote_conversion_kind;

    market.session_open_dow = params.session_open_dow;
    market.session_open_seconds = params.session_open_seconds;
    market.session_close_dow = params.session_close_dow;
    market.session_close_seconds = params.session_close_seconds;

    market.weekend_max_leverage = params.weekend_max_leverage;
    market.weekend_oi_cap_bps = params.weekend_oi_cap_bps;
    market.weekend_spread_bps = params.weekend_spread_bps;
    market.weekend_max_conf_bps = params.weekend_max_conf_bps;

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

    market.base_spread_bps = params.base_spread_bps;
    market.conf_spread_multiplier_bps = params.conf_spread_multiplier_bps;
    market.skew_impact_bps_per_unit = params.skew_impact_bps_per_unit;

    market.open_fee_rate = params.open_fee_rate;
    market.close_fee_rate = params.close_fee_rate;

    market.cum_funding_long = 0;
    market.cum_funding_short = 0;
    market.cum_borrow_index = 0;
    market.last_funding_update_ts = clock.unix_timestamp;
    market.funding_rate_cap_per_hour = params.funding_rate_cap_per_hour;
    market.carry_rate_per_hour = params.carry_rate_per_hour;

    market.oi_long = 0;
    market.oi_short = 0;
    market.base_oi_long = 0;
    market.base_oi_short = 0;
    market.last_price = 0;
    market.ema_price = 0;
    market.last_price_update_ts = 0;
    market.total_fees_collected = 0;

    market.bump = ctx.bumps.market;

    market.validate_risk_params()?;
    market.validate_session()?;

    protocol.num_markets = protocol
        .num_markets
        .checked_add(1)
        .ok_or(SolfxError::TooManyMarkets)?;

    emit!(MarketInitialized {
        market_index,
        symbol: params.symbol,
        feed_kind: params.feed_kind as u8,
        pyth_feed_id: params.pyth_feed_id,
        is_synthetic: market.is_synthetic(),
        needs_quote_conversion: market.needs_quote_conversion(),
        max_leverage: market.max_leverage,
        ts: clock.unix_timestamp,
    });

    Ok(())
}
