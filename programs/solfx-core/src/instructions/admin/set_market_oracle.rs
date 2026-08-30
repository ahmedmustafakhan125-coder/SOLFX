use anchor_lang::prelude::*;

use crate::errors::SolfxError;
use crate::events::MarketOracleChanged;
use crate::instructions::admin::update_market::AdminMarket;
use crate::oracle::require_feed_id_set;
use crate::state::MarketStatus;

/// Repoint a market at a different Pyth feed.
///
/// `pyth_feed_id` was write-once until now: `initialize_market` set it and nothing could
/// correct it. That is a mainnet-blocking gap rather than a devnet convenience, because Pyth
/// retires feeds — `InterestRate.10YZ6` and `Commodities.CAN6/USD` are both gone from the
/// catalogue. Under the old shape, a market whose feed was retired was permanently dead: it
/// could not be priced, so positions on it could not be closed or liquidated either.
///
/// The four guards below are ordered cheapest-to-most-structural, and each one closes a
/// distinct way this instruction could be misused.
pub fn set_market_oracle(
    ctx: Context<AdminMarket>,
    expected_current: [u8; 32],
    new_feed_id: [u8; 32],
) -> Result<()> {
    let market = &mut ctx.accounts.market;
    let old_feed_id = market.pyth_feed_id;

    // 1. Compare-and-swap on the stored feed, mirroring Drift's
    //    `handle_update_spot_market_oracle`. The caller must prove it read the market before
    //    changing it, so a stale deployment script or a copy-pasted command cannot repoint a
    //    market it has an outdated picture of. This is the guard that makes the instruction
    //    safe to expose to an operator tool at all.
    require!(old_feed_id == expected_current, SolfxError::WrongOracleFeed);

    // 2. An all-zero id is what an uninitialised `[u8; 32]` looks like. Reusing
    //    `initialize_market`'s own check keeps the two listing paths from diverging.
    require_feed_id_set(&new_feed_id)?;

    // 3. A no-op write must fail loudly. Emitting `MarketOracleChanged` with two identical
    //    ids would tell an indexer a repoint happened when nothing did.
    require!(new_feed_id != old_feed_id, SolfxError::InvalidParameter);

    // 4. The load-bearing one. Repointing under an open book re-prices every live position
    //    against an instrument it was never opened on: entry prices, unrealised PnL and
    //    liquidation thresholds were all computed from the old feed, and the next oracle read
    //    would compare them against an unrelated quote. Halt first, repoint, re-activate —
    //    the same deliberate two-step as listing a market `Initialized` rather than `Active`.
    require!(
        market.status != MarketStatus::Active,
        SolfxError::MarketNotActive
    );

    market.pyth_feed_id = new_feed_id;

    emit!(MarketOracleChanged {
        market_index: market.market_index,
        old_feed_id,
        new_feed_id,
        ts: Clock::get()?.unix_timestamp,
    });
    Ok(())
}
