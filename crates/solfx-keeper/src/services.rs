//! The three keepers.
//!
//! Each is a loop over a shared [`Book`]. They are separate services rather than one loop
//! because their failure modes are not alike: a liquidator that stops is an emergency, a
//! trigger executor that stops annoys traders, and a funding crank that stops for a minute
//! costs nothing at all. Splitting them lets an operator alert on the first without being
//! woken by the third.

use std::sync::Arc;

use anchor_lang::{InstructionData, ToAccountMetas};
use anyhow::Result;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use tokio::sync::RwLock;

use solfx_core::state::{MarketStatus, TriggerKind};

use crate::book::{trigger_pda, Book, Vaults};
use crate::chain::{is_benign_race, Chain};
use crate::config::Config;
use crate::danger;

/// How many actions one pass may submit.
///
/// A cap, not a target. Without it a cascade turns into an unbounded burst of transactions
/// from one signer, which the RPC rate-limits and the leader drops — so the keeper ends up
/// landing *fewer* liquidations than if it had paced itself. The next pass is 400ms away.
const MAX_ACTIONS_PER_PASS: usize = 8;

pub struct Shared {
    pub chain: Chain,
    pub cfg: Config,
    pub book: RwLock<Book>,
    pub vaults: Vaults,
}

/// Refuse to act on a book that has gone stale.
///
/// A keeper whose RPC has died still has a plausible-looking snapshot in memory, and will
/// happily keep deciding from it. Going quiet is the correct behaviour: the protocol is
/// permissionless, so someone else's keeper is still running, whereas transactions built from
/// a ten-minute-old book are wrong in a way that costs real money.
pub(crate) fn book_is_fresh(shared: &Shared, book: &Book) -> bool {
    let age = book.age();
    if age.as_secs() > shared.cfg.max_book_age_secs {
        tracing::error!(
            age_secs = age.as_secs(),
            "book is stale; standing down this pass"
        );
        return false;
    }
    true
}

// --- the refresher --------------------------------------------------------------------------

/// Owns the book. Everything else reads it.
pub async fn run_refresher(shared: Arc<Shared>) {
    let mut slow = tokio::time::interval(shared.cfg.refresh_interval());
    let mut fast = tokio::time::interval(shared.cfg.scan_interval());
    slow.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    fast.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = slow.tick() => {
                let mut book = shared.book.write().await;
                if let Err(e) = book.refresh(&shared.chain).await {
                    tracing::error!(error = %format!("{e:#}"), "full refresh failed");
                } else {
                    tracing::info!(
                        markets = book.markets.len(),
                        positions = book.positions.len(),
                        triggers = book.triggers.len(),
                        "book refreshed"
                    );
                }
            }
            _ = fast.tick() => {
                let mut book = shared.book.write().await;
                if let Err(e) = book.refresh_prices(&shared.chain).await {
                    tracing::warn!(error = %format!("{e:#}"), "price refresh failed");
                }
            }
        }
    }
}

// --- liquidator -----------------------------------------------------------------------------

pub async fn run_liquidator(shared: Arc<Shared>) {
    let Some(reward) = shared.cfg.reward_token_account else {
        tracing::error!(
            "liquidator needs --reward-token-account (a USDC account it owns); not starting"
        );
        return;
    };

    let mut tick = tokio::time::interval(shared.cfg.scan_interval());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if let Err(e) = liquidation_pass(&shared, reward).await {
            tracing::error!(error = %format!("{e:#}"), "liquidation pass failed");
        }
    }
}

async fn liquidation_pass(shared: &Arc<Shared>, reward: Pubkey) -> Result<()> {
    let candidates = {
        let book = shared.book.read().await;
        if !book_is_fresh(shared, &book) {
            return Ok(());
        }
        danger::assess(
            &book,
            shared.cfg.watch_health_factor_bps,
            shared.cfg.trace_eval,
        )
    };

    let mut fired = 0usize;
    for candidate in &candidates {
        if !candidate.liquidatable {
            // The near-miss band. Logged so an operator can see pressure building rather than
            // only the liquidations that resulted.
            tracing::debug!(
                position = %candidate.position,
                hf_bps = candidate.health_factor_bps,
                notional = candidate.health.notional,
                "near miss"
            );
            continue;
        }
        if fired >= MAX_ACTIONS_PER_PASS {
            tracing::warn!(
                remaining = candidates.len().saturating_sub(fired),
                "action cap reached; continuing next pass"
            );
            break;
        }

        let ix = {
            let book = shared.book.read().await;
            build_liquidate(shared, &book, candidate, reward)
        };
        let Some(ix) = ix else { continue };

        tracing::info!(
            position = %candidate.position,
            notional = candidate.health.notional,
            equity = candidate.health.liquidation_equity,
            mm = candidate.health.maintenance_margin,
            "liquidating"
        );
        match shared.chain.send(ix, "liquidate_position").await {
            Ok(Some(sig)) => tracing::info!(%sig, position = %candidate.position, "liquidated"),
            Ok(None) => {}
            Err(e) if is_benign_race(&e) => {
                tracing::debug!(position = %candidate.position, "lost the race, or no longer liquidatable");
            }
            Err(e) => {
                tracing::error!(position = %candidate.position, error = %format!("{e:#}"), "liquidation failed");
            }
        }
        fired = fired.saturating_add(1);
    }
    Ok(())
}

fn build_liquidate(
    shared: &Shared,
    book: &Book,
    candidate: &danger::Candidate,
    reward: Pubkey,
) -> Option<Instruction> {
    let feeds = book.feeds.get(&candidate.market_index)?;
    let authority = book.authorities.get(&candidate.user_account)?;
    let v = shared.vaults;

    let accounts = solfx_core::accounts::LiquidatePosition {
        liquidator: shared.chain.pubkey(),
        protocol: v.protocol,
        user_account: candidate.user_account,
        market: crate::book::market_pda(candidate.market_index),
        position: candidate.position,
        rent_destination: *authority,
        liquidator_token_account: reward,
        collateral_vault: v.collateral_vault,
        lp_pool: v.lp_pool,
        lp_vault: v.lp_vault,
        insurance_fund: v.insurance_fund,
        insurance_vault: v.insurance_vault,
        fee_vault: v.fee_vault,
        price_update: feeds.primary,
        secondary_price_update: feeds.secondary,
        quote_conversion_price_update: feeds.quote_conversion,
        token_program: anchor_spl_token_id(),
    };
    Some(Instruction {
        program_id: solfx_core::ID,
        accounts: accounts.to_account_metas(None),
        data: solfx_core::instruction::LiquidatePosition {}.data(),
    })
}

// --- trigger executor -----------------------------------------------------------------------

pub async fn run_triggers(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(shared.cfg.scan_interval());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if let Err(e) = trigger_pass(&shared).await {
            tracing::error!(error = %format!("{e:#}"), "trigger pass failed");
        }
    }
}

async fn trigger_pass(shared: &Arc<Shared>) -> Result<()> {
    let ready = {
        let book = shared.book.read().await;
        if !book_is_fresh(shared, &book) {
            return Ok(());
        }
        collect_ready_triggers(shared, &book)
    };

    for (ix, order, kind) in ready.into_iter().take(MAX_ACTIONS_PER_PASS) {
        tracing::info!(%order, ?kind, "firing trigger");
        match shared.chain.send(ix, "execute_trigger_order").await {
            Ok(Some(sig)) => tracing::info!(%sig, %order, "trigger fired"),
            Ok(None) => {}
            Err(e) if is_benign_race(&e) => {
                tracing::debug!(%order, "lost the race, or no longer met");
            }
            Err(e) => tracing::error!(%order, error = %format!("{e:#}"), "trigger failed"),
        }
    }
    Ok(())
}

fn collect_ready_triggers(shared: &Shared, book: &Book) -> Vec<(Instruction, Pubkey, TriggerKind)> {
    let mut out = Vec::new();
    for (key, order) in &book.triggers {
        let Some(position) = book.positions.get(&order.position) else {
            // The position closed and took the order's reason to exist with it. The order
            // account is still there holding rent; only its owner can reclaim that, so the
            // keeper leaves it alone rather than wasting a transaction on a certain failure.
            continue;
        };
        if position.size_base == 0 {
            continue;
        }
        let Some(market) = book.markets.get(&order.market_index) else {
            continue;
        };
        if !market.status.allows_close() {
            continue;
        }
        // Judged through the **trading** gate: firing a stop is a trade, and it must clear
        // the same bar a trader crossing the same spread would have to clear.
        let Some(price) = book.price_for_trading(order.market_index) else {
            continue;
        };
        if !order
            .kind
            .is_met(position.direction, order.trigger_price, price.spot.price)
        {
            continue;
        }

        let Some(feeds) = book.feeds.get(&order.market_index) else {
            continue;
        };
        let v = shared.vaults;
        let accounts = solfx_core::accounts::ExecuteTriggerOrder {
            keeper: shared.chain.pubkey(),
            protocol: v.protocol,
            user_account: order.user_account,
            market: crate::book::market_pda(order.market_index),
            position: order.position,
            trigger_order: trigger_pda(&order.position, order.order_id),
            collateral_vault: v.collateral_vault,
            lp_pool: v.lp_pool,
            lp_vault: v.lp_vault,
            insurance_fund: v.insurance_fund,
            insurance_vault: v.insurance_vault,
            fee_vault: v.fee_vault,
            price_update: feeds.primary,
            secondary_price_update: feeds.secondary,
            quote_conversion_price_update: feeds.quote_conversion,
            token_program: anchor_spl_token_id(),
        };
        out.push((
            Instruction {
                program_id: solfx_core::ID,
                accounts: accounts.to_account_metas(None),
                data: solfx_core::instruction::ExecuteTriggerOrder {}.data(),
            },
            *key,
            order.kind,
        ));
    }
    out
}

// --- cranks ---------------------------------------------------------------------------------

/// Funding, session and price cranks.
///
/// All three are idempotent and cheap, and all three are *unpaid*. That is a deliberate gap
/// and it is recorded as one: unlike a liquidation or a trigger, nothing compensates whoever
/// calls these, so in practice the operator runs them. It is safe because none of them can be
/// called profitably or harmfully by a stranger — but "the operator must run this" is a
/// weaker promise than "anyone is paid to", and § 7 should not pretend otherwise.
pub async fn run_cranks(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(shared.cfg.crank_interval());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let indices: Vec<u16> = {
            let book = shared.book.read().await;
            if !book_is_fresh(&shared, &book) {
                continue;
            }
            book.markets
                .values()
                // A delisted market has nothing left to crank, and one still merely
                // `Initialized` was never activated.
                .filter(|m| !matches!(m.status, MarketStatus::Initialized | MarketStatus::Delisted))
                .map(|m| m.market_index)
                .collect()
        };

        for index in indices {
            let feeds = {
                let book = shared.book.read().await;
                book.feeds.get(&index).copied()
            };
            let Some(feeds) = feeds else { continue };
            let market = crate::book::market_pda(index);
            let v = shared.vaults;
            let me = shared.chain.pubkey();

            // Session first: it decides whether the market is open, and the other two behave
            // differently on either side of that. Cranking price before session would mark
            // a market with the previous session's parameters.
            let session = Instruction {
                program_id: solfx_core::ID,
                accounts: solfx_core::accounts::CrankMarketSession {
                    keeper: me,
                    protocol: v.protocol,
                    market,
                    price_update: Some(feeds.primary),
                    secondary_price_update: feeds.secondary,
                    quote_conversion_price_update: feeds.quote_conversion,
                }
                .to_account_metas(None),
                data: solfx_core::instruction::CrankMarketSession {}.data(),
            };
            crank(&shared, session, "crank_market_session", index).await;

            // A market whose price the crank cannot even read is refused with `OracleStale` every
            // time. Four listed markets have no feed posted on devnet, and asking anyway cost four
            // failed simulations a minute and a log full of them. Judged by the crank's own read
            // (`observe_market_price`), so a wide or deviating price is still cranked.
            let priceable = {
                let book = shared.book.read().await;
                book.price_for_observe(index).is_some()
            };
            if priceable {
                let price = Instruction {
                    program_id: solfx_core::ID,
                    accounts: solfx_core::accounts::CrankMarketPrice {
                        keeper: me,
                        protocol: v.protocol,
                        market,
                        price_update: feeds.primary,
                        secondary_price_update: feeds.secondary,
                        quote_conversion_price_update: feeds.quote_conversion,
                    }
                    .to_account_metas(None),
                    data: solfx_core::instruction::CrankMarketPrice {}.data(),
                };
                crank(&shared, price, "crank_market_price", index).await;
            }

            let funding = Instruction {
                program_id: solfx_core::ID,
                accounts: solfx_core::accounts::CrankFunding {
                    keeper: me,
                    protocol: v.protocol,
                    market,
                }
                .to_account_metas(None),
                data: solfx_core::instruction::CrankFunding {}.data(),
            };
            crank(&shared, funding, "crank_funding", index).await;
        }
    }
}

/// Cranks fail benignly all the time — "nothing to do" is the common case, since the keeper
/// polls far more often than funding accrues. Only genuine errors are worth an operator's
/// attention.
async fn crank(shared: &Arc<Shared>, ix: Instruction, label: &str, index: u16) {
    match shared.chain.send(ix, label).await {
        Ok(Some(sig)) => tracing::info!(%sig, market = index, crank = label, "cranked"),
        Ok(None) => {}
        Err(e) if is_benign_race(&e) => {
            tracing::trace!(market = index, crank = label, "nothing to do");
        }
        Err(e) => {
            tracing::warn!(market = index, crank = label, error = %format!("{e:#}"), "crank failed");
        }
    }
}

fn anchor_spl_token_id() -> Pubkey {
    solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
}

// --- feed watchdog ----------------------------------------------------------------------------

/// Compare each on-chain price account against Hermes, and complain when they diverge.
///
/// # Why this is a separate concern from liquidating
///
/// § 7.2 and correction C-1 make the protocol *refuse to trade* on a stale or wide feed. That
/// is the right behaviour and it is already enforced on chain — but from the outside it is
/// indistinguishable from the protocol being broken. Traders see rejected orders; the
/// operator sees no errors at all, because nothing errored.
///
/// This loop closes that gap. It has no authority and sends no transactions; it exists so the
/// operator learns that a sponsored publisher has stopped, or that the account has drifted
/// from the real market, *before* the support tickets arrive. Hermes is the reference because
/// it is the same data the on-chain account is supposed to be carrying — a difference between
/// them is a delivery problem, not a market move.
pub async fn run_watchdog(shared: Arc<Shared>) {
    let hermes = match crate::pyth::Hermes::new(
        &shared.cfg.hermes_url,
        shared.cfg.hermes_token.as_deref(),
    ) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "watchdog not starting");
            return;
        }
    };

    let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;

        // (feed id, on-chain price message, market index) for every leg of every market.
        let legs: Vec<([u8; 32], i64, i64, i32, u16)> = {
            let book = shared.book.read().await;
            let mut legs = Vec::new();
            for market in book.markets.values() {
                // Only markets that can actually be traded. Two reasons, and the second is
                // the one that bites: a halted market's price drifting harms nobody because
                // no order can reach it, and Hermes entitles API keys *per feed* and refuses
                // an entire batch if any single feed in it is unentitled — so one dead feed
                // belonging to a halted market blinds the watchdog for every live one.
                if market.status != MarketStatus::Active {
                    continue;
                }
                let ids = [
                    Some(market.pyth_feed_id),
                    crate::pyth::is_set(&market.secondary_feed_id)
                        .then_some(market.secondary_feed_id),
                    crate::pyth::is_set(&market.quote_conversion_feed)
                        .then_some(market.quote_conversion_feed),
                ];
                for id in ids.into_iter().flatten() {
                    let Some(update) = book.prices.get(&book.price_account_for(&id)) else {
                        continue;
                    };
                    legs.push((
                        id,
                        update.price_message.publish_time,
                        update.price_message.price,
                        update.price_message.exponent,
                        market.market_index,
                    ));
                }
            }
            legs
        };
        if legs.is_empty() {
            continue;
        }

        let ids: Vec<[u8; 32]> = legs.iter().map(|leg| leg.0).collect();
        let latest = match hermes.latest(&ids).await {
            Ok(l) => l,
            Err(e) => {
                // Losing the Hermes reference is a monitoring outage, not a protocol one. Say
                // so at warn, not error: escalating it would page someone about a dashboard.
                // The message is the error's own, because the two causes need different
                // responses — an unreachable endpoint is transient, a 403 means the API key is
                // not entitled to those feeds and no amount of waiting fixes it.
                tracing::warn!(error = %format!("{e:#}"), "watchdog reference unavailable");
                continue;
            }
        };

        // The protocol's own gate, not the keeper's book tolerance. This used to be
        // `max_book_age_secs` (45), so a price 46 seconds old logged "the protocol will refuse to
        // trade this market" at error level while the program, whose limit is 60, accepted it.
        // Measured on devnet on 2026-09-24: the poster's normal cycle peaks at 44–48 seconds, so
        // the false alarm fired on BTC/USD every few passes and taught the log to be ignored.
        let tolerance = i64::from(solfx_core::constants::MAX_ALLOWED_STALENESS_SECONDS);
        for (id, on_chain_ts, on_chain_price, on_chain_expo, market_index) in legs {
            let feed = crate::pyth::feed_hex(&id);
            let Some(off_chain) = latest.get(&feed) else {
                tracing::warn!(%feed, "hermes does not know this feed");
                continue;
            };

            let lag = off_chain.publish_time.saturating_sub(on_chain_ts);
            if lag > tolerance {
                tracing::error!(
                    market = market_index,
                    %feed,
                    lag_secs = lag,
                    "on-chain price account is behind the market — the protocol will correctly \
                     refuse to trade this market until it catches up"
                );
                continue;
            }

            // A feed can advance its timestamp while publishing a price that has drifted from
            // the market, which no staleness check catches and which the protocol has no way
            // to detect on its own: a single oracle cannot tell that it is the one that is
            // wrong. Comparing against Hermes is the only outside opinion available.
            match divergence_bps(on_chain_price, on_chain_expo, off_chain) {
                Some(bps) if bps > DIVERGENCE_ALARM_BPS => tracing::error!(
                    market = market_index,
                    %feed,
                    divergence_bps = bps,
                    conf_bps = off_chain.conf_bps(),
                    "on-chain price disagrees with Hermes"
                ),
                Some(bps) => tracing::debug!(
                    market = market_index,
                    lag_secs = lag,
                    divergence_bps = bps,
                    "feed healthy"
                ),
                None => {
                    tracing::warn!(market = market_index, %feed, "cannot compare feed to hermes")
                }
            }
        }
    }
}

/// How far an on-chain price may drift from Hermes before it is worth waking someone.
///
/// Generous on purpose. The two are sampled at different instants, so a fast market produces
/// a real difference that is nobody's fault; alarming on that would make the alarm worthless.
/// 50bps on an FX major is far outside anything sampling jitter explains.
const DIVERGENCE_ALARM_BPS: u64 = 50;

/// Absolute difference between an on-chain price and Hermes', in basis points of the latter.
///
/// Both sides carry their own exponent, so they are brought to a common scale before being
/// compared rather than assumed to match — a feed whose exponent changes would otherwise read
/// as a catastrophic divergence when nothing had moved at all.
fn divergence_bps(
    on_chain: i64,
    on_chain_expo: i32,
    off_chain: &crate::pyth::OffChainPrice,
) -> Option<u64> {
    let scale = |value: i64, expo: i32, target: i32| -> Option<i128> {
        let shift = u32::try_from(expo.checked_sub(target)?).ok()?;
        i128::from(value).checked_mul(10_i128.checked_pow(shift)?)
    };
    // Compare at the finer of the two exponents so neither side loses precision.
    let target = on_chain_expo.min(off_chain.expo);
    let a = scale(on_chain, on_chain_expo, target)?;
    let b = scale(off_chain.price, off_chain.expo, target)?;
    if b == 0 {
        return None;
    }
    let diff = a.checked_sub(b)?.unsigned_abs();
    u64::try_from(diff.checked_mul(10_000)?.checked_div(b.unsigned_abs())?).ok()
}

#[cfg(test)]
// The workspace denies `unwrap` because a panic in a keeper is an outage. In a test a panic
// is the reporting mechanism.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::pyth::OffChainPrice;

    fn hermes(price: i64, expo: i32) -> OffChainPrice {
        OffChainPrice {
            price,
            conf: 0,
            expo,
            publish_time: 0,
        }
    }

    #[test]
    fn identical_prices_do_not_diverge() {
        assert_eq!(divergence_bps(108_543, -5, &hermes(108_543, -5)), Some(0));
    }

    /// The case this function exists for: the two sides carry their own exponents, and a feed
    /// whose exponent changed would read as a total divergence if they were compared raw.
    #[test]
    fn a_different_exponent_is_not_a_divergence() {
        // 1.08543 written at 1e-5 and at 1e-8 is the same number.
        assert_eq!(
            divergence_bps(108_543, -5, &hermes(108_543_000, -8)),
            Some(0)
        );
    }

    #[test]
    fn a_one_percent_gap_reads_as_a_hundred_bps() {
        // 1.09628 against 1.08543 is +0.99933%, which floors to 99bps.
        assert_eq!(divergence_bps(109_628, -5, &hermes(108_543, -5)), Some(99));
    }

    /// Direction must not matter: a feed reading low is exactly as broken as one reading high.
    #[test]
    fn divergence_is_absolute() {
        let high = divergence_bps(109_628, -5, &hermes(108_543, -5));
        let low = divergence_bps(107_458, -5, &hermes(108_543, -5));
        assert_eq!(high, Some(99));
        assert_eq!(low, Some(99));
    }

    #[test]
    fn a_zero_reference_is_not_comparable_rather_than_infinite() {
        assert_eq!(divergence_bps(108_543, -5, &hermes(0, -5)), None);
    }

    /// A real divergence has to clear the alarm threshold, and normal sampling jitter must
    /// not. 10bps between two samples of a fast FX major is unremarkable; 100bps is not.
    #[test]
    fn the_alarm_threshold_separates_jitter_from_a_broken_feed() {
        let jitter = divergence_bps(108_652, -5, &hermes(108_543, -5)).unwrap();
        let broken = divergence_bps(109_628, -5, &hermes(108_543, -5)).unwrap();
        assert!(jitter <= DIVERGENCE_ALARM_BPS, "10bps must not alarm");
        assert!(broken > DIVERGENCE_ALARM_BPS, "100bps must alarm");
    }
}
