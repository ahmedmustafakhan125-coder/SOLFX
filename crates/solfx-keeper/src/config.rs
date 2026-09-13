//! Keeper configuration.
//!
//! Everything here has a default that works against a local validator, because a keeper you
//! cannot start without a config file is a keeper nobody runs during development — and the
//! whole argument for permissionless liquidation is that *other people* run one.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use clap::Parser;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;

/// Pyth's public Hermes endpoint. Used only to *check* whether an on-chain price account has
/// fallen behind the real market; see [`crate::pyth`].
pub const DEFAULT_HERMES: &str = crate::pyth::DEFAULT_HERMES_URL;

#[derive(Parser, Debug, Clone)]
#[command(name = "solfx-keeper", about = "SolFX off-chain keepers")]
pub struct Config {
    /// Which keepers to run. Several may be given; they share one RPC client and one book.
    #[arg(long, value_enum, default_values_t = [Service::Liquidator, Service::Triggers, Service::Cranks, Service::Watchdog], env = "SOLFX_SERVICES")]
    pub service: Vec<Service>,

    #[arg(long, default_value = "http://127.0.0.1:8899", env = "SOLFX_RPC_URL")]
    pub rpc_url: String,

    /// Signer. Needs SOL for fees and, for the liquidator, a USDC token account.
    #[arg(long, default_value = "keeper.json", env = "SOLFX_KEYPAIR")]
    pub keypair: PathBuf,

    /// The liquidator's USDC account, where penalties are paid. Trigger tips arrive as
    /// lamports on the signer itself and need no token account.
    #[arg(long, env = "SOLFX_REWARD_TOKEN_ACCOUNT")]
    pub reward_token_account: Option<Pubkey>,

    #[arg(long, default_value = DEFAULT_HERMES, env = "SOLFX_HERMES_URL")]
    pub hermes_url: String,

    /// Hermes API key. Pyth put the public endpoint behind authentication on 26 Aug 2026.
    /// Free key at https://pythdata.app.
    #[arg(long, env = "PYTH_API_KEY")]
    pub hermes_token: Option<String>,

    /// How often to re-read the whole book from the chain.
    ///
    /// This is the **slow** loop. It exists so a keeper that has been running for a week and
    /// has drifted, or one that just started, converges on the truth; it is not how
    /// liquidations are found in time. See `scan_interval`.
    /// Must be **shorter than `max_book_age_secs`**, with room for a missed refresh —
    /// `Config::validate` refuses otherwise. The old default pair was 30 against a 20-second
    /// tolerance, which made the book stale by construction for a third of every cycle.
    #[arg(long, default_value_t = 15, env = "SOLFX_REFRESH_SECS")]
    pub refresh_secs: u64,

    /// How often to re-price the mirrored book against the latest oracle accounts.
    ///
    /// This is the **fast** loop and it sets the liquidation latency floor, so Phase 7's
    /// "sub-2s" target is really a statement about this number plus one block.
    #[arg(long, default_value_t = 400, env = "SOLFX_SCAN_MS")]
    pub scan_ms: u64,

    /// Funding and session cranks are hourly and daily jobs; polling them fast is waste.
    #[arg(long, default_value_t = 60, env = "SOLFX_CRANK_SECS")]
    pub crank_secs: u64,

    /// Micro-lamports per compute unit. A liquidation competes for blockspace during exactly
    /// the congestion spike that caused it, so bidding zero means arriving after the move.
    #[arg(long, default_value_t = 50_000, env = "SOLFX_PRIORITY_FEE")]
    pub priority_fee_micro_lamports: u64,

    #[arg(long, default_value_t = 400_000, env = "SOLFX_CU_LIMIT")]
    pub compute_unit_limit: u32,

    /// Report a position as a near miss once its health factor drops below this, so an
    /// operator sees pressure building rather than only the liquidations that resulted.
    #[arg(long, default_value_t = 15_000, env = "SOLFX_WATCH_HF_BPS")]
    pub watch_health_factor_bps: u64,

    /// Refuse to act on a book older than this. A keeper that cannot reach its RPC must go
    /// quiet, not keep firing decisions based on a snapshot from ten minutes ago.
    ///
    /// Defaults to three times `refresh_secs`, so one missed refresh is survivable and two
    /// are not. See `Config::validate` for why the relationship is enforced rather than
    /// merely documented.
    #[arg(long, default_value_t = 45, env = "SOLFX_MAX_BOOK_AGE_SECS")]
    pub max_book_age_secs: u64,

    /// Log every evaluation rather than only actions. Very loud; for a single position under
    /// investigation.
    /// Feed id → price account map, for clusters where `price-poster` publishes the prices.
    ///
    /// Without it the keeper derives the sponsored `[shard, feed_id]` PDA, which is correct on
    /// mainnet and wrong anywhere the poster writes into its own keypair accounts — it then
    /// watches addresses that hold nothing and liquidates nothing. A missing file is fine and
    /// means "this cluster's feeds are sponsored"; see `price_map.rs`.
    #[arg(
        long,
        default_value = "price-accounts.json",
        env = "SOLFX_PRICE_ACCOUNTS"
    )]
    pub price_accounts: PathBuf,

    /// Ceiling on RPC calls per second.
    ///
    /// Set below the endpoint's limit, and remember the limit is shared: `price-poster` has
    /// the same flag, and a keeper and a poster on one free-tier key must divide the budget
    /// between them rather than each assume the whole of it. Measured on a free Helius key —
    /// the poster alone ran 11 consecutive clean passes, and lost 491 feeds across 371 passes
    /// with an unthrottled keeper beside it. The poster is the one that suffers, because its
    /// calls are the ones carrying a 60-second deadline.
    #[arg(long, default_value_t = 5, env = "SOLFX_MAX_RPS")]
    pub max_rps: u32,

    #[arg(long, env = "SOLFX_TRACE_EVAL")]
    pub trace_eval: bool,

    /// Evaluate and log, but never send a transaction. The honest way to try a new build
    /// against mainnet.
    #[arg(long, env = "SOLFX_DRY_RUN")]
    pub dry_run: bool,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Liquidations and auto-deleveraging.
    Liquidator,
    /// Take-profit and stop-loss execution.
    Triggers,
    /// Funding, session and price cranks.
    Cranks,
    /// Compares on-chain price accounts against Hermes and complains when they diverge.
    /// Read-only: it sends no transactions.
    Watchdog,
}

impl Config {
    pub fn wants(&self, s: Service) -> bool {
        self.service.contains(&s)
    }

    pub fn refresh_interval(&self) -> Duration {
        Duration::from_secs(self.refresh_secs.max(1))
    }

    pub fn scan_interval(&self) -> Duration {
        Duration::from_millis(self.scan_ms.max(50))
    }

    pub fn crank_interval(&self) -> Duration {
        Duration::from_secs(self.crank_secs.max(5))
    }

    /// `processed` deliberately.
    ///
    /// A liquidator waiting for `confirmed` is a liquidator reacting to a book two blocks
    /// stale, which during a fast move is the difference between a penalty and a bad debt.
    /// The cost of being wrong is one rejected transaction — the program re-checks every
    /// condition, so a keeper's optimism is never authoritative.
    pub fn commitment(&self) -> CommitmentConfig {
        CommitmentConfig::processed()
    }

    /// Refuse a configuration whose book is stale by construction.
    ///
    /// # The failure this exists to prevent
    ///
    /// `book_is_fresh` stands the keeper down for a pass whenever the book is older than
    /// `max_book_age_secs`, and only the **slow** loop refreshes it — `refresh_prices` updates
    /// marks without resetting the age. So if `refresh_secs >= max_book_age_secs` the book is
    /// stale for part of every single cycle, by arithmetic, on a healthy machine with a
    /// healthy RPC.
    ///
    /// Worse than the gap is its regularity. The crank tick is a whole number of seconds and
    /// so is the refresh, so once a tick lands inside the stale window it lands there on every
    /// subsequent cycle rather than drifting out of it. Measured on the VPS with the old
    /// defaults, 2026-09-09: **165 "book is stale; standing down this pass" in ten minutes and
    /// zero cranks in eight**, while systemd reported `active (running)` and the health probe
    /// reported healthy.
    ///
    /// # Why this refuses rather than correcting itself
    ///
    /// The same reason `main` refuses when the protocol account is missing: a keeper that
    /// silently repairs a nonsensical configuration is a keeper running settings its operator
    /// does not know about. The defaults are coherent, so this can only fire on values someone
    /// passed deliberately — and they are the only person who can decide which of the two they
    /// meant.
    pub fn validate(&self) -> Result<()> {
        if self.refresh_secs >= self.max_book_age_secs {
            anyhow::bail!(
                "--refresh-secs {} is not shorter than --max-book-age-secs {}, so the book \
                 would be stale for part of every cycle and the keeper would stand down \
                 without ever saying why. Give the tolerance room for at least one missed \
                 refresh: --max-book-age-secs {} or higher, or --refresh-secs {} or lower.",
                self.refresh_secs,
                self.max_book_age_secs,
                self.refresh_secs.saturating_mul(2),
                self.max_book_age_secs.saturating_sub(1),
            );
        }
        Ok(())
    }

    pub fn load_keypair(&self) -> Result<Keypair> {
        let raw = std::fs::read_to_string(&self.keypair)
            .with_context(|| format!("reading keypair {}", self.keypair.display()))?;
        let bytes: Vec<u8> = serde_json::from_str(&raw)
            .with_context(|| format!("parsing keypair {}", self.keypair.display()))?;
        Keypair::try_from(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("invalid keypair {}: {e}", self.keypair.display()))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use super::*;

    /// The minimum a `Config` needs to parse. Everything else has a default, which is the
    /// point of the first test.
    fn parse(extra: &[&str]) -> Config {
        let mut argv = vec!["solfx-keeper"];
        argv.extend_from_slice(extra);
        Config::parse_from(argv)
    }

    /// The regression that matters. The shipped defaults were `refresh_secs = 30` against
    /// `max_book_age_secs = 20`, so a keeper started with no flags at all was stale for a
    /// third of every cycle and stood down without explaining itself.
    #[test]
    fn the_defaults_are_coherent() {
        let cfg = parse(&[]);
        assert!(
            cfg.validate().is_ok(),
            "a keeper started with no flags must be able to run: refresh={} max_age={}",
            cfg.refresh_secs,
            cfg.max_book_age_secs
        );
        assert!(
            cfg.refresh_secs < cfg.max_book_age_secs,
            "the tolerance must exceed the refresh interval"
        );
    }

    /// One missed refresh must be survivable, or a single slow RPC round trip stands the
    /// keeper down. Two in a row should not be.
    #[test]
    fn the_defaults_survive_one_missed_refresh() {
        let cfg = parse(&[]);
        assert!(cfg.refresh_secs.saturating_mul(2) < cfg.max_book_age_secs);
    }

    #[test]
    fn equal_values_are_refused() {
        // The exact boundary: at equality the book reaches the tolerance the instant before
        // the refresh that would have reset it, so the race is lost every cycle.
        let cfg = parse(&["--refresh-secs", "30", "--max-book-age-secs", "30"]);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn the_old_shipped_defaults_are_refused() {
        let cfg = parse(&["--refresh-secs", "30", "--max-book-age-secs", "20"]);
        let err = cfg.validate().expect_err("30/20 must not be accepted");
        let msg = err.to_string();
        // The message has to name both numbers and a way out; an operator reading a crash
        // loop at 3am should not have to read the source to find the fix.
        assert!(msg.contains("30") && msg.contains("20"), "{msg}");
        assert!(msg.contains("--max-book-age-secs"), "{msg}");
    }

    #[test]
    fn a_shorter_refresh_than_the_tolerance_is_accepted() {
        // What the VPS deployment actually runs.
        assert!(
            parse(&["--refresh-secs", "15", "--max-book-age-secs", "45"])
                .validate()
                .is_ok()
        );
        // And the tightest legal pair.
        assert!(
            parse(&["--refresh-secs", "19", "--max-book-age-secs", "20"])
                .validate()
                .is_ok()
        );
    }
}
