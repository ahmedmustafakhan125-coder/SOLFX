//! A local mirror of the protocol's book, and the addresses derived from it.
//!
//! # The property this module exists to preserve
//!
//! The keeper judges a position with [`solfx_core::risk::assess`] and
//! [`solfx_core::oracle::load_price_for_liquidation`] — **the same functions the program
//! runs**, compiled from the same source, not a reimplementation.
//!
//! That is not a convenience. A keeper with its own margin maths is a second, unaudited risk
//! engine, and the two only have to disagree by a rounding step for the keeper to spend all
//! day sending transactions the chain rejects, or — far worse — to sit quietly on a position
//! the chain would have liquidated. Sharing the code makes that class of bug unrepresentable:
//! the keeper's answer differs from the chain's only where its *inputs* differ, which is a
//! staleness question with a bounded, measurable answer rather than a logic question with an
//! unbounded one.
//!
//! What the keeper does *not* share is authority. Every condition is re-checked on chain, so
//! a mirror that has drifted costs one rejected transaction and never a wrong outcome.

use std::collections::HashMap;

use anchor_lang::solana_program::clock::Clock;
use anyhow::{Context as _, Result};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solana_pubkey::Pubkey;

use solfx_core::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, PROTOCOL_SEED, TRIGGER_SEED,
};
use solfx_core::oracle::MarketPrice;
use solfx_core::state::{Market, Position, TriggerOrder, UserAccount};

use crate::chain::Chain;
use crate::pyth;

// --- addresses ------------------------------------------------------------------------------

#[must_use]
pub fn market_pda(index: u16) -> Pubkey {
    Pubkey::find_program_address(&[MARKET_SEED, &index.to_le_bytes()], &solfx_core::ID).0
}

#[must_use]
pub fn trigger_pda(position: &Pubkey, order_id: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[TRIGGER_SEED, position.as_ref(), &[order_id]],
        &solfx_core::ID,
    )
    .0
}

/// The vault and pool addresses every keeper instruction needs. Fixed for the lifetime of the
/// protocol, so they are derived once at startup rather than per transaction.
#[derive(Debug, Clone, Copy)]
pub struct Vaults {
    pub protocol: Pubkey,
    pub collateral_vault: Pubkey,
    pub lp_pool: Pubkey,
    pub lp_vault: Pubkey,
    pub insurance_fund: Pubkey,
    pub insurance_vault: Pubkey,
    pub fee_vault: Pubkey,
}

impl Vaults {
    #[must_use]
    pub fn derive() -> Self {
        let one = |seed: &[u8]| Pubkey::find_program_address(&[seed], &solfx_core::ID).0;
        Self {
            protocol: one(PROTOCOL_SEED),
            collateral_vault: one(COLLATERAL_VAULT_SEED),
            lp_pool: one(LP_POOL_SEED),
            lp_vault: one(LP_VAULT_SEED),
            insurance_fund: one(INSURANCE_FUND_SEED),
            insurance_vault: one(INSURANCE_VAULT_SEED),
            fee_vault: one(FEE_VAULT_SEED),
        }
    }
}

// --- the mirror -----------------------------------------------------------------------------

/// The three price accounts one market may need. Synthetic markets (§ 4.3) need a second leg;
/// non-USD-quoted markets need a conversion rate (correction C-3).
#[derive(Debug, Clone, Copy)]
pub struct FeedSet {
    pub primary: Pubkey,
    pub secondary: Option<Pubkey>,
    pub quote_conversion: Option<Pubkey>,
}

/// The address this cluster keeps `feed_id`'s price at: the map's answer when it has one, the
/// derived sponsored PDA otherwise.
///
/// **Every** lookup of a price account must come through here, and the reason is a bug that
/// has now appeared twice. `Book::prices` is keyed by the *resolved* address, so any code that
/// derives the sponsored PDA independently indexes that map with a key it does not contain,
/// finds nothing, and carries on quietly — the watchdog compared an empty set of legs that way
/// and reported healthy while checking nothing at all.
fn resolve_price_account(overrides: &HashMap<[u8; 32], Pubkey>, feed_id: &[u8; 32]) -> Pubkey {
    overrides
        .get(feed_id)
        .copied()
        .unwrap_or_else(|| pyth::price_account(feed_id))
}

impl FeedSet {
    /// Resolve a market's feeds, consulting `overrides` before deriving.
    ///
    /// The order is the whole point. On a cluster with sponsored feeds the map is empty and
    /// every address derives, exactly as before. On a cluster where `price-poster` publishes
    /// into its own keypair accounts the derived address holds a stale price or none at all,
    /// and only the map knows where the traded price lives — see [`crate::price_map`].
    fn for_market(market: &Market, overrides: &HashMap<[u8; 32], Pubkey>) -> Self {
        Self {
            primary: resolve_price_account(overrides, &market.pyth_feed_id),
            secondary: pyth::is_set(&market.secondary_feed_id)
                .then(|| resolve_price_account(overrides, &market.secondary_feed_id)),
            quote_conversion: pyth::is_set(&market.quote_conversion_feed)
                .then(|| resolve_price_account(overrides, &market.quote_conversion_feed)),
        }
    }

    fn keys(self) -> impl Iterator<Item = Pubkey> {
        std::iter::once(self.primary)
            .chain(self.secondary)
            .chain(self.quote_conversion)
    }
}

pub struct Book {
    pub markets: HashMap<u16, Market>,
    pub feeds: HashMap<u16, FeedSet>,
    pub positions: HashMap<Pubkey, Position>,
    pub triggers: HashMap<Pubkey, TriggerOrder>,
    /// `user_account` → its authority, needed to return a closed position's rent.
    pub authorities: HashMap<Pubkey, Pubkey>,
    /// Latest price accounts, keyed by address.
    pub prices: HashMap<Pubkey, PriceUpdateV2>,
    /// The chain's clock as of the last refresh. Positions are judged against this, not
    /// against the keeper's wall clock, because staleness gates compare to on-chain time.
    pub clock: Clock,
    pub refreshed_at: std::time::Instant,
    /// Feed id → price account, for feeds this cluster does not host at the derived address.
    price_account_overrides: HashMap<[u8; 32], Pubkey>,
    /// Price accounts already reported missing. Warned once when an account goes missing, not
    /// on every fast refresh: one delisted-in-all-but-name market's absent account logged 218
    /// warnings in nine minutes on devnet, which buries the warnings that are news.
    missing_prices: std::collections::HashSet<Pubkey>,
}

impl Book {
    /// An empty book that prefers `overrides` when resolving a feed's price account, and
    /// derives the sponsored PDA for anything the map does not name. Pass an empty map on a
    /// cluster whose feeds are sponsored.
    pub fn with_price_accounts(overrides: HashMap<[u8; 32], Pubkey>) -> Self {
        Self {
            price_account_overrides: overrides,
            missing_prices: std::collections::HashSet::new(),
            markets: HashMap::new(),
            feeds: HashMap::new(),
            positions: HashMap::new(),
            triggers: HashMap::new(),
            authorities: HashMap::new(),
            prices: HashMap::new(),
            clock: Clock::default(),
            refreshed_at: std::time::Instant::now(),
        }
    }

    /// Re-read everything: markets, positions, orders, user authorities.
    ///
    /// Deliberately a full re-read rather than an incremental patch. Incremental state that
    /// can silently miss an update is exactly how a keeper ends up not liquidating a position
    /// it believes it already closed, and the whole book is small enough that correctness is
    /// worth far more than the saved bandwidth.
    pub async fn refresh(&mut self, chain: &Chain) -> Result<()> {
        let markets: Vec<(Pubkey, Market)> = chain.all(&solfx_core::ID).await?;
        self.markets.clear();
        self.feeds.clear();
        for (_, market) in markets {
            self.feeds.insert(
                market.market_index,
                FeedSet::for_market(&market, &self.price_account_overrides),
            );
            self.markets.insert(market.market_index, market);
        }

        let positions: Vec<(Pubkey, Position)> = chain.all(&solfx_core::ID).await?;
        self.positions = positions.into_iter().collect();

        let triggers: Vec<(Pubkey, TriggerOrder)> = chain.all(&solfx_core::ID).await?;
        self.triggers = triggers.into_iter().collect();

        let users: Vec<(Pubkey, UserAccount)> = chain.all(&solfx_core::ID).await?;
        self.authorities = users
            .into_iter()
            .map(|(key, user)| (key, user.authority))
            .collect();

        self.refresh_prices(chain).await?;
        self.refreshed_at = std::time::Instant::now();
        Ok(())
    }

    /// Where this cluster keeps `feed_id`'s price. See [`resolve_price_account`].
    #[must_use]
    pub fn price_account_for(&self, feed_id: &[u8; 32]) -> Pubkey {
        resolve_price_account(&self.price_account_overrides, feed_id)
    }

    /// Markets whose primary price account is not present on this cluster.
    ///
    /// Returned rather than logged so the caller chooses the severity. The distinction that
    /// matters: *every* market unresolved is a misconfiguration — almost always a self-posted
    /// cluster with no `--price-accounts` map, where the derived sponsored addresses simply do
    /// not exist — and the keeper should refuse to start rather than run blind. *One* market
    /// unresolved is a dead feed, and stopping would abandon the other markets it protects.
    #[must_use]
    pub fn markets_without_prices(&self) -> Vec<(u16, Pubkey)> {
        let mut out: Vec<(u16, Pubkey)> = self
            .feeds
            .iter()
            .filter(|(_, set)| !self.prices.contains_key(&set.primary))
            .map(|(index, set)| (*index, set.primary))
            .collect();
        out.sort_unstable();
        out
    }

    /// Re-read only the oracle accounts and the clock. This is the fast loop: prices move
    /// every few hundred milliseconds, but the set of open positions does not.
    pub async fn refresh_prices(&mut self, chain: &Chain) -> Result<()> {
        let keys: Vec<Pubkey> = {
            let mut set: Vec<Pubkey> = self
                .feeds
                .values()
                .copied()
                .flat_map(FeedSet::keys)
                .collect();
            set.sort_unstable();
            set.dedup();
            set
        };
        if keys.is_empty() {
            return Ok(());
        }

        let datas = chain.multiple(&keys).await?;
        for (key, data) in keys.iter().zip(datas) {
            let Some(data) = data else {
                if self.missing_prices.insert(*key) {
                    tracing::warn!(%key, "price account missing on chain");
                }
                continue;
            };
            if self.missing_prices.remove(key) {
                tracing::info!(%key, "price account is back on chain");
            }
            match pyth::parse_update(&data) {
                Ok(update) => {
                    self.prices.insert(*key, update);
                }
                Err(e) => tracing::warn!(%key, error = %e, "unreadable price account"),
            }
        }

        let slot = chain.rpc.get_slot().await.context("get_slot")?;
        let unix_timestamp = chain
            .rpc
            .get_block_time(slot)
            .await
            // A slot whose block time is not yet available is normal at `processed`; the
            // keeper's own clock is a good enough stand-in for a staleness comparison
            // measured in tens of seconds.
            .unwrap_or_else(|_| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs().cast_signed())
            });
        self.clock = Clock {
            slot,
            unix_timestamp,
            ..Clock::default()
        };
        Ok(())
    }

    /// Price a market through the **liquidation** gate.
    ///
    /// Liquidation reads use a wider confidence ceiling than trading does (correction C-4, and
    /// the CHF-depeg replay in Phase 4): a depeg widens confidence precisely when positions
    /// most need closing, and a single ceiling would freeze liquidations at that moment while
    /// leaving the pool exposed.
    pub fn price_for_liquidation(&self, market_index: u16) -> Option<MarketPrice> {
        let market = self.markets.get(&market_index)?;
        let feeds = self.feeds.get(&market_index)?;
        let primary = self.prices.get(&feeds.primary)?;
        let secondary = feeds.secondary.and_then(|k| self.prices.get(&k));
        let quote = feeds.quote_conversion.and_then(|k| self.prices.get(&k));
        solfx_core::oracle::load_price_for_liquidation(
            market,
            primary,
            secondary,
            quote,
            &self.clock,
        )
        .ok()
    }

    /// Price a market through the **trading** gate — the one a trigger order is judged
    /// against, because firing a stop is a trade and must clear the same bar a trader would.
    pub fn price_for_trading(&self, market_index: u16) -> Option<MarketPrice> {
        let market = self.markets.get(&market_index)?;
        let feeds = self.feeds.get(&market_index)?;
        let primary = self.prices.get(&feeds.primary)?;
        let secondary = feeds.secondary.and_then(|k| self.prices.get(&k));
        let quote = feeds.quote_conversion.and_then(|k| self.prices.get(&k));
        solfx_core::oracle::load_validated_price(market, primary, secondary, quote, &self.clock)
            .ok()
    }

    /// Price a market the way `crank_market_price` reads it: staleness enforced, confidence and
    /// deviation not — the crank exists to *record* a wide or deviating price, so gating it on
    /// the trading gate would skip it exactly when it matters.
    pub fn price_for_observe(&self, market_index: u16) -> Option<MarketPrice> {
        let market = self.markets.get(&market_index)?;
        let feeds = self.feeds.get(&market_index)?;
        let primary = self.prices.get(&feeds.primary)?;
        let secondary = feeds.secondary.and_then(|k| self.prices.get(&k));
        let quote = feeds.quote_conversion.and_then(|k| self.prices.get(&k));
        solfx_core::oracle::observe_market_price(market, primary, secondary, quote, &self.clock)
            .ok()
    }

    #[must_use]
    pub fn age(&self) -> std::time::Duration {
        self.refreshed_at.elapsed()
    }
}
