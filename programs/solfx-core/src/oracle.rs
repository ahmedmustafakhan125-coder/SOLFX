//! The single validated read path from Pyth to a price the engine will trade on
//! (`ARCHITECTURE.md` ADR-006, § 7.1).
//!
//! # One choke point
//!
//! No instruction reads a price any other way. One place to audit, one place to fix, and no
//! instruction can accidentally skip a check. Every gate below is enforced here or nowhere.
//!
//! # The gates, in order
//!
//! 1. **Account ownership.** `Account<'info, PriceUpdateV2>` will only deserialise an
//!    account owned by the Pyth receiver program. Enforced by Anchor, not by us.
//! 2. **Feed identity.** The update's `feed_id` must equal the one on `Market`. An attacker
//!    who posts a real, fresh, fully-verified SOL/USD update cannot use it to trade EUR/USD.
//! 3. **Verification level.** `Full` only. `Partial` lowers the number of Wormhole guardians
//!    that must collude to forge a price, which is not a trade worth making on a leveraged
//!    venue.
//! 4. **Freshness, both ends.** Not older than `max_staleness_seconds`, and not dated
//!    further into the future than clock drift explains.
//! 5. **Confidence.** `conf / price` within the market's ceiling (correction C-4).
//! 6. **Deviation.** Spot within `max_deviation_bps` of Pyth's own EMA.
//!
//! # Why `get_price_unchecked` rather than `get_price_no_older_than`
//!
//! The SDK helper bundles the feed-id check, the verification-level check and a *one-sided*
//! staleness check. We need a two-sided freshness window (see below), so the staleness half
//! would be duplicated anyway — and the helper reaches it through
//! `maximum_age.try_into().unwrap()`, a panic path in a dependency. Calling the unchecked
//! accessor and performing all six gates here means every check is visible in one function
//! and none of them is a dependency's implementation detail.
//!
//! # Why freshness is checked at both ends
//!
//! `get_price_no_older_than` asks only `publish_time + max_age >= now`. A price stamped an
//! hour ahead of the cluster clock satisfies that trivially, and keeps satisfying it for the
//! next hour no matter what the market does — the C-1 stale-price exploit reached through a
//! different door. `solfx_math::oracle::validate_publish_time` closes both ends.
//!
//! # Why the deviation reference is Pyth's EMA and not a stored one
//!
//! § 7.1 compares spot against `market.ema_price`, maintained on chain. That creates a
//! deadlock: refreshing the stored reference requires reading a price, reading a price
//! requires passing the deviation gate, and after a large genuine move nothing passes — the
//! market wedges shut exactly when it most needs to reprice.
//!
//! `PriceFeedMessage` already carries `ema_price` at the same exponent as `price`, computed
//! by Pyth. Using it removes the deadlock, removes a cranking requirement, and removes the
//! rounding drift a self-maintained integer EMA accumulates. `Market::ema_price` is still
//! stored, but as an *observation* for the frontend rather than as the gate's input.

use anchor_lang::prelude::*;
use pyth_solana_receiver_sdk::price_update::{PriceUpdateV2, VerificationLevel};
use solfx_math::oracle::{
    self, compose_synthetic, deviation_bps, normalize_conf, normalize_price, validate_deviation,
    validate_publish_time, ValidatedPrice,
};

use crate::constants::MAX_FUTURE_DRIFT_SECONDS;
use crate::errors::{IntoProgramResult, SolfxError};
use crate::state::{Market, PriceSource};

/// One feed's contribution, after normalisation and freshness checks but before the
/// confidence ceiling is applied.
///
/// Confidence is deliberately *not* gated per leg. For a synthetic market the composed band
/// is the linear sum of the legs, so it is never narrower than the widest leg — gating the
/// composed value alone is both sufficient and the correct place to apply a market's
/// ceiling, since the ceiling describes the market, not its inputs.
struct RawLeg {
    price: i64,
    conf: u64,
    ema: i64,
    publish_time: i64,
    deviation_bps: u64,
}

/// A fully validated price for a market, plus everything the caller needs to record or
/// react to it.
pub struct MarketPrice {
    /// Spot, at `PRICE_PRECISION`. Composed already, if the market is synthetic.
    pub spot: ValidatedPrice,
    /// Pyth's EMA for the primary leg, at `PRICE_PRECISION`. Stored for display.
    pub reference_ema: i64,
    /// Spot's distance from `reference_ema`, in bps. Measured whether or not it was gated,
    /// so a caller can trip a breaker on it (§ 7.2).
    pub deviation_bps: u64,
    /// Quote-currency conversion rate at `PRICE_PRECISION`, for non-USD-quoted markets
    /// (correction C-3). `None` when the market is USD-quoted.
    pub quote_conversion_rate: Option<i64>,
}

/// Which gates apply to a read.
///
/// Three modes, because "is this price good enough?" has three different right answers
/// depending on what is about to be done with it. Making that an explicit enum rather than a
/// pair of booleans keeps the choke point auditable: there are exactly three ways to read a
/// price, and each is named.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PriceGate {
    /// **Opening or closing voluntarily.** Every gate at its strictest. Trading against a
    /// band you cannot price is how latency arbitrageurs get paid (C-4), and a voluntary
    /// trade can always simply not happen.
    Trading,
    /// **Liquidation and auto-deleveraging.** Staleness enforced; confidence checked against
    /// the market's *liquidation* ceiling; deviation not checked at all.
    ///
    /// Both relaxations are deliberate, and both come from the same observation: **the
    /// conditions that make a position liquidatable are the conditions that widen the band
    /// and dislocate the price.** The CHF-depeg replay measured a confidence two orders of
    /// magnitude above normal and a 30% deviation from the EMA. Under `Trading` gates, that
    /// price is unusable — which means the position cannot be closed, keeps falling, and
    /// turns a bounded loss into unbounded bad debt.
    ///
    /// Deviation specifically is *not* re-checked because the crank has already halted the
    /// market on it and flagged it for a human. Blocking the liquidation too would leave the
    /// position frozen in exactly the state the halt was called to contain.
    ///
    /// Staleness stays strict. A stale price is never acceptable for anything: it is the
    /// C-1 exploit, and it is the one gate no argument justifies relaxing.
    Liquidation,
    /// **Observation.** Staleness only. Confidence and deviation are *measured* and returned
    /// for the caller to act on rather than enforced.
    ///
    /// This is what the cranker uses, and the distinction is load-bearing: a price too
    /// uncertain to trade on must still be *recordable*, because the crank is what trips the
    /// breaker. Gating it would mean a market that never halts, holding a last-known price
    /// from before the dislocation — the failure mode looking exactly like the exploit.
    Observe,
}

/// Read and fully validate a market's price. Every gate at its strictest.
///
/// This is what trading instructions call.
pub fn load_validated_price(
    market: &Market,
    primary: &PriceUpdateV2,
    secondary: Option<&PriceUpdateV2>,
    quote_conversion: Option<&PriceUpdateV2>,
    clock: &Clock,
) -> Result<MarketPrice> {
    read(
        market,
        primary,
        secondary,
        quote_conversion,
        clock,
        PriceGate::Trading,
    )
}

/// Read for liquidation or auto-deleveraging. See [`PriceGate::Liquidation`].
pub fn load_price_for_liquidation(
    market: &Market,
    primary: &PriceUpdateV2,
    secondary: Option<&PriceUpdateV2>,
    quote_conversion: Option<&PriceUpdateV2>,
    clock: &Clock,
) -> Result<MarketPrice> {
    read(
        market,
        primary,
        secondary,
        quote_conversion,
        clock,
        PriceGate::Liquidation,
    )
}

/// Read without enforcing confidence or deviation, returning both for the caller to judge.
/// See [`PriceGate::Observe`].
pub fn observe_market_price(
    market: &Market,
    primary: &PriceUpdateV2,
    secondary: Option<&PriceUpdateV2>,
    quote_conversion: Option<&PriceUpdateV2>,
    clock: &Clock,
) -> Result<MarketPrice> {
    read(
        market,
        primary,
        secondary,
        quote_conversion,
        clock,
        PriceGate::Observe,
    )
}

fn read(
    market: &Market,
    primary: &PriceUpdateV2,
    secondary: Option<&PriceUpdateV2>,
    quote_conversion: Option<&PriceUpdateV2>,
    clock: &Clock,
    gate: PriceGate,
) -> Result<MarketPrice> {
    let base = read_leg(primary, &market.pyth_feed_id, market, clock, gate)?;

    // The confidence ceiling this read is judged against.
    let conf_ceiling = match gate {
        PriceGate::Trading => market.effective_max_conf_bps(),
        PriceGate::Liquidation => market.liquidation_conf_ceiling(),
        PriceGate::Observe => u16::MAX,
    };

    let spot = match market.price_source {
        PriceSource::Direct => {
            // A caller that supplies a secondary update for a direct market has either
            // mis-built the transaction or is probing for a path that ignores it. Reject
            // rather than silently discard: an ignored account is an unaudited one.
            require!(secondary.is_none(), SolfxError::UnexpectedPriceUpdate);
            ValidatedPrice::new(base.price, base.conf, base.publish_time, conf_ceiling)
                .or_program_err()?
        }
        PriceSource::Synthetic { invert_quote } => {
            let quote_update = secondary.ok_or(SolfxError::MissingSecondaryPriceUpdate)?;
            let quote_leg = read_leg(quote_update, &market.secondary_feed_id, market, clock, gate)?;

            // Legs enter composition with the ceiling deferred; the composed band carries it.
            let base_vp = ValidatedPrice::new(base.price, base.conf, base.publish_time, u16::MAX)
                .or_program_err()?;
            let quote_vp = ValidatedPrice::new(
                quote_leg.price,
                quote_leg.conf,
                quote_leg.publish_time,
                u16::MAX,
            )
            .or_program_err()?;

            compose_synthetic(base_vp, quote_vp, invert_quote, conf_ceiling).or_program_err()?
        }
    };

    let quote_conversion_rate = if market.needs_quote_conversion() {
        let update = quote_conversion.ok_or(SolfxError::MissingQuoteConversionPriceUpdate)?;
        let leg = read_leg(update, &market.quote_conversion_feed, market, clock, gate)?;
        Some(leg.price)
    } else {
        require!(
            quote_conversion.is_none(),
            SolfxError::UnexpectedPriceUpdate
        );
        None
    };

    Ok(MarketPrice {
        spot,
        reference_ema: base.ema,
        deviation_bps: base.deviation_bps,
        quote_conversion_rate,
    })
}

/// Gates 1–4 and 6 for a single feed. Gate 5 (confidence) is applied by the caller to the
/// composed price — see [`RawLeg`].
fn read_leg(
    update: &PriceUpdateV2,
    expected_feed_id: &[u8; 32],
    market: &Market,
    clock: &Clock,
    gate: PriceGate,
) -> Result<RawLeg> {
    // Gate 3: Full verification only. Checked before anything is read out of the account.
    require!(
        update.verification_level.gte(VerificationLevel::Full),
        SolfxError::WrongOracleFeed
    );

    // Gate 2: feed identity. `get_price_unchecked` compares `price_message.feed_id` against
    // the id we pass and errors on mismatch, so a valid update for the wrong instrument is
    // rejected here rather than silently traded.
    let p = update
        .get_price_unchecked(expected_feed_id)
        .map_err(|_| error!(SolfxError::WrongOracleFeed))?;

    // Gate 4: freshness, both ends.
    validate_publish_time(
        p.publish_time,
        clock.unix_timestamp,
        market.max_staleness_seconds,
        MAX_FUTURE_DRIFT_SECONDS,
    )
    .or_program_err()?;

    let price = normalize_price(p.price, p.exponent).or_program_err()?;
    let conf = normalize_conf(p.conf, p.exponent).or_program_err()?;

    // Gate 6: deviation from Pyth's EMA.
    //
    // `Price` (what the accessor returns) carries only price/conf/exponent/publish_time; the
    // EMA lives on the underlying `PriceFeedMessage` at the same exponent. Reading it
    // directly is safe here and only here, because `get_price_unchecked` above has already
    // proved this message belongs to the feed we asked for — without that check first, this
    // would be reading an arbitrary attacker-supplied number.
    //
    // A non-positive EMA means Pyth has not established one yet. § 7.1 skips the check in
    // that case rather than failing closed: a newly listed feed with no history would
    // otherwise be permanently untradeable.
    let ema = normalize_price(update.price_message.ema_price, p.exponent).unwrap_or(0);
    let deviation = if ema > 0 {
        let d = deviation_bps(price, ema).or_program_err()?;
        if gate == PriceGate::Trading {
            validate_deviation(d, market.max_deviation_bps).or_program_err()?;
        }
        d
    } else {
        0
    };

    Ok(RawLeg {
        price,
        conf,
        ema,
        publish_time: p.publish_time,
        deviation_bps: deviation,
    })
}

/// Reject an all-zero feed id.
///
/// A zeroed id is what an uninitialised `[u8; 32]` looks like, and Phase 0b found eight
/// symbols listed in Pyth's catalogue that have never published a price. A market pointed
/// at either could be created but never priced, opened or liquidated. Catching it at
/// `initialize_market` is the cheap moment; the expensive one is a position nobody can close.
pub fn require_feed_id_set(feed_id: &[u8; 32]) -> Result<()> {
    require!(feed_id != &[0u8; 32], SolfxError::InvalidFeedId);
    Ok(())
}

/// Re-exported so instructions never reach past this module for oracle maths.
pub use oracle::pow10;
