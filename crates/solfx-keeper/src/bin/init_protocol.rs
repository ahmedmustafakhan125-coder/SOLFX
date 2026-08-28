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
/// Five markets chosen to cover *distinct code paths* rather than to be representative:
/// a USD-quoted major, a JPY cross, an EM pair with a wide confidence band, a session-bound
/// metal, and the one continuous feed class. Listing all 33 proves nothing extra about the
/// program and costs 33 transactions per iteration. `--all` lists the full set.
const STARTER: &[MarketSpec] = &[
    MarketSpec {
        symbol: "EUR/USD",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "EUR/JPY",
        tier: Tier::Major,
    },
    MarketSpec {
        symbol: "USD/INR",
        tier: Tier::Emerging,
    },
    MarketSpec {
        symbol: "XAU/USD",
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
    #[arg(long, default_value = "https://hermes.pyth.network")]
    hermes_url: String,
    /// Admin, payer, and initial LP.
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
    /// Reuse an existing collateral mint. Omitted, a fresh 6-decimal test mint is created.
    #[arg(long)]
    usdc_mint: Option<Pubkey>,
    /// List all 33 markets rather than the 5-market starter set.
    #[arg(long)]
    all: bool,
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

    let markets: &[MarketSpec] = if args.all { ALL } else { STARTER };

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

    let feeds = resolve_feed_ids(&args.hermes_url, markets).await?;
    println!("  resolved {} of {} feed ids\n", feeds.len(), markets.len());

    let p = Pdas::derive();

    // --- the collateral mint -------------------------------------------------------------
    let usdc_mint = match args.usdc_mint {
        Some(m) => {
            println!("using existing mint {m}");
            m
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
    if rpc.get_account(&p.protocol).await.is_ok() {
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
    println!("\nlisting markets:");
    let mut listed: Vec<serde_json::Value> = Vec::new();
    for (index, spec) in markets.iter().enumerate() {
        let market_index = u16::try_from(index).map_err(|_| anyhow!("too many markets"))?;
        let market_pda = Pubkey::find_program_address(
            &[MARKET_SEED, &market_index.to_le_bytes()],
            &solfx_core::ID,
        )
        .0;

        let Some(feed_id) = feeds.get(spec.symbol) else {
            println!("  {:<16} SKIPPED — no Hermes feed", spec.symbol);
            continue;
        };

        listed.push(serde_json::json!({
            "market_index": market_index,
            "symbol": spec.symbol,
            "market_pda": market_pda.to_string(),
            "feed_id": hex::encode(feed_id),
        }));

        if let Ok(data) = rpc.get_account_data(&market_pda).await {
            // Already listed. It may still be sitting in `Initialized` from an earlier run
            // that stopped short of activating it, so check rather than assume.
            let live = solfx_core::state::Market::try_deserialize(&mut data.as_slice())
                .map(|m| m.status == MarketStatus::Active)
                .unwrap_or(false);
            // Retune regardless of status. An already-active market listed by an older build
            // still carries whatever bounds that build computed, and "active" is no evidence
            // they are right — skipping here is how a broken envelope survives a re-run.
            // Retune first. A market listed by an older build may carry position bounds
            // derived from the FX lot for every asset, which makes the *minimum* BTC
            // position ~0.1 BTC and rejects any sane order as too small.
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
                    "  {:<16} [{market_index}] risk params refreshed (min size {})",
                    spec.symbol,
                    contracts::min_position_base(spec.symbol)
                ),
                Err(e) => println!(
                    "  {:<16} [{market_index}] retune failed: {e:#}",
                    spec.symbol
                ),
            }
            if live {
                continue;
            }
            println!("  {:<16} [{market_index}] activating", spec.symbol);
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
                Ok(sig) => println!("  {:<16}      active {sig}", ""),
                Err(e) => println!("  {:<16}      ACTIVATION FAILED: {e:#}", ""),
            }
            continue;
        }
        if args.dry_run {
            println!(
                "  {:<16} [{market_index}] would list at {market_pda}",
                spec.symbol
            );
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
            Ok(sig) => println!("  {:<16} [{market_index}] listed {sig}", spec.symbol),
            Err(e) => {
                println!("  {:<16} [{market_index}] FAILED: {e:#}", spec.symbol);
                continue;
            }
        }

        // `initialize_market` deliberately leaves the market `Initialized`, not `Active`:
        // a mis-typed feed id or risk parameter must not be tradeable the instant it is
        // listed. Going live is a separate, explicit decision, so make it explicitly.
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
            Ok(sig) => println!("  {:<16}      active {sig}", ""),
            Err(e) => println!("  {:<16}      ACTIVATION FAILED: {e:#}", ""),
        }
    }

    // --- seed the pool -------------------------------------------------------------------
    // Without liquidity the LP pool cannot be the counterparty, so every open_position fails
    // on `InsufficientPoolLiquidity` — a confusing first experience of a working protocol.
    if args.lp_seed > 0 && !args.dry_run && args.usdc_mint.is_none() {
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

async fn resolve_feed_ids(
    hermes: &str,
    markets: &[MarketSpec],
) -> Result<HashMap<String, [u8; 32]>> {
    #[derive(serde::Deserialize)]
    struct Entry {
        id: String,
        attributes: Attrs,
    }
    #[derive(serde::Deserialize)]
    struct Attrs {
        #[serde(default)]
        display_symbol: Option<String>,
        #[serde(default)]
        symbol: Option<String>,
        #[serde(default)]
        asset_type: Option<String>,
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let catalogue: Vec<Entry> = http
        .get(format!("{}/v2/price_feeds", hermes.trim_end_matches('/')))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("parsing the Hermes catalogue")?;

    let want: HashMap<String, &str> = markets
        .iter()
        .map(|m| (m.symbol.to_uppercase(), m.symbol))
        .collect();
    let mut best: HashMap<String, (String, String)> = HashMap::new();

    for entry in catalogue {
        let asset_type = entry.attributes.asset_type.clone().unwrap_or_default();
        let mut names = Vec::new();
        if let Some(d) = &entry.attributes.display_symbol {
            names.push(d.to_uppercase());
        }
        if let Some(s) = &entry.attributes.symbol {
            names.push(s.rsplit('.').next().unwrap_or(s).to_uppercase());
        }
        for name in names {
            let Some(original) = want.get(&name) else {
                continue;
            };
            let better = match best.get(*original) {
                None => true,
                Some((_, ty)) => ty == "Crypto" && asset_type != "Crypto",
            };
            if better {
                best.insert(
                    (*original).to_string(),
                    (entry.id.clone(), asset_type.clone()),
                );
            }
        }
    }

    let mut out = HashMap::new();
    for (symbol, (id, _)) in best {
        let mut bytes = [0u8; 32];
        if hex::decode_to_slice(id.trim_start_matches("0x"), &mut bytes).is_ok() {
            out.insert(symbol, bytes);
        }
    }
    Ok(out)
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
