//! Does the market list survive on devnet?
//!
//! # Why this exists
//!
//! Phase 0b validated 29 markets against **Hermes**, which reports Pyth's *mainnet*
//! aggregate. Nothing in that measurement says the corresponding price account exists on
//! **devnet**, and devnet's sponsored-feed coverage is far thinner. If USD/INR, USD/TRY and
//! the rest of the EM set are absent there, the devnet deployment lists majors and crypto
//! and nothing else — which would gut the "emerging-market pairs no venue on any chain
//! lists" positioning for the entire capstone demo.
//!
//! That is a question worth twenty minutes before it is worth a deployment script.
//!
//! # The one property that makes this probe trustworthy
//!
//! It derives price-account addresses with [`pyth::price_account`] — **the same function the
//! keeper uses**, compiled from the same source via `#[path]`. A probe that derived addresses
//! its own way would be checking whether *some* account exists, not whether *the account the
//! protocol will actually read* exists, and a green result would mean nothing.
//!
//! # Running it
//!
//! ```text
//! cargo run -p solfx-keeper --bin devnet-feed-probe
//! cargo run -p solfx-keeper --bin devnet-feed-probe -- --cluster mainnet   # the baseline
//! cargo run -p solfx-keeper --bin devnet-feed-probe -- --json > feeds.json
//! ```
//!
//! Read-only. It sends no transaction and needs no keypair.

// `pyth.rs` is shared verbatim with the keeper binary, so it carries items this probe does
// not call (the Hermes latest-price client, in particular). Sharing the file is the point —
// it guarantees identical address derivation — so the unused remainder is suppressed rather
// than split out into a module that could then drift.
#[allow(dead_code)]
#[path = "../pyth.rs"]
mod pyth;

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use clap::Parser as _;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;

/// The 33 markets the devnet deployment intends to list.
///
/// Grouped so the report shows *which part of the product* is at risk, not merely a count:
/// losing an exotic is an inconvenience, losing the whole EM tier is a positioning problem.
const MARKETS: &[(&str, &[&str])] = &[
    (
        "FX majors",
        &["EUR/USD", "GBP/USD", "USD/JPY", "USD/CAD", "AUD/USD"],
    ),
    (
        "FX crosses",
        &[
            "EUR/JPY", "GBP/JPY", "CAD/JPY", "CHF/JPY", "EUR/GBP", "EUR/AUD", "EUR/CHF", "AUD/JPY",
        ],
    ),
    ("FX reduced-leverage", &["USD/CHF", "NZD/USD"]),
    ("Metals", &["XAU/USD", "XAG/USD", "XPT/USD", "XPD/USD"]),
    (
        "Emerging markets",
        &[
            "USD/MXN", "USD/ZAR", "USD/PHP", "USD/INR", "USD/TRY", "USD/TWD", "USD/KRW",
        ],
    ),
    ("LATAM (session-bound)", &["USD/BRL", "USD/CLP", "USD/PEN"]),
    ("Crypto", &["BTC/USD", "ETH/USD", "SOL/USD"]),
    ("Commodities", &["USOILSPOT/USD"]),
];

#[derive(clap::Parser, Clone)]
#[command(
    name = "devnet-feed-probe",
    about = "Check Pyth feed availability per cluster"
)]
struct Args {
    /// Which cluster's price accounts to check.
    #[arg(long, default_value = "devnet")]
    cluster: Cluster,
    /// Override the RPC endpoint (takes precedence over --cluster).
    #[arg(long)]
    rpc_url: Option<String>,
    /// Hermes base URL, for the symbol -> feed id catalogue.
    #[arg(long, default_value = pyth::DEFAULT_HERMES_URL)]
    hermes_url: String,

    /// Hermes API key. Required since 26 Aug 2026 — see `price-poster --help`.
    #[arg(long, env = "PYTH_API_KEY")]
    hermes_token: Option<String>,
    /// Older than this is not LIVE.
    #[arg(long, default_value_t = 120)]
    max_age_secs: i64,
    /// Older than this is ABANDONED rather than merely session-stale. Default 3 days: long
    /// enough to cover a weekend plus a holiday, short enough that a genuinely maintained
    /// feed never trips it.
    #[arg(long, default_value_t = 259_200)]
    abandoned_after_secs: i64,
    /// Emit JSON instead of the table.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Cluster {
    Devnet,
    Mainnet,
    Localnet,
}

impl Cluster {
    fn rpc_url(self) -> &'static str {
        match self {
            Self::Devnet => "https://api.devnet.solana.com",
            Self::Mainnet => "https://api.mainnet-beta.solana.com",
            Self::Localnet => "http://127.0.0.1:8899",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Devnet => "devnet",
            Self::Mainnet => "mainnet-beta",
            Self::Localnet => "localnet",
        }
    }
}

// --- the Hermes catalogue ---------------------------------------------------------------

// --- the report -------------------------------------------------------------------------

/// One market's resolved feed, before the cluster is queried.
struct Planned {
    group: &'static str,
    symbol: &'static str,
    feed: Option<(String, String, Pubkey)>,
}

#[derive(serde::Serialize)]
struct Row {
    group: String,
    symbol: String,
    asset_type: Option<String>,
    feed_id: Option<String>,
    price_account: Option<String>,
    account_exists: bool,
    /// Reported as the raw mantissa and exponent rather than a float: this is a price feed
    /// audit, and silently rounding the thing being audited would be an odd choice.
    price_mantissa: Option<i64>,
    price_exponent: Option<i32>,
    conf_bps: Option<u64>,
    age_secs: Option<i64>,
    verdict: &'static str,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let rpc_url = args
        .rpc_url
        .clone()
        .unwrap_or_else(|| args.cluster.rpc_url().to_string());

    let all: Vec<&str> = MARKETS
        .iter()
        .flat_map(|(_, s)| s.iter().copied())
        .collect();

    if !args.json {
        println!(
            "Pyth feed availability — {} ({rpc_url})",
            args.cluster.label()
        );
        println!("{} markets requested\n", all.len());
    }

    // Shared resolver — see `pyth::resolve_feed_ids`. Keyed uppercase, carrying the
    // fully-qualified Pyth symbol so the report can show what each id actually is.
    let resolved =
        pyth::resolve_feed_ids(&args.hermes_url, args.hermes_token.as_deref(), &all).await?;
    let catalogue: HashMap<String, (String, String)> = resolved
        .into_iter()
        .map(|(sym, f)| (sym.to_uppercase(), (f.feed_id, f.pyth_symbol)))
        .collect();

    // Derive every address first, then fetch in bulk: one getMultipleAccounts per 100 keys
    // rather than 33 round trips.
    let mut planned: Vec<Planned> = Vec::new();
    for (group, symbols) in MARKETS {
        for symbol in *symbols {
            let feed = catalogue.get(&symbol.to_uppercase()).and_then(|(id, ty)| {
                let mut bytes = [0u8; 32];
                hex::decode_to_slice(id.trim_start_matches("0x"), &mut bytes)
                    .ok()
                    .map(|()| (id.clone(), ty.clone(), pyth::price_account(&bytes)))
            });
            planned.push(Planned {
                group,
                symbol,
                feed,
            });
        }
    }

    let keys: Vec<Pubkey> = planned
        .iter()
        .filter_map(|p| p.feed.as_ref().map(|(_, _, k)| *k))
        .collect();

    let rpc = RpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::confirmed());
    let mut fetched: HashMap<Pubkey, Option<Vec<u8>>> = HashMap::new();
    for chunk in keys.chunks(100) {
        let accounts = rpc
            .get_multiple_accounts(chunk)
            .await
            .with_context(|| format!("getMultipleAccounts against {rpc_url}"))?;
        for (key, account) in chunk.iter().zip(accounts) {
            fetched.insert(*key, account.map(|a| a.data));
        }
    }

    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0u64, |d| d.as_secs()),
    )
    .unwrap_or(i64::MAX);

    let mut rows: Vec<Row> = Vec::new();
    for Planned {
        group,
        symbol,
        feed,
    } in planned
    {
        let group = group.to_string();
        let symbol = symbol.to_string();
        let Some((feed_id, asset_type, account)) = feed else {
            rows.push(Row {
                group,
                symbol,
                asset_type: None,
                feed_id: None,
                price_account: None,
                account_exists: false,
                price_mantissa: None,
                price_exponent: None,
                conf_bps: None,
                age_secs: None,
                verdict: "NO FEED",
            });
            continue;
        };

        let data = fetched.get(&account).cloned().flatten();
        let Some(data) = data else {
            rows.push(Row {
                group,
                symbol,
                asset_type: Some(asset_type),
                feed_id: Some(feed_id),
                price_account: Some(account.to_string()),
                account_exists: false,
                price_mantissa: None,
                price_exponent: None,
                conf_bps: None,
                age_secs: None,
                verdict: "NOT ON CLUSTER",
            });
            continue;
        };

        match pyth::parse_update(&data) {
            Ok(update) => {
                let m = update.price_message;
                let age = now.saturating_sub(m.publish_time);
                let conf_bps = m
                    .conf
                    .checked_mul(10_000)
                    .and_then(|n| n.checked_div(m.price.unsigned_abs()));
                // Three states, not two. A feed that is hours old outside its session is
                // healthy; one that is months old is not maintained on this cluster at all,
                // and collapsing the two would hide exactly the finding this probe is for.
                let verdict = if age > args.abandoned_after_secs {
                    "ABANDONED"
                } else if age > args.max_age_secs {
                    "SESSION-STALE"
                } else {
                    "LIVE"
                };
                rows.push(Row {
                    group,
                    symbol,
                    asset_type: Some(asset_type),
                    feed_id: Some(feed_id),
                    price_account: Some(account.to_string()),
                    account_exists: true,
                    price_mantissa: Some(m.price),
                    price_exponent: Some(m.exponent),
                    conf_bps,
                    age_secs: Some(age),
                    verdict,
                });
            }
            Err(_) => rows.push(Row {
                group,
                symbol,
                asset_type: Some(asset_type),
                feed_id: Some(feed_id),
                price_account: Some(account.to_string()),
                account_exists: true,
                price_mantissa: None,
                price_exponent: None,
                conf_bps: None,
                age_secs: None,
                verdict: "UNREADABLE",
            }),
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    let mut current = "";
    for row in &rows {
        if row.group != current {
            current = &row.group;
            println!("\n  {current}");
            println!(
                "  {:<16} {:>16} {:>9} {:>10}  verdict",
                "symbol", "price", "conf bps", "age"
            );
            println!("  {}", "-".repeat(70));
        }
        let price = match (row.price_mantissa, row.price_exponent) {
            (Some(m), Some(e)) => format!("{m}e{e}"),
            _ => "—".to_string(),
        };
        let conf = row.conf_bps.map_or("—".to_string(), |c| c.to_string());
        let age = row.age_secs.map_or("—".to_string(), format_age);
        println!(
            "  {:<16} {price:>16} {conf:>9} {age:>10}  {}",
            row.symbol, row.verdict
        );
    }

    // The summary is the actual output; the table above is evidence for it.
    let count = |v: &str| rows.iter().filter(|r| r.verdict == v).count();
    let (live, session, abandoned) = (count("LIVE"), count("SESSION-STALE"), count("ABANDONED"));
    let (missing, no_feed) = (count("NOT ON CLUSTER"), count("NO FEED"));

    println!("\n  {}", "=".repeat(70));
    println!("  of {} markets:", rows.len());
    println!("    LIVE            {live:>3}   publishing now");
    println!("    SESSION-STALE   {session:>3}   plausibly just outside trading hours");
    println!("    ABANDONED       {abandoned:>3}   account exists but has not published in days");
    println!("    NOT ON CLUSTER  {missing:>3}   no price account at the derived address");
    println!("    NO FEED         {no_feed:>3}   not in the Hermes catalogue at all");

    println!("\n  Tradeable per group (LIVE or SESSION-STALE only):");
    for (group, symbols) in MARKETS {
        let usable = rows
            .iter()
            .filter(|r| r.group == *group)
            .filter(|r| r.verdict == "LIVE" || r.verdict == "SESSION-STALE")
            .count();
        let flag = if usable == 0 {
            "   <-- ENTIRE GROUP UNAVAILABLE"
        } else {
            ""
        };
        println!("    {:<24} {usable}/{}{flag}", group, symbols.len());
    }

    println!("\n  ABANDONED is the finding that matters: the account exists, so a naive");
    println!("  existence check would pass, but the protocol would refuse every trade on it");
    println!("  (MAX_ALLOWED_STALENESS_SECONDS = 60). Those markets are not listable here.");
    Ok(())
}

/// Ages here span seconds to months, and "10029862" does not read as "116 days".
///
/// `integer_division` is denied workspace-wide because silent truncation in financial maths
/// is a real bug class. This is a human-readable age label — truncating "23.7 hours" to
/// "23h" is the intended behaviour, and no decision is taken on the result.
#[allow(clippy::integer_division)]
fn format_age(secs: i64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7_200 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3_600),
        s => format!("{}d", s / 86_400),
    }
}
