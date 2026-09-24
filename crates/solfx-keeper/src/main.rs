//! SolFX off-chain keepers.
//!
//! # What Phase 7 is actually for
//!
//! Phases 0–6 built a protocol where every dangerous action is *possible* for anyone to
//! perform: liquidation, trigger execution and the cranks are all permissionless
//! instructions. That is the safety property. But a permissionless instruction nobody calls
//! is a promise with no mechanism behind it, and Phase 7 is where the mechanism arrives.
//!
//! # The one design decision worth reading
//!
//! This keeper does not reimplement the risk engine. It calls [`solfx_core::risk::assess`]
//! and the oracle gates directly — the same functions, compiled from the same source, that
//! the program runs on chain. See [`book`] for why that eliminates an entire class of bug
//! rather than merely saving effort.
//!
//! # Running it
//!
//! ```text
//! solfx-keeper --rpc-url http://127.0.0.1:8899 \
//!              --keypair keeper.json \
//!              --reward-token-account <USDC token account> \
//!              --service liquidator --service triggers --service cranks
//! ```
//!
//! `--dry-run` evaluates and logs without sending anything, which is the honest way to point
//! a new build at mainnet.

mod book;
mod chain;
mod config;
mod danger;
mod nox_crank;
// Compiled into the keeper *and* `#[path]`-included by each operator binary. No single
// consumer uses every item — the poster resolves feeds but never reads a price account, the
// keeper does the reverse — so "never used" is a property of which binary is compiling, not
// dead code. The `#[path]` includes carry the same allow.
//
// Both need it, and each needs its *own* attribute: an outer attribute applies to the one
// item that follows it. Inserting `price_map` between this block's allow and `mod pyth`
// once moved the allow onto `price_map` and silently re-exposed three warnings in `pyth`.
#[allow(dead_code)]
mod price_map;
#[allow(dead_code)]
mod pyth;
mod services;
mod throttle;

use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::Parser as _;
use config::{Config, Service};
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::parse();
    init_tracing();

    // Before anything reaches the network. A configuration whose book is stale by
    // construction produces a keeper that logs, stands down, and cranks nothing, while
    // systemd and the health probe both call it healthy — see `Config::validate`.
    cfg.validate()?;

    let payer = cfg.load_keypair()?;
    let chain = chain::Chain::new(&cfg, payer);
    tracing::info!(
        keeper = %chain.pubkey(),
        rpc = %cfg.rpc_url,
        dry_run = cfg.dry_run,
        services = ?cfg.service,
        "starting"
    );

    // Fail loudly at startup if the protocol is not there. A keeper that starts cleanly
    // against the wrong cluster and then quietly finds nothing to do is the worst outcome:
    // it looks healthy on every dashboard while liquidating nothing.
    let vaults = book::Vaults::derive();
    let protocol: solfx_core::state::Protocol = chain
        .account(&vaults.protocol)
        .await
        .context("protocol account not found — wrong cluster, or not yet initialised")?;
    tracing::info!(
        admin = %protocol.admin,
        markets = protocol.num_markets,
        paused = protocol.paused,
        "connected"
    );

    // Where this cluster's prices actually live. Empty on a sponsored cluster, where every
    // address derives; populated on one where `price-poster` publishes into its own accounts.
    let price_entries = price_map::load(&cfg.price_accounts)?;
    let overrides = price_map::by_feed_id(&price_entries);
    tracing::info!(
        map = %cfg.price_accounts.display(),
        feeds = overrides.len(),
        "price account map"
    );

    let shared = Arc::new(services::Shared {
        chain,
        cfg: cfg.clone(),
        book: RwLock::new(book::Book::with_price_accounts(overrides)),
        vaults,
    });

    // One synchronous load before any service starts, so nothing decides from an empty book.
    shared
        .book
        .write()
        .await
        .refresh(&shared.chain)
        .await
        .context("initial book load")?;

    // Same reasoning as the protocol check above, one level deeper. A keeper whose price
    // accounts all resolve to addresses that hold nothing runs perfectly, reports healthy, and
    // liquidates nothing — the failure mode that hides. Every market unresolved is not a dead
    // feed, it is the wrong address book, so refuse to start and say which flag fixes it.
    {
        let book = shared.book.read().await;
        let unresolved = book.markets_without_prices();
        let total = book.feeds.len();
        if !unresolved.is_empty() && unresolved.len() == total {
            for (index, account) in &unresolved {
                tracing::error!(market = index, %account, "no price at this address");
            }
            anyhow::bail!(
                "none of the {total} listed markets has a price account on this cluster.\n\
                 The keeper derives the sponsored Pyth PDA `[shard, feed_id]`, which is right \
                 on mainnet but wrong wherever `price-poster` publishes into its own keypair \
                 accounts.\n\
                 Point --price-accounts at the map the poster wrote (default \
                 price-accounts.json), or run the poster first to create it."
            );
        }
        for (index, account) in &unresolved {
            tracing::error!(
                market = index,
                %account,
                "market has no price on chain — it will not be liquidated or triggered"
            );
        }
    }

    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(services::run_refresher(Arc::clone(&shared)));
    if cfg.wants(Service::Liquidator) {
        tasks.spawn(services::run_liquidator(Arc::clone(&shared)));
    }
    if cfg.wants(Service::Triggers) {
        tasks.spawn(services::run_triggers(Arc::clone(&shared)));
    }
    if cfg.wants(Service::Cranks) {
        tasks.spawn(services::run_cranks(Arc::clone(&shared)));
    }
    if cfg.wants(Service::Watchdog) {
        tasks.spawn(services::run_watchdog(Arc::clone(&shared)));
    }
    if cfg.wants(Service::Nox) {
        tasks.spawn(nox_crank::run_nox(Arc::clone(&shared)));
    }

    // Ctrl-C wins the race and the process exits. There is no in-flight state to drain:
    // every decision this keeper makes is re-validated on chain, so a transaction abandoned
    // mid-flight either landed and was correct, or was dropped and is irrelevant. That is
    // what makes "survives an instance being killed" true rather than aspirational — a
    // restarted keeper rebuilds its whole view from the chain in one pass and owes nothing
    // to its past self.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("shutting down"),
        Some(joined) = tasks.join_next() => {
            match joined {
                Ok(()) => tracing::error!("a service exited unexpectedly"),
                Err(e) => tracing::error!(error = %e, "a service panicked"),
            }
        }
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(filter)
        .init();
}
