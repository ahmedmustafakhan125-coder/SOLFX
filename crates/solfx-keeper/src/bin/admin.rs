//! Market administration: status transitions and oracle repointing.
//!
//! # Why this exists
//!
//! `set_market_status` and `set_market_oracle` had no client. That was tolerable while the
//! only status change happened inside `init-protocol`'s activation pass, and it stopped being
//! tolerable the moment devnet ended up with three markets listed against the wrong feed:
//! the repair sequence is halt → repoint → re-activate, and nothing could send any of the
//! three.
//!
//! # The compare-and-swap, and why this tool does not paper over it
//!
//! `set_market_oracle` takes `expected_current` and rejects the transaction unless it matches
//! the feed already stored. That is Drift's `handle_update_spot_market_oracle` guard, and its
//! purpose is to stop a script working from a stale picture of the chain from repointing a
//! market it does not actually understand.
//!
//! This tool satisfies that guard honestly rather than trivially: it reads the market
//! immediately before building the transaction and shows the operator what it read. Passing
//! `--expected` explicitly is supported and is checked **locally, before sending**, so a
//! mismatch costs an error message instead of a failed transaction and a confusing
//! `WrongOracleFeed` in the logs.
//!
//! # Running it
//!
//! ```text
//! cargo run -p solfx-keeper --bin admin -- list
//! cargo run -p solfx-keeper --bin admin -- status --market 1 --set halted
//! cargo run -p solfx-keeper --bin admin -- oracle --market 1 --new-feed USD/JPY
//! cargo run -p solfx-keeper --bin admin -- repair --market 1 --activate
//! ```

#[allow(dead_code)]
#[path = "../pyth.rs"]
mod pyth;

use anchor_lang::{AccountDeserialize, InstructionData as _, ToAccountMetas as _};
use anyhow::{anyhow, bail, Context as _, Result};
use clap::Parser as _;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer as _;
use solana_transaction::Transaction;

use solfx_core::constants::{MARKET_SEED, PROTOCOL_SEED};
use solfx_core::state::{Market, MarketStatus, Protocol};

/// These instructions write a handful of fields and emit one event. The default 200k budget
/// would do; requesting less is cheaper and makes a runaway obvious rather than silent.
const ADMIN_CU: u32 = 60_000;

#[derive(clap::Parser)]
#[command(name = "admin", about = "Administer SolFX markets")]
struct Args {
    #[arg(long, env = "SOLFX_RPC_URL", default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    /// Must be the protocol admin — every instruction here is `has_one = admin`.
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
    #[arg(long, default_value = pyth::DEFAULT_HERMES_URL)]
    hermes_url: String,
    /// Hermes API key. Required since 26 Aug 2026.
    #[arg(long, env = "PYTH_API_KEY")]
    hermes_token: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Read every market on chain: index, symbol, status, stored feed.
    List {
        /// Also resolve each symbol against Hermes and flag markets whose stored feed is not
        /// the one the symbol resolves to. Needs a Hermes token.
        #[arg(long)]
        verify: bool,
    },
    /// Move a market through the status machine.
    Status {
        /// Market index, or symbol as stored on chain (e.g. `EURUSD`).
        #[arg(long)]
        market: String,
        #[arg(long, value_enum)]
        set: StatusArg,
    },
    /// Repoint a market at a different Pyth feed. The market must not be Active.
    Oracle {
        #[arg(long)]
        market: String,
        /// A 64-char hex feed id, or a SolFX symbol (`USD/JPY`) to resolve via Hermes.
        #[arg(long)]
        new_feed: String,
        /// The feed you believe is currently stored. Checked locally before sending.
        #[arg(long)]
        expected: Option<String>,
    },
    /// The full repair sequence: halt, repoint, and optionally re-activate.
    Repair {
        #[arg(long)]
        market: String,
        /// Defaults to resolving the market's own on-chain symbol against Hermes.
        #[arg(long)]
        new_feed: Option<String>,
        /// Re-activate after repointing. Without it the market is left `Halted` and the
        /// activation command is printed instead — the same verify-then-stop default
        /// `init-protocol --activate` uses, and for the same reason: a repointed market is
        /// exactly as unverified as a freshly listed one.
        #[arg(long)]
        activate: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum StatusArg {
    Initialized,
    Active,
    ReduceOnly,
    Halted,
    GapWindow,
    Delisted,
}

impl StatusArg {
    /// `WeekendMode` and `PreOpenWindow` are deliberately absent: the program rejects both
    /// from this instruction, so offering them here would only produce a failed transaction.
    fn to_status(self) -> MarketStatus {
        match self {
            Self::Initialized => MarketStatus::Initialized,
            Self::Active => MarketStatus::Active,
            Self::ReduceOnly => MarketStatus::ReduceOnly,
            Self::Halted => MarketStatus::Halted,
            Self::GapWindow => MarketStatus::GapWindow,
            Self::Delisted => MarketStatus::Delisted,
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

async fn run(args: Args) -> Result<()> {
    let admin = load_keypair(&args.keypair)?;
    let rpc = RpcClient::new_with_commitment(args.rpc_url.clone(), CommitmentConfig::confirmed());

    match &args.cmd {
        Cmd::List { verify } => list(&args, &rpc, *verify).await,
        Cmd::Status { market, set } => {
            let m = find_market(&rpc, market).await?;
            set_status(&rpc, &admin, &m, set.to_status()).await
        }
        Cmd::Oracle {
            market,
            new_feed,
            expected,
        } => {
            let m = find_market(&rpc, market).await?;
            let new = resolve_feed(&args, new_feed).await?;
            set_oracle(&rpc, &admin, &m, new, expected.as_deref()).await
        }
        Cmd::Repair {
            market,
            new_feed,
            activate,
        } => {
            let m = find_market(&rpc, market).await?;
            let wanted = match new_feed {
                Some(f) => f.clone(),
                // The market carries its own symbol on chain; that is the authority on what
                // it is supposed to price, not whatever the operator types.
                None => m.market.symbol_str().to_string(),
            };
            let new = resolve_feed(&args, &wanted).await?;
            repair(&rpc, &admin, &m, new, *activate).await
        }
    }
}

// --- reading -------------------------------------------------------------------------------

struct Listed {
    index: u16,
    pda: Pubkey,
    market: Market,
}

fn protocol_pda() -> Pubkey {
    Pubkey::find_program_address(&[PROTOCOL_SEED], &solfx_core::ID).0
}

fn market_pda(index: u16) -> Pubkey {
    Pubkey::find_program_address(&[MARKET_SEED, &index.to_le_bytes()], &solfx_core::ID).0
}

async fn load_all(rpc: &RpcClient) -> Result<Vec<Listed>> {
    let protocol_key = protocol_pda();
    let data = rpc
        .get_account_data(&protocol_key)
        .await
        .with_context(|| format!("reading protocol {protocol_key} — is the RPC url right?"))?;
    let protocol = Protocol::try_deserialize(&mut data.as_slice()).context("decoding Protocol")?;

    let mut out = Vec::new();
    for index in 0..protocol.num_markets {
        let pda = market_pda(index);
        let Ok(bytes) = rpc.get_account_data(&pda).await else {
            println!("  ! market {index} ({pda}) is missing on this cluster");
            continue;
        };
        let market = Market::try_deserialize(&mut bytes.as_slice())
            .with_context(|| format!("decoding market {index}"))?;
        out.push(Listed { index, pda, market });
    }
    Ok(out)
}

/// Accepts an index or an on-chain symbol. Symbols are compared case-insensitively and with
/// any separator removed, so `USD/JPY`, `usdjpy` and `USDJPY` all find the same market.
async fn find_market(rpc: &RpcClient, needle: &str) -> Result<Listed> {
    let all = load_all(rpc).await?;
    if let Ok(index) = needle.parse::<u16>() {
        return all
            .into_iter()
            .find(|m| m.index == index)
            .ok_or_else(|| anyhow!("no market at index {index}"));
    }
    let want = normalise(needle);
    let mut hits: Vec<Listed> = all
        .into_iter()
        .filter(|m| normalise(m.market.symbol_str()) == want)
        .collect();
    match hits.len() {
        1 => hits.pop().ok_or_else(|| anyhow!("unreachable")),
        0 => bail!("no market with symbol {needle} — run `admin list`"),
        // Duplicate symbols are exactly the devnet situation this tool repairs, so refusing
        // to guess is the point: repointing the wrong one of a pair is unrecoverable noise.
        _ => bail!(
            "{needle} matches {} markets ({}). Use --market <index>.",
            hits.len(),
            hits.iter()
                .map(|m| m.index.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn normalise(symbol: &str) -> String {
    symbol
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_uppercase()
}

// --- commands ------------------------------------------------------------------------------

async fn list(args: &Args, rpc: &RpcClient, verify: bool) -> Result<()> {
    let all = load_all(rpc).await?;
    println!("{} market(s) on {}\n", all.len(), args.rpc_url);

    let resolved = if verify {
        let symbols: Vec<String> = all
            .iter()
            .map(|m| spaced_symbol(m.market.symbol_str()))
            .collect();
        let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
        match pyth::resolve_feed_ids(&args.hermes_url, args.hermes_token.as_deref(), &refs).await {
            Ok(map) => Some(map),
            Err(e) => {
                println!("  ! could not reach Hermes, showing stored feeds only: {e}\n");
                None
            }
        }
    } else {
        None
    };

    let mut unhealthy = 0usize;
    for m in &all {
        let stored = hex::encode(m.market.pyth_feed_id);
        let duplicate = all.iter().any(|o| {
            o.index != m.index
                && normalise(o.market.symbol_str()) == normalise(m.market.symbol_str())
        });
        println!(
            "  [{:>2}] {:<10} {:?}{}",
            m.index,
            m.market.symbol_str(),
            m.market.status,
            if duplicate {
                "   (duplicate symbol)"
            } else {
                ""
            }
        );
        println!("       {}", m.pda);
        println!("       feed {stored}");

        if let Some(map) = resolved.as_ref() {
            let key = spaced_symbol(m.market.symbol_str());
            match map.get(&key) {
                Some(f) if f.feed_id.eq_ignore_ascii_case(&stored) => {
                    println!("       ok   matches {}", f.pyth_symbol);
                }
                // `symbol` is write-once, so a market sharing a symbol with another can never
                // become a distinct instrument. Repointing it at the resolved feed would not
                // repair anything — it would promote a broken duplicate into a working one,
                // and two identical markets is worse than one obviously dead one.
                Some(_) if duplicate => {
                    unhealthy = unhealthy.saturating_add(1);
                    println!("       DUPLICATE — shares a symbol with another market, and");
                    println!("       `symbol` is write-once, so this can never be repaired into");
                    println!("       a distinct instrument. Take it out of service instead:");
                    println!("         admin status --market {} --set halted", m.index);
                }
                Some(f) => {
                    unhealthy = unhealthy.saturating_add(1);
                    println!("       MISMATCH — {} resolves to", f.pyth_symbol);
                    println!("       want {}", f.feed_id);
                    println!(
                        "       repair: admin repair --market {} --activate",
                        m.index
                    );
                }
                None => {
                    unhealthy = unhealthy.saturating_add(1);
                    println!("       UNRESOLVED — no entitled Pyth feed for this symbol");
                }
            }
        }
        println!();
    }

    if verify && unhealthy > 0 {
        println!("{unhealthy} market(s) need attention.");
    }
    Ok(())
}

async fn set_status(
    rpc: &RpcClient,
    admin: &Keypair,
    m: &Listed,
    new_status: MarketStatus,
) -> Result<()> {
    if m.market.status == new_status {
        println!(
            "[{}] {} is already {new_status:?} — nothing to do",
            m.index,
            m.market.symbol_str()
        );
        return Ok(());
    }
    println!(
        "[{}] {}: {:?} -> {new_status:?}",
        m.index,
        m.market.symbol_str(),
        m.market.status
    );

    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::AdminMarket {
            admin: admin.pubkey(),
            protocol: protocol_pda(),
            market: m.pda,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::SetMarketStatus { new_status }.data(),
    };
    let sig = send(rpc, admin, vec![ix]).await?;
    println!("  ok {sig}");
    Ok(())
}

async fn set_oracle(
    rpc: &RpcClient,
    admin: &Keypair,
    m: &Listed,
    new_feed: [u8; 32],
    expected: Option<&str>,
) -> Result<()> {
    let current = m.market.pyth_feed_id;

    // Every one of these is a guard the program also enforces. Checking here costs nothing
    // and turns a failed transaction into a sentence.
    if let Some(claimed) = expected {
        let claimed = parse_feed_hex(claimed)?;
        if claimed != current {
            bail!(
                "--expected does not match the chain.\n  \
                 you said  {}\n  on chain  {}\n\
                 The market moved under you, or the index is wrong. Re-read with `admin list`.",
                hex::encode(claimed),
                hex::encode(current)
            );
        }
    }
    if new_feed == current {
        bail!(
            "[{}] {} already points at {} — the program rejects a no-op repoint",
            m.index,
            m.market.symbol_str(),
            hex::encode(current)
        );
    }
    if m.market.status == MarketStatus::Active {
        bail!(
            "[{}] {} is Active. Repointing an active market would re-price every open \
             position against an instrument it was never opened on.\n  \
             Halt it first: admin status --market {} --set halted",
            m.index,
            m.market.symbol_str(),
            m.index
        );
    }

    println!(
        "[{}] {} ({:?})",
        m.index,
        m.market.symbol_str(),
        m.market.status
    );
    println!("  from {}", hex::encode(current));
    println!("  to   {}", hex::encode(new_feed));

    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::AdminMarket {
            admin: admin.pubkey(),
            protocol: protocol_pda(),
            market: m.pda,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::SetMarketOracle {
            expected_current: current,
            new_feed_id: new_feed,
        }
        .data(),
    };
    let sig = send(rpc, admin, vec![ix]).await?;
    println!("  ok {sig}");
    Ok(())
}

async fn repair(
    rpc: &RpcClient,
    admin: &Keypair,
    m: &Listed,
    new_feed: [u8; 32],
    activate: bool,
) -> Result<()> {
    if new_feed == m.market.pyth_feed_id {
        println!(
            "[{}] {} already points at the resolved feed — nothing to repair",
            m.index,
            m.market.symbol_str()
        );
        return Ok(());
    }

    if m.market.status != MarketStatus::Halted {
        set_status(rpc, admin, m, MarketStatus::Halted).await?;
    }

    // Re-read: `set_status` changed the account, and `set_oracle` refuses to send against a
    // stale copy. This is the same discipline the compare-and-swap exists to enforce.
    let fresh = find_market(rpc, &m.index.to_string()).await?;
    set_oracle(rpc, admin, &fresh, new_feed, None).await?;

    if activate {
        let fresh = find_market(rpc, &m.index.to_string()).await?;
        set_status(rpc, admin, &fresh, MarketStatus::Active).await?;
    } else {
        println!(
            "\n  left Halted. Verify the feed, then:\n    \
             admin status --market {} --set active",
            m.index
        );
    }
    Ok(())
}

// --- plumbing ------------------------------------------------------------------------------

/// A stored symbol is `USDJPY`; Hermes and `pyth::asset_class_for` expect `USD/JPY`.
fn spaced_symbol(stored: &str) -> String {
    let clean = normalise(stored);
    if clean.len() == 6 {
        let (base, quote) = clean.split_at(3);
        format!("{base}/{quote}")
    } else {
        stored.to_string()
    }
}

/// Accepts a 64-char hex id or a symbol to resolve against Hermes.
async fn resolve_feed(args: &Args, wanted: &str) -> Result<[u8; 32]> {
    let trimmed = wanted.trim_start_matches("0x");
    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return parse_feed_hex(trimmed);
    }

    let symbol = spaced_symbol(wanted);
    println!("resolving {symbol} against Hermes…");
    let map = pyth::resolve_feed_ids(
        &args.hermes_url,
        args.hermes_token.as_deref(),
        &[symbol.as_str()],
    )
    .await
    .context("resolving the feed id — is PYTH_API_KEY set?")?;

    let found = map
        .get(&symbol)
        .ok_or_else(|| anyhow!("Hermes has no entitled feed for {symbol}"))?;
    println!("  {} -> {}", found.pyth_symbol, found.feed_id);
    parse_feed_hex(&found.feed_id)
}

fn parse_feed_hex(text: &str) -> Result<[u8; 32]> {
    let clean = text.trim().trim_start_matches("0x");
    let mut out = [0u8; 32];
    hex::decode_to_slice(clean, &mut out)
        .with_context(|| format!("{text} is not a 32-byte hex feed id"))?;
    if out == [0u8; 32] {
        bail!(
            "an all-zero feed id is rejected on chain — it is what an uninitialised id looks like"
        );
    }
    Ok(out)
}

async fn send(rpc: &RpcClient, payer: &Keypair, ixs: Vec<Instruction>) -> Result<String> {
    let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(ADMIN_CU)];
    all.extend(ixs);
    let blockhash = rpc.get_latest_blockhash().await.context("blockhash")?;
    let msg = Message::new(&all, Some(&payer.pubkey()));
    let mut tx = Transaction::new_unsigned(msg);
    tx.try_sign(&[payer], blockhash).context("signing")?;
    Ok(rpc.send_and_confirm_transaction(&tx).await?.to_string())
}

fn load_keypair(path: &str) -> Result<Keypair> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        format!(
            "{}/{rest}",
            std::env::var("HOME").context("HOME is not set")?
        )
    } else {
        path.to_string()
    };
    let bytes: Vec<u8> = serde_json::from_str(
        &std::fs::read_to_string(&expanded).with_context(|| format!("reading {expanded}"))?,
    )?;
    Keypair::try_from(bytes.as_slice()).map_err(|e| anyhow!("invalid keypair: {e}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn symbols_match_regardless_of_separator_or_case() {
        assert_eq!(normalise("USD/JPY"), "USDJPY");
        assert_eq!(normalise("usdjpy"), "USDJPY");
        assert_eq!(normalise("USDJPY"), "USDJPY");
        assert_eq!(normalise("XAU/USD"), "XAUUSD");
    }

    /// The stored symbol has no separator, but `asset_class_for` and the Hermes catalogue
    /// both key on the slashed form. Getting this wrong resolves nothing and looks like an
    /// entitlement failure.
    #[test]
    fn stored_symbols_regain_their_separator() {
        assert_eq!(spaced_symbol("USDJPY"), "USD/JPY");
        assert_eq!(spaced_symbol("BTCUSD"), "BTC/USD");
        assert_eq!(spaced_symbol("XAGUSD"), "XAG/USD");
        // Not six characters: left alone rather than split at a guess.
        assert_eq!(spaced_symbol("USOILSPOTUSD"), "USOILSPOTUSD");
    }

    #[test]
    fn feed_hex_parses_with_or_without_prefix() {
        let bare = "ef2c98c8b6f2b0e9b1c0a6d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3";
        let with = format!("0x{bare}");
        assert_eq!(
            parse_feed_hex(bare).unwrap(),
            parse_feed_hex(&with).unwrap()
        );
    }

    /// The program rejects an all-zero id; catching it here saves a transaction.
    #[test]
    fn an_all_zero_feed_is_refused_before_sending() {
        let zeros = "0".repeat(64);
        assert!(parse_feed_hex(&zeros).is_err());
    }

    #[test]
    fn a_malformed_feed_id_is_refused() {
        assert!(parse_feed_hex("not-hex").is_err());
        assert!(parse_feed_hex("abcd").is_err(), "too short");
    }
}
