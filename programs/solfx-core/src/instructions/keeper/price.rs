//! Permissionless upkeep.
//!
//! Phase 2 ships one crank. Funding, session and liquidation cranks arrive in Phase 4.

use anchor_lang::prelude::*;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::constants::{MARKET_SEED, PROTOCOL_SEED};
use crate::errors::SolfxError;
use crate::events::{CircuitBreakerTripped, MarketPriceUpdated, MarketStatusChanged};
use crate::oracle::observe_market_price;
use crate::state::{Market, MarketStatus, Protocol};

#[derive(Accounts)]
pub struct CrankMarketPrice<'info> {
    /// Permissionless. Anyone may refresh a market's observed price — there is nothing to
    /// steal and a market whose price only its operator can record is a market with a single
    /// point of failure.
    pub keeper: Signer<'info>,

    #[account(
        seeds = [PROTOCOL_SEED],
        bump = protocol.bump,
    )]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,

    /// Primary Pyth pull-oracle update. Posted by the caller in the same transaction
    /// (correction C-5) — the account is verified and read, never trusted.
    pub price_update: Box<Account<'info, PriceUpdateV2>>,

    /// Quote leg, for synthetic markets. `None` otherwise.
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,

    /// Quote-currency conversion feed, for non-USD-quoted markets (C-3). `None` otherwise.
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
}

/// Read the oracle, record the result, and trip the deviation breaker if it fires.
///
/// # What this instruction is for
///
/// Phase 2's exit criterion is that direct, synthetic, EM and non-USD-quoted feeds all
/// **price correctly**. Proving that requires an on-chain path that reads a real Pyth
/// account through the full validation stack and writes an observable result. This is it.
/// It is also the on-chain counterpart of the Phase 0b probe: a `MarketPriceUpdated` event
/// is a durable record that a listed market was reachable, fresh and inside its confidence
/// band at a given moment.
///
/// # Why it observes rather than gates
///
/// The deviation check is measured here, not enforced. § 7.2 says a deviating market goes
/// to `Halted` with a guardian alert, which is a different response from rejecting a trade —
/// and a crank forced through the gate could never refresh anything after a large genuine
/// move, so the market would wedge shut exactly when it most needed attention. Staleness and
/// confidence *are* enforced, because a stale or uncertain price is not an observation worth
/// recording.
///
/// # Failing closed
///
/// A market that trips the breaker goes to `Halted` and stays there until an admin reviews
/// it. Nothing here can move a market back to `Active`. 24/7 pricing is a property of a
/// feed, never a promise the protocol makes on its behalf.
pub fn crank_market_price(ctx: Context<CrankMarketPrice>) -> Result<()> {
    let clock = Clock::get()?;
    let market = &mut ctx.accounts.market;

    require!(
        market.status != MarketStatus::Delisted,
        SolfxError::MarketNotActive
    );

    let observed = observe_market_price(
        market,
        &ctx.accounts.price_update,
        // Two dereferences, not one: `as_deref` strips the `Box` and leaves
        // `&Account<PriceUpdateV2>`, and the oracle module takes the bare `PriceUpdateV2`
        // so it never has to know it is being handed an Anchor wrapper.
        ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
        ctx.accounts
            .quote_conversion_price_update
            .as_deref()
            .map(|a| &**a),
        &clock,
    )?;

    market.last_price = observed.spot.price;
    market.ema_price = observed.reference_ema;
    market.last_price_update_ts = clock.unix_timestamp;

    emit!(MarketPriceUpdated {
        market_index: market.market_index,
        price: observed.spot.price,
        conf: observed.spot.conf,
        conf_bps: observed.spot.conf_bps,
        ema_price: observed.reference_ema,
        deviation_bps: observed.deviation_bps,
        publish_time: observed.spot.publish_time,
        ts: clock.unix_timestamp,
    });

    // Two breakers, both § 7.2, and both reachable *only* because this read observes rather
    // than gates. A crank that refused a wide or dislocated price could never trip either —
    // the market would stay Active holding a last-known price from before the event, which
    // is the failure mode looking exactly like the exploit it is meant to prevent.
    let deviation_tripped = observed.deviation_bps > u64::from(market.max_deviation_bps);
    let confidence_tripped = observed.spot.conf_bps > u64::from(market.effective_max_conf_bps());

    if (deviation_tripped || confidence_tripped) && market.status != MarketStatus::Halted {
        let old_status = market.status;
        market.status = MarketStatus::Halted;
        market.status_changed_at = clock.unix_timestamp;

        let (reason, observed_value, limit) = if deviation_tripped {
            (
                0u8,
                observed.deviation_bps,
                u64::from(market.max_deviation_bps),
            )
        } else {
            (
                2u8,
                observed.spot.conf_bps,
                u64::from(market.effective_max_conf_bps()),
            )
        };

        emit!(CircuitBreakerTripped {
            market_index: market.market_index,
            reason,
            observed: observed_value,
            limit,
            ts: clock.unix_timestamp,
        });
        emit!(MarketStatusChanged {
            market_index: market.market_index,
            old_status: old_status as u8,
            new_status: MarketStatus::Halted as u8,
            actor: 2, // automatic
            ts: clock.unix_timestamp,
        });
    }

    Ok(())
}
