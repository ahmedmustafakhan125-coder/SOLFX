//! Brings a freshly deployed SolFX up to a state you can actually trade against.
//!
//! # Why this is a program and not a shell script
//!
//! `scripts/deploy-localnet.sh` stops after `program deploy` and says so plainly: the
//! protocol is deployed but not initialised, because `initialize_protocol` needs a mint and
//! an admin and `initialize_market` needs thirty parameters of risk envelope per market.
//! Those are deployment decisions. Encoding them in `bash` with `solana` calls would mean
//! hand-rolling Borsh for a thirty-field struct in a language with no types.
//!
//! Here the parameters are the **same Rust structs the program deserialises**, so a field
//! renamed in `solfx-core` breaks this at compile time rather than at transaction time with
//! an opaque `InstructionDidNotDeserialize`.
//!
//! # The risk envelope
//!
//! Every number in [`TIERS`] traces to something already decided: the confidence
//! measurements in `docs/oracle-feasibility.md`, the reference market in
//! `programs/solfx-core/tests/common/mod.rs`, and D3 in `docs/ARCHITECTURE.md` (50x on
//! Tier 1, tiered down by class). Nothing here is invented. Where a value *is* a judgement
//! call for local testing — pool size, the starting balance — it is a flag.
//!
//! # Running it
//!
//! ```text
//! # localnet, after scripts/deploy-localnet.sh
//! cargo run -p solfx-keeper --bin init-protocol -- --rpc-url http://127.0.0.1:8899
//!
//! # devnet, once the program is deployed there
//! cargo run -p solfx-keeper --bin init-protocol -- --rpc-url https://api.devnet.solana.com
//!
//! cargo run -p solfx-keeper --bin init-protocol -- --dry-run     # print the plan, send nothing
//! ```
//!
//! It is idempotent: an account that already exists is reported and skipped, so re-running
//! after a partial failure resumes rather than starting over.

#[allow(dead_code)]
#[path = "../pyth.rs"]
mod pyth;

#[allow(dead_code)]
#[path = "../contracts.rs"]
mod contracts;

use std::collections::HashMap;
use std::path::PathBuf;

use anchor_lang::{AccountDeserialize as _, InstructionData as _, ToAccountMetas as _};
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

use solfx_core::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_MINT_SEED,
    LP_POOL_SEED, LP_VAULT_SEED, MARKET_SEED, PROTOCOL_SEED,
};
use solfx_core::instructions::admin::{
    InitializeMarketParams, InitializeProtocolParams, UpdateRiskParams,
};
use solfx_core::state::{FeedKind, MarketStatus, PriceSource, QuoteConversionKind};

/// The Rent sysvar. A fixed address, spelled out because Anchor 1.1.2 does not re-export it
/// at a stable path and a wrong sysvar fails as a constraint violation that names nothing.
const RENT_SYSVAR: Pubkey = solana_pubkey::pubkey!("SysvarRent111111111111111111111111111111111");

const ONE_USDC: u64 = 1_000_000;

/// A market to list, and the risk class it belongs to.
#[derive(Clone, Copy)]
struct MarketSpec {
    symbol: &'static str,
    tier: Tier,
}

/// Risk classes, from `docs/oracle-feasibility.md` §3.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// Majors and the tightest crosses. conf p95 <= 5.2 bps.
    Major,
    /// USD/CHF, NZD/USD, XPT. conf p95 7.7-18.1 bps.
    Reduced,
    /// The EM set. conf p95 2.5-11.1 bps, but structurally more volatile.
    Emerging,
    /// Metals: session-bound, tight in session.
    Metal,
    /// Crypto: the only genuinely continuous feeds we have.
    Crypto,
    /// Oil. Session-bound, wider.
    Commodity,
}

/// `(max_leverage, imr_bps, mmr_bps, max_conf_bps, liq_max_conf_bps, base_spread_bps)`
///
/// `liquidation_max_conf_bps` is deliberately far above `max_conf_bps` — see the CHF-depeg
/// reasoning in `tests/common/mod.rs`. A liquidation ceiling set at "a few times normal"
/// does not fire during a depeg, and a position that cannot be liquidated keeps falling.
const TIERS: &[(Tier, u16, u16, u16, u16, u16, u16)] = &[
    (Tier::Major, 50, 200, 100, 15, 300, 2),
    (Tier::Reduced, 20, 500, 250, 25, 400, 4),
    (Tier::Emerging, 10, 1_000, 500, 30, 500, 8),
    (Tier::Metal, 20, 500, 250, 20, 400, 5),
    (Tier::Crypto, 10, 1_000, 500, 30, 500, 6),
    (Tier::Commodity, 10, 1_000, 500, 35, 600, 10),
];

/// The starter set for local testing.
///
/// Chosen to cover *distinct code paths* rather than to be representative, and constrained
/// by what a **free Pyth plan can actually read**. Pyth put Hermes behind authentication on
/// 26 Aug 2026; the free tier is "view-only access to all symbols" in the Terminal UI, which
/// is not the same as API access — measured, 9 of 86 FX/metal/commodity feeds return 200 and
/// the other 77 return 403 `Not entitled`. Pro, at $2,500/mo minimum, is what covers the
/// rest.
///
/// So the set below is every free-tier feed that exercises something different:
///
/// | market   | what it is the only test of                                        |
/// |----------|--------------------------------------------------------------------|
/// | EUR/USD  | USD-quoted major — the reference path                              |
/// | USD/JPY  | JPY-quoted, so the **quote conversion** of correction C-3, and the  |
/// |          | 2-decimal JPY pip convention                                       |
/// | USD/CNH  | a managed currency, on the reduced-leverage tier                   |
/// | XAU/USD  | metals: 100oz contract, session calendar, CME hours                |
/// | XAG/USD  | a second metal, to prove the class is not one hard-coded symbol    |
/// | BTC/USD  | the only continuous feed — the one thing testable at a weekend     |
///
/// The three markets this replaces (EUR/JPY, USD/INR and a BTC/USD that resolved to a
/// funding rate) still occupy devnet indices 1, 2 and 4. They cannot be *removed* —
/// `num_markets` only grows — but since `set_market_oracle` they can be repaired in place:
/// halt, repoint at the correct feed, re-activate. That is what `admin repair` does.
/// `--all` lists the full 33-market set, most of which needs a paid plan.
const STARTER: &[MarketSpec] = &[
    MarketSpec {
        symbol: "EUR/USD",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "USD/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "USD/CNH",
        tier: Tier::Reduced,
    },
    MarketSpec {
        symbol: "XAU/USD",
        tier: Tier::Metal,
    },
    MarketSpec {
        symbol: "XAG/USD",
        tier: Tier::Metal,
    },
    MarketSpec {
        symbol: "BTC/USD",
        tier: Tier::Crypto,
    },
];

const ALL: &[MarketSpec] = &[
    MarketSpec {
        symbol: "EUR/USD",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "GBP/USD",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "USD/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "USD/CAD",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "AUD/USD",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "EUR/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "GBP/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "CAD/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "CHF/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "EUR/GBP",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "EUR/AUD",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "EUR/CHF",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "AUD/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "USD/CHF",
        tier: Tier::Reduced,
    },
    MarketSpec {
        symbol: "NZD/USD",
        tier: Tier::Reduced,
    },
    MarketSpec {
        symbol: "XAU/USD",
        tier: Tier::Metal,
    },
    MarketSpec {
        symbol: "XAG/USD",
        tier: Tier::Metal,
    },
    MarketSpec {
        symbol: "XPT/USD",
        tier: Tier::Reduced,
    },
    MarketSpec {
        symbol: "XPD/USD",
        tier: Tier::Reduced,
    },
    MarketSpec {
        symbol: "USD/MXN",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/ZAR",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/PHP",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/INR",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/TRY",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/TWD",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/KRW",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/BRL",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/CLP",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "USD/PEN",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "BTC/USD",
        tier: Tier::Crypto,
    },
    MarketSpec {
        symbol: "ETH/USD",
        tier: Tier::Crypto,
    },
    MarketSpec {
        symbol: "SOL/USD",
        tier: Tier::Crypto,
    },
    MarketSpec {
        symbol: "USOILSPOT/USD",
        tier: Tier::Commodity,
    },
];

#[derive(clap::Parser)]
#[command(
    name = "init-protocol",
    about = "Initialise SolFX and list its markets"
)]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[arg(long, default_value = pyth::DEFAULT_HERMES_URL)]
    hermes_url: String,

    /// Hermes API key. Required since 26 Aug 2026 — see `price-poster --help`.
    #[arg(long, env = "PYTH_API_KEY")]
    hermes_token: Option<String>,
    /// Admin, payer, and initial LP.
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
    /// Reuse an existing collateral mint. Omitted, a fresh 6-decimal test mint is created.
    #[arg(long)]
    usdc_mint: Option<Pubkey>,
    /// List all 33 markets rather than the 5-market starter set.
    #[arg(long)]
    all: bool,
    /// List only these symbols, comma separated — `--markets ETH/USD,SOL/USD`.
    ///
    /// Drawn from the same 33-market table as `--all`, so a symbol that is not in that table
    /// is refused rather than guessed at. This is how a market is added to a venue that is
    /// already live: the market set is *data*, so extending it needs no program change, and
    /// naming the two you want avoids listing 27 others whose feeds a free plan cannot read.
    #[arg(long, value_delimiter = ',')]
    markets: Vec<String>,
    /// Test USDC minted to the admin.
    #[arg(long, default_value_t = 10_000_000)]
    mint_amount: u64,
    /// Seed the LP pool with this much, so trades have a counterparty.
    #[arg(long, default_value_t = 1_000_000)]
    lp_seed: u64,
    /// Where to write the deployment summary.
    #[arg(long, default_value = "deployment.json")]
    out: PathBuf,
    /// Print the plan and send nothing.
    #[arg(long)]
    dry_run: bool,

    /// Send the listing and activation transactions. Without it this command **verifies and
    /// stops**: it prints what each symbol resolves to and how that compares with what is
    /// already on chain, and sends nothing.
    ///
    /// The default is deliberate. `initialize_market` leaves a market `Initialized` rather
    /// than `Active` precisely so a mis-typed feed id cannot trade the instant it is listed —
    /// and this tool used to activate automatically, which threw that protection away. A
    /// market listed against the wrong feed cannot be repaired without `set_market_oracle`
    /// and a halt, so the cheapest place to catch it is a human reading this table.
    #[arg(long)]
    activate: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

/// Resolve `--markets` against both market tables, preserving the order the caller asked for.
///
/// Both, because neither is a superset of the other: `USD/CNH` is in [`STARTER`] and not in
/// [`ALL`]. Searching only one silently refuses a symbol this tool can demonstrably list.
///
/// An unknown symbol is an error, not a skip. A typo that silently listed nothing would look
/// exactly like a successful run, and the whole point of this tool printing a table is that a
/// human can see what it is about to do.
fn select_markets(requested: &[String]) -> Result<Vec<MarketSpec>> {
    let known = || ALL.iter().chain(STARTER.iter());
    let mut out = Vec::with_capacity(requested.len());
    for want in requested {
        let want = want.trim();
        match known().find(|m| m.symbol.eq_ignore_ascii_case(want)) {
            Some(spec) => out.push(*spec),
            None => {
                let mut names: Vec<&str> = known().map(|m| m.symbol).collect();
                names.sort_unstable();
                names.dedup();
                bail!(
                    "unknown market {want:?}. The {} listable symbols are:\n  {}",
                    names.len(),
                    names.join(", ")
                )
            }
        }
    }
    // Two spellings of one symbol would try to list it twice at two indices.
    for (i, a) in out.iter().enumerate() {
        if out
            .iter()
            .skip(i.saturating_add(1))
            .any(|b| b.symbol == a.symbol)
        {
            bail!("market {:?} was requested more than once", a.symbol);
        }
    }
    Ok(out)
}

async fn run(args: Args) -> Result<()> {
    let admin = load_keypair(&args.keypair)?;
    let rpc = RpcClient::new_with_commitment(args.rpc_url.clone(), CommitmentConfig::confirmed());

    if args.all && !args.markets.is_empty() {
        bail!("--all and --markets are mutually exclusive: pass one or the other");
    }
    // Held outside the `if` so the borrow in `markets` outlives it.
    let requested: Vec<MarketSpec>;
    let markets: &[MarketSpec] = if args.markets.is_empty() {
        if args.all {
            ALL
        } else {
            STARTER
        }
    } else {
        requested = select_markets(&args.markets)?;
        &requested
    };

    println!("init-protocol");
    println!("  rpc      {}", args.rpc_url);
    println!("  admin    {}", admin.pubkey());
    println!("  program  {}", solfx_core::ID);
    println!("  markets  {}\n", markets.len());

    // Fail here rather than three transactions in, when the cause is no longer obvious.
    if rpc.get_account(&solfx_core::ID).await.is_err() {
        bail!(
            "solfx_core is not deployed at {} on this cluster.\n\
             Run scripts/deploy-localnet.sh (localnet) or anchor deploy (devnet) first.",
            solfx_core::ID
        );
    }
    let balance = rpc.get_balance(&admin.pubkey()).await.unwrap_or(0);
    // Printed as integer lamports-per-billion rather than a float: the workspace denies
    // floating-point arithmetic, and a balance is money.
    println!(
        "  admin balance {}.{:09} SOL",
        balance.checked_div(1_000_000_000).unwrap_or(0),
        balance.checked_rem(1_000_000_000).unwrap_or(0)
    );
    if balance == 0 && !args.dry_run {
        bail!("admin has no SOL on this cluster");
    }

    // Shared resolver — see `pyth::resolve_feed_ids`.
    let symbols: Vec<&str> = markets.iter().map(|m| m.symbol).collect();
    let feeds: HashMap<String, [u8; 32]> =
        pyth::resolve_feed_ids(&args.hermes_url, args.hermes_token.as_deref(), &symbols)
            .await?
            .into_iter()
            .filter_map(|(sym, f)| {
                let mut bytes = [0u8; 32];
                hex::decode_to_slice(f.feed_id.trim_start_matches("0x"), &mut bytes)
                    .ok()
                    .map(|()| (sym, bytes))
            })
            .collect();
    println!("  resolved {} of {} feed ids\n", feeds.len(), markets.len());

    let p = Pdas::derive();

    // --- the collateral mint -------------------------------------------------------------
    //
    // Order matters. The protocol stores the mint it was initialised with and rejects any
    // other with `WrongCollateralMint`, so a re-run that mints a *fresh* one before checking
    // whether the protocol already exists produces a mint nothing on chain will accept — the
    // tool reports success on every step up to `add_liquidity`, which then fails on an error
    // that names the mint rather than the re-run that caused it.
    //
    // So: if the protocol is already there, adopt its mint. That is what makes this command
    // idempotent in the sense that matters — not "does not crash on a second run", but
    // "a second run converges on the same state as the first".
    let existing_protocol = match rpc.get_account_data(&p.protocol).await {
        Ok(data) => solfx_core::state::Protocol::try_deserialize(&mut data.as_slice()).ok(),
        Err(_) => None,
    };

    let usdc_mint = match args.usdc_mint {
        Some(m) => {
            println!("using existing mint {m}");
            m
        }
        None if existing_protocol.is_some() => {
            // `is_some` is checked above, so the expect-free unwrap here is a match arm.
            let mint = existing_protocol
                .as_ref()
                .map(|pr| pr.usdc_mint)
                .unwrap_or_default();
            println!("adopting the protocol's configured mint {mint}");
            mint
        }
        None if args.dry_run => {
            println!("[dry-run] would create a 6-decimal test mint");
            Pubkey::default()
        }
        None => {
            let mint = create_test_mint(&rpc, &admin).await?;
            println!("created test mint {mint}");
            mint_to_admin(
                &rpc,
                &admin,
                &mint,
                args.mint_amount.saturating_mul(ONE_USDC),
            )
            .await?;
            println!("  minted {} test USDC to admin", args.mint_amount);
            mint
        }
    };

    // --- the protocol --------------------------------------------------------------------
    if existing_protocol.is_some() {
        println!(
            "\nprotocol already initialised at {} — skipping",
            p.protocol
        );
    } else if args.dry_run {
        println!("\n[dry-run] would initialize_protocol at {}", p.protocol);
    } else {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::InitializeProtocol {
                admin: admin.pubkey(),
                protocol: p.protocol,
                usdc_mint,
                collateral_vault: p.collateral_vault,
                fee_vault: p.fee_vault,
                insurance_fund: p.insurance_fund,
                insurance_vault: p.insurance_vault,
                lp_pool: p.lp_pool,
                lp_vault: p.lp_vault,
                lp_mint: p.lp_mint,
                token_program: spl_token_id(),
                system_program: anchor_lang::system_program::ID,
                rent: RENT_SYSVAR,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::InitializeProtocol {
                params: protocol_params(),
            }
            .data(),
        };
        let sig = send(&rpc, &admin, vec![ix], 300_000).await?;
        println!("\ninitialize_protocol  {sig}");
    }

    // --- the markets ---------------------------------------------------------------------
    //
    // Markets are reconciled **by symbol, read back from the account**, never by position in
    // `markets`. That distinction is the whole point of this block.
    //
    // The previous version derived `market_index` from the array index. Editing the market
    // list therefore did not add markets — it re-pointed names at the accounts that already
    // occupied those indices, and because `pyth_feed_id` and `symbol` are write-once (set only
    // by `initialize_market`; `UpdateRiskParams` carries neither) the follow-up "risk params
    // refreshed" pass could not correct them. The result on devnet was three markets whose
    // stored feed did not match their name, including one silver market pointed at a Bitcoin
    // funding rate. Nothing warned, because nothing compared.
    //
    // So: read every existing market, key by `Market::symbol_str()`, and refuse to touch any
    // market whose stored feed differs from the resolved one.
    println!("\nreconciling markets against chain state:");

    let num_markets = existing_protocol.as_ref().map_or(0, |pr| pr.num_markets);
    let existing_pdas: Vec<Pubkey> = (0..num_markets)
        .map(|i| Pubkey::find_program_address(&[MARKET_SEED, &i.to_le_bytes()], &solfx_core::ID).0)
        .collect();

    // `getMultipleAccounts` accepts at most 100 keys per request.
    let mut on_chain: HashMap<String, (u16, Pubkey, [u8; 32], MarketStatus)> = HashMap::new();
    let mut duplicates: Vec<(String, u16, u16)> = Vec::new();
    let mut all_on_chain: Vec<(String, u16, MarketStatus)> = Vec::new();
    for chunk in existing_pdas.chunks(100) {
        let fetched = rpc
            .get_multiple_accounts(chunk)
            .await
            .context("reading existing markets")?;
        for (pda, maybe) in chunk.iter().zip(fetched) {
            let Some(account) = maybe else { continue };
            let Ok(m) = solfx_core::state::Market::try_deserialize(&mut account.data.as_slice())
            else {
                continue;
            };
            let symbol = m.symbol_str().to_string();
            // Two markets can carry the same symbol — index 4 and index 5 both say "BTC/USD"
            // on devnet, because the first was listed against the wrong feed and replaced
            // rather than repaired. Overwriting silently would hide that, so record it.
            if let Some((prior, ..)) = on_chain.get(&symbol) {
                duplicates.push((symbol.clone(), *prior, m.market_index));
            }
            all_on_chain.push((symbol.clone(), m.market_index, m.status));
            on_chain.insert(symbol, (m.market_index, *pda, m.pyth_feed_id, m.status));
        }
    }

    let mut next_index = num_markets;
    let mut listed: Vec<serde_json::Value> = Vec::new();
    let mut mismatches = 0usize;

    for spec in markets {
        let Some(feed_id) = feeds.get(spec.symbol) else {
            println!("  {:<10} SKIPPED — no Hermes feed", spec.symbol);
            continue;
        };
        let want = hex::encode(feed_id);

        // ---- already listed --------------------------------------------------------------
        if let Some((index, market_pda, have_feed, status)) = on_chain.get(spec.symbol).copied() {
            let have = hex::encode(have_feed);
            if have != want {
                // Do not send anything. The feed cannot be changed by the retune below, so a
                // transaction here would only make the account look freshly maintained while
                // still pricing the wrong instrument.
                mismatches = mismatches.saturating_add(1);
                println!(
                    "  {:<10} [{index}] *** FEED MISMATCH — untouched ***",
                    spec.symbol
                );
                println!("             on chain {have}");
                println!("             resolved {want}");
                println!(
                    "             repair:  halt the market, then set_market_oracle \
                     --market-index {index} --expected {have} --new {want}"
                );
                listed.push(serde_json::json!({
                    "market_index": index,
                    "symbol": spec.symbol,
                    "market_pda": market_pda.to_string(),
                    "feed_id": have,
                    "resolved_feed_id": want,
                    "healthy": false,
                    "listed": true,
                    "reason": "stored feed does not match the resolved feed",
                }));
                continue;
            }

            listed.push(serde_json::json!({
                "market_index": index,
                "symbol": spec.symbol,
                "market_pda": market_pda.to_string(),
                "feed_id": have,
                "healthy": true,
                "listed": true,
            }));

            if !args.activate || args.dry_run {
                println!(
                    "  {:<10} [{index}] listed, feed OK, status {status:?}",
                    spec.symbol
                );
                continue;
            }

            // Retune regardless of status: a market listed by an older build still carries
            // whatever bounds that build computed, and "active" is no evidence they are right.
            let retune = Instruction {
                program_id: solfx_core::ID,
                accounts: solfx_core::accounts::AdminMarket {
                    admin: admin.pubkey(),
                    protocol: p.protocol,
                    market: market_pda,
                }
                .to_account_metas(None),
                data: solfx_core::instruction::UpdateMarketRiskParams {
                    params: risk_params(spec),
                }
                .data(),
            };
            match send(&rpc, &admin, vec![retune], 60_000).await {
                Ok(_) => println!(
                    "  {:<10} [{index}] risk params refreshed (min size {})",
                    spec.symbol,
                    contracts::min_position_base(spec.symbol)
                ),
                Err(e) => println!("  {:<10} [{index}] retune failed: {e:#}", spec.symbol),
            }

            if status == MarketStatus::Active {
                continue;
            }
            let activate = Instruction {
                program_id: solfx_core::ID,
                accounts: solfx_core::accounts::AdminMarket {
                    admin: admin.pubkey(),
                    protocol: p.protocol,
                    market: market_pda,
                }
                .to_account_metas(None),
                data: solfx_core::instruction::SetMarketStatus {
                    new_status: MarketStatus::Active,
                }
                .data(),
            };
            match send(&rpc, &admin, vec![activate], 60_000).await {
                Ok(sig) => println!("  {:<10}      active {sig}", ""),
                Err(e) => println!("  {:<10}      ACTIVATION FAILED: {e:#}", ""),
            }
            continue;
        }

        // ---- not listed yet --------------------------------------------------------------
        //
        // A new market always takes the next free index. `initialize_market` enforces
        // `market_index == protocol.num_markets`, so an existing index can never be reused
        // even if this tool were wrong about which are taken.
        let market_index = next_index;
        let market_pda = Pubkey::find_program_address(
            &[MARKET_SEED, &market_index.to_le_bytes()],
            &solfx_core::ID,
        )
        .0;

        // `listed` records whether the account actually exists. A verification pass must not
        // leave a deployment file claiming markets it only *planned* to create — a client
        // reading it would derive PDAs for accounts that were never funded.
        listed.push(serde_json::json!({
            "market_index": market_index,
            "symbol": spec.symbol,
            "market_pda": market_pda.to_string(),
            "feed_id": want,
            "healthy": true,
            "listed": args.activate && !args.dry_run,
        }));

        if !args.activate || args.dry_run {
            println!(
                "  {:<10} [{market_index}] NEW — would list at {market_pda}",
                spec.symbol
            );
            next_index = next_index.saturating_add(1);
            continue;
        }

        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::InitializeMarket {
                admin: admin.pubkey(),
                protocol: p.protocol,
                market: market_pda,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::InitializeMarket {
                market_index,
                params: market_params(spec, *feed_id),
            }
            .data(),
        };
        match send(&rpc, &admin, vec![ix], 120_000).await {
            Ok(sig) => println!("  {:<10} [{market_index}] listed {sig}", spec.symbol),
            Err(e) => {
                println!("  {:<10} [{market_index}] FAILED: {e:#}", spec.symbol);
                continue;
            }
        }
        next_index = next_index.saturating_add(1);

        let activate = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::AdminMarket {
                admin: admin.pubkey(),
                protocol: p.protocol,
                market: market_pda,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::SetMarketStatus {
                new_status: MarketStatus::Active,
            }
            .data(),
        };
        match send(&rpc, &admin, vec![activate], 60_000).await {
            Ok(sig) => println!("  {:<10}      active {sig}", ""),
            Err(e) => println!("  {:<10}      ACTIVATION FAILED: {e:#}", ""),
        }
    }

    // Markets that exist on chain but are not in the requested set. They are not errors —
    // an operator may be listing a subset — but they are indistinguishable from abandoned
    // mis-listings unless they are named, and an unpriceable Active market is a trap for any
    // client that enumerates `0..num_markets` rather than reading this file.
    let orphans: Vec<&(String, u16, MarketStatus)> = all_on_chain
        .iter()
        .filter(|(sym, ..)| !markets.iter().any(|m| m.symbol == sym))
        .collect();
    if !orphans.is_empty() {
        println!("\n  on chain but not in this market set:");
        for (sym, index, status) in orphans {
            println!("    {sym:<10} [{index}] status {status:?}");
        }
        println!(
            "    These are not offered by this deployment file. Halt any that are still \
             Active — an enumerating client would otherwise find them."
        );
    }

    if !duplicates.is_empty() {
        println!("\n  *** duplicate symbols on chain ***");
        for (sym, first, second) in &duplicates {
            println!("    {sym:<10} at [{first}] and [{second}]");
        }
        println!(
            "    A symbol resolves to exactly one market here — the higher index wins. The \
             lower one is a replaced mis-listing and should be halted."
        );
    }

    if mismatches > 0 {
        println!(
            "\n  {mismatches} market(s) hold a feed that is not the one resolved for their \
             symbol. They were left untouched and are marked \"healthy\": false in the \
             deployment file — a client must not offer them."
        );
    }
    if !args.activate {
        println!(
            "\nVerification pass only — no transaction was sent. The deployment file below is \
             still refreshed, because it is written from what was just read on chain. Re-run \
             with --activate once the table above is right."
        );
    }

    // `--activate` gates this too. It used to sit outside the gate, so a verification run
    // printed "nothing was sent" and then sent an `add_liquidity` — and because the seed is
    // unconditional rather than idempotent, every re-run stacked another million test USDC
    // onto the pool. A verify pass must be readable without changing what it reports on.
    if args.lp_seed > 0 && args.activate && !args.dry_run && args.usdc_mint.is_none() {
        println!("\nseeding LP pool with {} test USDC", args.lp_seed);
        match seed_lp(
            &rpc,
            &admin,
            &p,
            usdc_mint,
            args.lp_seed.saturating_mul(ONE_USDC),
        )
        .await
        {
            Ok(sig) => println!("  add_liquidity  {sig}"),
            Err(e) => println!("  add_liquidity FAILED: {e:#}"),
        }
    }

    // --- the summary ---------------------------------------------------------------------
    let summary = serde_json::json!({
        "rpc_url": args.rpc_url,
        "program_id": solfx_core::ID.to_string(),
        "admin": admin.pubkey().to_string(),
        "usdc_mint": usdc_mint.to_string(),
        "protocol": p.protocol.to_string(),
        "collateral_vault": p.collateral_vault.to_string(),
        "lp_pool": p.lp_pool.to_string(),
        "lp_vault": p.lp_vault.to_string(),
        "lp_mint": p.lp_mint.to_string(),
        "insurance_fund": p.insurance_fund.to_string(),
        "fee_vault": p.fee_vault.to_string(),
        "markets": listed,
    });
    std::fs::write(&args.out, serde_json::to_string_pretty(&summary)?)?;
    println!("\nwrote {}", args.out.display());
    println!("\nNext: start the price poster, or every trade fails on oracle staleness:");
    println!(
        "  cargo run -p solfx-keeper --bin price-poster -- --rpc-url {}",
        args.rpc_url
    );
    Ok(())
}

// --- parameters ----------------------------------------------------------------------------

fn protocol_params() -> InitializeProtocolParams {
    InitializeProtocolParams {
        // On a test cluster the admin holds every role. On devnet or mainnet the guardian is
        // a separate hot key whose only power is to stop trading (ADR-008).
        guardian: Pubkey::default(),
        fee_split_lp_bps: 6_000,
        fee_split_treasury_bps: 2_000,
        fee_split_insurance_bps: 1_500,
        fee_split_referral_bps: 500,
        // Zero on a test cluster so LP behaviour can be exercised without waiting. Production
        // needs a real cooldown: it is the anti-JIT defence (threat T6).
        lp_withdrawal_cooldown_seconds: 0,
        lp_exit_fee_bps: 10,
        lp_performance_fee_bps: 1_000,
        insurance_target_balance: 100_000 * ONE_USDC,
    }
}

/// The risk envelope alone, for retuning a market that is already listed.
fn risk_params(spec: &MarketSpec) -> UpdateRiskParams {
    let (_, max_leverage, imr_bps, mmr_bps, max_conf_bps, liq_conf_bps, _) = TIERS
        .iter()
        .find(|(t, ..)| *t == spec.tier)
        .copied()
        .unwrap_or((Tier::Major, 50, 200, 100, 15, 300, 2));
    UpdateRiskParams {
        max_leverage,
        imr_bps,
        mmr_bps,
        liquidation_fee_bps: 50,
        max_oi_long: 1_000_000 * ONE_USDC,
        max_oi_short: 1_000_000 * ONE_USDC,
        max_position_size: contracts::max_position_base(spec.symbol),
        min_position_size: contracts::min_position_base(spec.symbol),
        max_staleness_seconds: 60,
        max_conf_bps,
        liquidation_max_conf_bps: liq_conf_bps,
        max_deviation_bps: 1_000,
        weekend_max_leverage: 0,
        weekend_oi_cap_bps: 0,
        weekend_max_conf_bps: 0,
    }
}

fn market_params(spec: &MarketSpec, feed_id: [u8; 32]) -> InitializeMarketParams {
    let (_, max_leverage, imr_bps, mmr_bps, max_conf_bps, liq_conf_bps, base_spread_bps) = TIERS
        .iter()
        .find(|(t, ..)| *t == spec.tier)
        .copied()
        .unwrap_or((Tier::Major, 50, 200, 100, 15, 300, 2));

    let continuous = spec.tier == Tier::Crypto;

    InitializeMarketParams {
        symbol: spec.symbol.to_string(),
        feed_kind: if continuous {
            FeedKind::Crypto
        } else {
            FeedKind::SpotFx
        },
        price_source: PriceSource::Direct,
        pyth_feed_id: feed_id,
        secondary_feed_id: [0u8; 32],
        quote_conversion_feed: [0u8; 32],
        quote_conversion_kind: QuoteConversionKind::None,

        // Interbank week: Sunday 21:00 UTC to Friday 21:00 UTC. Crypto is continuous, and a
        // session window covering the whole week is how that is expressed.
        session_open_dow: 0,
        session_open_seconds: if continuous { 0 } else { 75_600 },
        session_close_dow: if continuous { 6 } else { 5 },
        session_close_seconds: if continuous { 86_399 } else { 75_600 },

        weekend_max_leverage: 0,
        weekend_oi_cap_bps: 0,
        weekend_spread_bps: 0,
        weekend_max_conf_bps: 0,

        max_leverage,
        imr_bps,
        mmr_bps,
        liquidation_fee_bps: 50,
        max_oi_long: 1_000_000 * ONE_USDC,
        max_oi_short: 1_000_000 * ONE_USDC,
        max_position_size: contracts::max_position_base(spec.symbol),
        // A base-unit floor is asset-specific and therefore nearly useless as a *value*
        // floor: a sane minimum for EUR/USD is orders of magnitude wrong for gold. The real
        // money floor is `pricing::validate_notional`. This only stops dust.
        min_position_size: contracts::min_position_base(spec.symbol),

        // 60s rather than the test harness's 10s: a real cluster has real latency, and the
        // price poster runs on a 10s cycle.
        max_staleness_seconds: 60,
        max_conf_bps,
        liquidation_max_conf_bps: liq_conf_bps,
        max_deviation_bps: 1_000,

        base_spread_bps,
        conf_spread_multiplier_bps: 10_000,
        skew_impact_bps_per_unit: 1,

        // 1 bp at RATE_PRECISION (1e9).
        open_fee_rate: 100_000,
        close_fee_rate: 100_000,
        // 0.1% per hour ceiling, matching the reference market. Carry is zero on a test
        // cluster: a non-zero carry rate makes every position drift while you are trying to
        // reason about whether a fill was priced correctly.
        funding_rate_cap_per_hour: 1_000_000,
        carry_rate_per_hour: 0,
    }
}

// --- PDAs ----------------------------------------------------------------------------------

struct Pdas {
    protocol: Pubkey,
    collateral_vault: Pubkey,
    fee_vault: Pubkey,
    insurance_fund: Pubkey,
    insurance_vault: Pubkey,
    lp_pool: Pubkey,
    lp_vault: Pubkey,
    lp_mint: Pubkey,
}

impl Pdas {
    fn derive() -> Self {
        let one = |seed: &[u8]| Pubkey::find_program_address(&[seed], &solfx_core::ID).0;
        Self {
            protocol: one(PROTOCOL_SEED),
            collateral_vault: one(COLLATERAL_VAULT_SEED),
            fee_vault: one(FEE_VAULT_SEED),
            insurance_fund: one(INSURANCE_FUND_SEED),
            insurance_vault: one(INSURANCE_VAULT_SEED),
            lp_pool: one(LP_POOL_SEED),
            lp_vault: one(LP_VAULT_SEED),
            lp_mint: one(LP_MINT_SEED),
        }
    }
}

// --- SPL token plumbing ---------------------------------------------------------------------
//
// These instructions are built by hand rather than with `spl-token`'s and
// `spl-associated-token-account-client`'s builders. Those crates vendor their own
// `solana-pubkey` and `solana-instruction` versions, which do not match the ones this crate
// resolves, so every call site would need a conversion shim and the shim itself would break
// on the next version bump. The wire formats below are stable and tiny.

/// SPL Token program.
const TOKEN_PROGRAM: Pubkey = solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Associated Token Account program.
const ATA_PROGRAM: Pubkey = solana_pubkey::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/// `spl_token::state::Mint::LEN`.
const MINT_LEN: u64 = 82;

fn spl_token_id() -> Pubkey {
    TOKEN_PROGRAM
}

/// The canonical associated token account: `[wallet, token_program, mint]`.
fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), TOKEN_PROGRAM.as_ref(), mint.as_ref()],
        &ATA_PROGRAM,
    )
    .0
}

fn meta(pubkey: Pubkey, is_signer: bool, is_writable: bool) -> solana_instruction::AccountMeta {
    solana_instruction::AccountMeta {
        pubkey,
        is_signer,
        is_writable,
    }
}

/// System program `CreateAccount` — instruction 0.
fn create_account_ix(
    from: &Pubkey,
    to: &Pubkey,
    lamports: u64,
    space: u64,
    owner: &Pubkey,
) -> Instruction {
    let mut data = 0u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: anchor_lang::system_program::ID,
        accounts: vec![meta(*from, true, true), meta(*to, true, true)],
        data,
    }
}

/// SPL Token `InitializeMint` — instruction 0, with no freeze authority.
fn initialize_mint_ix(mint: &Pubkey, authority: &Pubkey, decimals: u8) -> Instruction {
    let mut data = vec![0u8, decimals];
    data.extend_from_slice(authority.as_ref());
    data.push(0); // COption::None for the freeze authority
    Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![meta(*mint, false, true), meta(RENT_SYSVAR, false, false)],
        data,
    }
}

/// SPL Token `MintTo` — instruction 7.
fn mint_to_ix(mint: &Pubkey, dest: &Pubkey, authority: &Pubkey, amount: u64) -> Instruction {
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            meta(*mint, false, true),
            meta(*dest, false, true),
            meta(*authority, true, false),
        ],
        data,
    }
}

/// ATA `Create` (0) or `CreateIdempotent` (1).
fn create_ata_ix(funder: &Pubkey, wallet: &Pubkey, mint: &Pubkey, idempotent: bool) -> Instruction {
    Instruction {
        program_id: ATA_PROGRAM,
        accounts: vec![
            meta(*funder, true, true),
            meta(ata(wallet, mint), false, true),
            meta(*wallet, false, false),
            meta(*mint, false, false),
            meta(anchor_lang::system_program::ID, false, false),
            meta(TOKEN_PROGRAM, false, false),
        ],
        data: vec![u8::from(idempotent)],
    }
}

async fn create_test_mint(rpc: &RpcClient, admin: &Keypair) -> Result<Pubkey> {
    let mint = Keypair::new();
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(usize::try_from(MINT_LEN)?)
        .await?;
    let ixs = vec![
        create_account_ix(
            &admin.pubkey(),
            &mint.pubkey(),
            rent,
            MINT_LEN,
            &TOKEN_PROGRAM,
        ),
        initialize_mint_ix(&mint.pubkey(), &admin.pubkey(), 6),
    ];
    let blockhash = rpc.get_latest_blockhash().await?;
    let msg = Message::new(&ixs, Some(&admin.pubkey()));
    let mut tx = Transaction::new_unsigned(msg);
    tx.try_sign(&[admin, &mint], blockhash)?;
    rpc.send_and_confirm_transaction(&tx).await?;
    Ok(mint.pubkey())
}

async fn mint_to_admin(rpc: &RpcClient, admin: &Keypair, mint: &Pubkey, amount: u64) -> Result<()> {
    let dest = ata(&admin.pubkey(), mint);
    let ixs = vec![
        create_ata_ix(&admin.pubkey(), &admin.pubkey(), mint, true),
        mint_to_ix(mint, &dest, &admin.pubkey(), amount),
    ];
    send(rpc, admin, ixs, 100_000).await?;
    Ok(())
}

async fn seed_lp(
    rpc: &RpcClient,
    admin: &Keypair,
    p: &Pdas,
    mint: Pubkey,
    amount: u64,
) -> Result<String> {
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::AddLiquidity {
            provider: admin.pubkey(),
            protocol: p.protocol,
            lp_pool: p.lp_pool,
            lp_vault: p.lp_vault,
            lp_mint: p.lp_mint,
            provider_token_account: ata(&admin.pubkey(), &mint),
            provider_lp_account: ata(&admin.pubkey(), &p.lp_mint),
            token_program: TOKEN_PROGRAM,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::AddLiquidity {
            amount,
            min_lp_out: 0,
        }
        .data(),
    };
    send(
        rpc,
        admin,
        vec![
            create_ata_ix(&admin.pubkey(), &admin.pubkey(), &p.lp_mint, true),
            ix,
        ],
        200_000,
    )
    .await
}

// --- plumbing ------------------------------------------------------------------------------

async fn send(rpc: &RpcClient, payer: &Keypair, ixs: Vec<Instruction>, cu: u32) -> Result<String> {
    let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(cu)];
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
