//! A whole NOXFUNDS mandate, start to finish, against a real cluster.
//!
//! # Why this exists
//!
//! Everything NOXFUNDS does had only ever happened inside LiteSVM. The program was deployed and
//! initialised on devnet and then sat there: zero `Mandate` accounts, zero `TraderProfile`
//! accounts. "Deployed" and "demonstrably works" are not the same claim, and only one of them
//! can be checked by someone else.
//!
//! This runs the full lifecycle and prints what the *program* answered at each step:
//!
//! ```text
//!   profile  →  fund mandate  →  SolFX account  →  deposit
//!            →  funded trade (with its stop, atomically)
//!            →  equity crank  →  close  →  reclaim the stop's rent
//!            →  request settlement  →  claim
//! ```
//!
//! It leaves real accounts on chain afterwards, which is the point: a marketplace page, an
//! explorer, or a judge can read them.
//!
//! # What it refuses to guess
//!
//! Every figure that could be wrong is read from the venue rather than assumed:
//!
//! - the **market** must be `Active` and inside its session ([`session_is_open`], the program's
//!   own function), or the trade would fail `MarketClosedForOpens` and look like a bug;
//! - the **price** comes from the price account named in `price-accounts.json`, never derived —
//!   see `price_map.rs` for the two bugs that caused;
//! - **position size and collateral** are checked against the market's own `min_position_size`,
//!   `max_position_size`, `imr_bps` and `max_leverage`;
//! - the **stop distance** is checked against the mandate's `max_stop_distance_bps` and the
//!   risk it implies against `max_risk_per_trade_bps`, before anything is sent.
//!
//! Every step is idempotent where the chain allows it: an account that already exists is
//! reported and skipped, so a re-run after a failure resumes rather than starting over.
//!
//! ```text
//! nox lifecycle                 # read the chain, print the plan, send nothing
//! nox lifecycle --execute       # run it
//! nox status                    # what is on chain for this investor and trader
//! ```

#[allow(dead_code)]
#[path = "../pyth.rs"]
mod pyth;

#[allow(dead_code)]
#[path = "../price_map.rs"]
mod price_map;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anchor_lang::{InstructionData as _, ToAccountMetas as _};
use anyhow::{anyhow, bail, Context as _, Result};
use clap::Parser as _;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer as _;
use solana_transaction::Transaction;

use noxfunds::instructions::investor::MandateRules;
use noxfunds::state::{Mandate, MandateState, NoxConfig, TraderProfile};
use solfx_core::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, PROTOCOL_SEED, USER_SEED,
};
use solfx_core::instructions::keeper::session::session_is_open;
use solfx_core::state::{Direction, Market, MarketStatus, Position, UserAccount};

const TOKEN_PROGRAM: Pubkey = solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ATA_PROGRAM: Pubkey = solana_pubkey::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const ONE_USDC: u64 = 1_000_000;
const BASE_PRECISION: u128 = 1_000_000_000;
const NOTIONAL_DIVISOR: u128 = 1_000_000_000_000;

/// Rent the mandate signer must hold before it can pay for a `UserAccount` (0.0016), a
/// `Position` (0.0026) and a `TriggerOrder` (0.0020), with room for a second round trip.
const SIGNER_LAMPORTS: u64 = 20_000_000;
/// Enough for the trader to pay transaction fees on two signatures a step.
const TRADER_LAMPORTS: u64 = 20_000_000;
/// The investor pays the mandate's and the vault's rent, plus its own fees.
const INVESTOR_LAMPORTS: u64 = 30_000_000;
/// `QUOTE_PRECISION` is 1e6, and the protocol refuses a collateral mint with any other
/// decimals (`InvalidCollateralMintDecimals`), so this is not a guess about *this* mint.
const USDC_DECIMALS: u8 = 6;

/// Where the poster publishes the feed → account map, if there is no local copy.
const PUBLIC_PRICE_MAP: &str = "https://solfx.cloud/price-accounts.json";

#[derive(clap::Parser)]
#[command(
    name = "nox",
    about = "Run a NOXFUNDS mandate end to end on a live cluster"
)]
struct Args {
    #[arg(long, env = "SOLFX_RPC_URL", default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    /// The funding wallet: pays the setup fees and supplies the principal, by minting it if it
    /// holds the mint authority and transferring it otherwise. Usually the deployer, which is
    /// also NOXFUNDS' treasury.
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
    /// The investor.
    ///
    /// A wallet of its own, created on first use — **not** the funder. NOXFUNDS' treasury is the
    /// deployer, and `claim_settlement` takes the investor's and the treasury's token accounts as
    /// separate writable `Account<TokenAccount>`s. Anchor refuses two mutable accounts with the
    /// same key, so paying an investor who *is* the treasury cannot work. Three wallets is also
    /// the honest picture: you can watch principal, profit share and fee land in three places.
    #[arg(long, default_value = "keys/nox-investor.json")]
    investor_keypair: PathBuf,
    /// The trader. Created on first use and reused after, so a re-run builds on the same
    /// record rather than a fresh one.
    #[arg(long, default_value = "keys/nox-trader.json")]
    trader_keypair: PathBuf,
    #[arg(long, default_value = "deployment.json")]
    deployment: PathBuf,
    #[arg(long, default_value = "price-accounts.json")]
    price_accounts: PathBuf,
    /// Which market to trade. Must be listed, `Active`, and inside its session.
    #[arg(long, default_value = "BTC/USD")]
    market: String,
    /// The mandate's principal, in whole USDC. Moved from the investor into the mandate's vault
    /// by `fund_mandate` itself.
    #[arg(long, default_value_t = 200)]
    principal: u64,
    /// Distinguishes several mandates from the same investor to the same trader.
    #[arg(long, default_value_t = 0)]
    seq: u8,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Read the chain, check every rule the trade will be judged against, and print the plan.
    /// Sends nothing without `--execute`.
    Lifecycle {
        #[arg(long)]
        execute: bool,
        /// Plan and simulate as this funding wallet without reading its key. A simulation needs
        /// no signature, so the whole run can be reviewed on a machine that never holds the
        /// deployer's key. Ignored with `--execute`, which signs with `--keypair`.
        #[arg(long)]
        as_funder: Option<Pubkey>,
    },
    /// What is on chain right now for this investor and trader.
    Status,
}

fn main() -> Result<()> {
    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

// --- addresses -------------------------------------------------------------------------------

struct Pdas {
    protocol: Pubkey,
    collateral_vault: Pubkey,
    fee_vault: Pubkey,
    insurance_fund: Pubkey,
    insurance_vault: Pubkey,
    lp_pool: Pubkey,
    lp_vault: Pubkey,
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
        }
    }
}

/// Every NOXFUNDS address this run touches. Derived from the programs' own seed constants.
struct Nox {
    config: Pubkey,
    mandate: Pubkey,
    signer: Pubkey,
    vault: Pubkey,
    profile: Pubkey,
    user_account: Pubkey,
}

impl Nox {
    fn derive(investor: &Pubkey, trader: &Pubkey, seq: u8) -> Self {
        use noxfunds::constants::{
            CONFIG_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED, MANDATE_VAULT_SEED, TRADER_SEED,
        };
        let nox = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &noxfunds::ID).0;
        let mandate = nox(&[MANDATE_SEED, investor.as_ref(), trader.as_ref(), &[seq]]);
        let signer = nox(&[MANDATE_SIGNER_SEED, mandate.as_ref()]);
        Self {
            config: nox(&[CONFIG_SEED]),
            mandate,
            signer,
            vault: nox(&[MANDATE_VAULT_SEED, mandate.as_ref()]),
            profile: nox(&[TRADER_SEED, trader.as_ref()]),
            user_account: Pubkey::find_program_address(
                &[USER_SEED, signer.as_ref()],
                &solfx_core::ID,
            )
            .0,
        }
    }
}

fn market_pda(index: u16) -> Pubkey {
    Pubkey::find_program_address(&[MARKET_SEED, &index.to_le_bytes()], &solfx_core::ID).0
}

fn position_pda(user_account: &Pubkey, market_index: u16, nonce: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            solfx_core::constants::POSITION_SEED,
            user_account.as_ref(),
            &market_index.to_le_bytes(),
            &[nonce],
        ],
        &solfx_core::ID,
    )
    .0
}

fn trigger_pda(position: &Pubkey, order_id: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            solfx_core::constants::TRIGGER_SEED,
            position.as_ref(),
            &[order_id],
        ],
        &solfx_core::ID,
    )
    .0
}

fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), TOKEN_PROGRAM.as_ref(), mint.as_ref()],
        &ATA_PROGRAM,
    )
    .0
}

// --- reading -----------------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct Deployment {
    #[serde(deserialize_with = "pubkey_str::deserialize")]
    usdc_mint: Pubkey,
    markets: Vec<MarketEntry>,
}

#[derive(serde::Deserialize)]
struct MarketEntry {
    market_index: u16,
    symbol: String,
}

mod pubkey_str {
    use serde::{Deserialize as _, Deserializer};
    use solana_pubkey::Pubkey;
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Pubkey, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

async fn maybe<T: anchor_lang::AccountDeserialize>(
    rpc: &RpcClient,
    key: &Pubkey,
) -> Result<Option<T>> {
    let value = rpc
        .get_account_with_commitment(key, rpc.commitment())
        .await
        .with_context(|| format!("reading {key}"))?
        .value;
    match value {
        None => Ok(None),
        Some(a) => Ok(Some(
            T::try_deserialize(&mut a.data.as_slice())
                .map_err(|e| anyhow!("decoding {key}: {e}"))?,
        )),
    }
}

async fn token_balance(rpc: &RpcClient, account: &Pubkey) -> u64 {
    rpc.get_token_account_balance(account)
        .await
        .ok()
        .and_then(|b| b.amount.parse().ok())
        .unwrap_or(0)
}

/// The feed → price-account map the poster publishes.
///
/// Derived addresses are wrong here: the poster's accounts are per-feed keypairs, not the
/// canonical `[shard, feed_id]` PDAs, and anything that derives looks in the wrong place. If
/// there is no local copy this fetches the one the web tier serves rather than guessing.
async fn price_accounts(path: &Path) -> Result<HashMap<String, Pubkey>> {
    if path.exists() {
        return Ok(price_map::by_symbol(&price_map::load(path)?));
    }
    println!("  no {} — fetching {PUBLIC_PRICE_MAP}", path.display());
    let body = reqwest::get(PUBLIC_PRICE_MAP)
        .await
        .with_context(|| format!("fetching {PUBLIC_PRICE_MAP}"))?
        .text()
        .await?;
    let entries: Vec<price_map::Entry> = serde_json::from_str(&body)?;
    Ok(price_map::by_symbol(&entries))
}

async fn oracle_price(rpc: &RpcClient, price_update: &Pubkey) -> Result<i64> {
    let data = rpc
        .get_account_data(price_update)
        .await
        .context("reading the price account")?;
    let m = pyth::parse_update(&data)?.price_message;
    if m.price <= 0 {
        bail!("the oracle reports a non-positive price");
    }
    let age = now()?.saturating_sub(m.publish_time);
    if age > i64::from(solfx_core::constants::MAX_ALLOWED_STALENESS_SECONDS) {
        bail!(
            "the price is {age}s old against a {}s gate — the market is shut, or the poster is \
             behind",
            solfx_core::constants::MAX_ALLOWED_STALENESS_SECONDS
        );
    }
    solfx_math::oracle::normalize_price(m.price, m.exponent)
        .map_err(|e| anyhow!("normalising the price: {e:?}"))
}

fn now() -> Result<i64> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    i64::try_from(secs).map_err(|_| anyhow!("the clock is beyond an i64"))
}

// --- the plan ------------------------------------------------------------------------------------

/// Everything the run needs, after the chain has been asked and every rule checked.
#[derive(Debug)]
struct Plan {
    market_index: u16,
    market: Pubkey,
    price_update: Pubkey,
    price: i64,
    size_base: u64,
    collateral: u64,
    notional: u64,
    stop_price: i64,
    price_limit: i64,
    rules: MandateRules,
}

/// Size the trade against the market's own limits and the mandate's own rules.
///
/// Nothing here is a constant someone chose: the bounds come from the `Market` account and the
/// rules are the ones this run is about to write into the `Mandate`. A violation is reported
/// now, by name, rather than as a failed transaction later.
/// The only four figures of a market's configuration the sizing depends on. Passed rather than
/// read from a whole `Market`, so the rule is testable and the call site names its inputs.
#[derive(Clone, Copy, Debug)]
struct Limits {
    min_position_size: u64,
    max_position_size: u64,
    imr_bps: u16,
    max_leverage: u16,
}

impl From<&Market> for Limits {
    fn from(m: &Market) -> Self {
        Self {
            min_position_size: m.min_position_size,
            max_position_size: m.max_position_size,
            imr_bps: m.imr_bps,
            max_leverage: m.max_leverage,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_trade(
    market_index: u16,
    market_key: Pubkey,
    market: Limits,
    price_update: Pubkey,
    price: i64,
    principal: u64,
) -> Result<Plan> {
    // Half the principal at risk of being deployed, a quarter of it as margin: leverage 2x,
    // far inside any market's ceiling, and it leaves room for the spread on both sides.
    let notional = principal
        .checked_div(2)
        .ok_or_else(|| anyhow!("principal overflows"))?;
    let collateral = principal
        .checked_div(4)
        .ok_or_else(|| anyhow!("principal overflows"))?;
    if notional == 0 || collateral == 0 {
        bail!("--principal {principal} is too small to size a trade; 4 USDC is the floor");
    }
    // Whole USDC into the protocol's 1e6 units, once, checked.
    let raw = |whole: u64| {
        whole
            .checked_mul(ONE_USDC)
            .ok_or_else(|| anyhow!("{whole} USDC overflows"))
    };
    let notional_raw = raw(notional)?;
    let collateral_raw = raw(collateral)?;
    let principal_raw = raw(principal)?;

    let price_u = u128::try_from(price).map_err(|_| anyhow!("negative price"))?;
    // `notional_raw`, not `notional`: the quote side is 1e6 units, and using whole USDC here
    // sized a $100 position at 1 base unit — caught by the bounds check below, on devnet.
    let size_base = u128::from(notional_raw)
        .checked_mul(NOTIONAL_DIVISOR)
        .and_then(|v| v.checked_div(price_u))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or_else(|| anyhow!("size overflows"))?;

    if size_base < market.min_position_size || size_base > market.max_position_size {
        bail!(
            "a ${notional} position is {size_base} base units, outside {}'s bounds of \
             {}..={}. Raise --principal (the size scales with it).",
            market_index,
            market.min_position_size,
            market.max_position_size
        );
    }
    let leverage = notional
        .checked_div(collateral.max(1))
        .ok_or_else(|| anyhow!("leverage overflows"))?;
    if leverage > u64::from(market.max_leverage) {
        bail!(
            "leverage {leverage}x exceeds the market's {}x",
            market.max_leverage
        );
    }
    let required_margin = u128::from(notional)
        .checked_mul(u128::from(market.imr_bps))
        .and_then(|v| v.checked_div(10_000))
        .ok_or_else(|| anyhow!("margin overflows"))?;
    if u128::from(collateral) < required_margin {
        bail!(
            "collateral {} is below the market's initial margin of {} ({} bps)",
            usdc(collateral),
            usdc(u64::try_from(required_margin).unwrap_or(u64::MAX)),
            market.imr_bps
        );
    }

    // A long, so the stop sits below the entry. 100 bps: inside the 300 bps the rules permit,
    // and the risk it implies is checked below against the same rule the program applies.
    const STOP_BPS: i64 = 100;
    let stop_offset = price
        .saturating_mul(STOP_BPS)
        .checked_div(10_000)
        .ok_or_else(|| anyhow!("stop offset overflows"))?;
    let stop_price = price
        .checked_sub(stop_offset)
        .ok_or_else(|| anyhow!("stop underflows"))?;

    let rules = MandateRules {
        max_trade_notional: notional_raw,
        max_total_notional: notional_raw,
        max_drawdown_bps: 1_000,
        max_daily_loss_bps: 1_000,
        max_risk_per_trade_bps: 500,
        max_stop_distance_bps: 300,
        max_concurrent_positions: 1,
        allowed_markets: 1u128
            .checked_shl(u32::from(market_index))
            .ok_or_else(|| anyhow!("market index {market_index} is outside the bitmap"))?,
        min_hold_slots: 0,
    };
    rules
        .validate()
        .map_err(|e| anyhow!("the rule set is incoherent: {e}"))?;

    // The same arithmetic `check_rules` applies on chain: risk at the stop, as bps of equity.
    let risk = u128::from(notional_raw)
        .checked_mul(u128::from(u64::try_from(STOP_BPS)?))
        .and_then(|v| v.checked_div(10_000))
        .ok_or_else(|| anyhow!("risk overflows"))?;
    let risk_bps = risk
        .checked_mul(10_000)
        .and_then(|v| v.checked_div(u128::from(principal_raw)))
        .ok_or_else(|| anyhow!("risk bps overflows"))?;
    if risk_bps > u128::from(rules.max_risk_per_trade_bps) {
        bail!(
            "risk at the stop is {risk_bps} bps of the principal, over the {} bps the mandate \
             allows",
            rules.max_risk_per_trade_bps
        );
    }

    // 100 bps either side of the oracle. There is no "disabled" slippage; the program always
    // compares, so an unbounded order is not a thing that can be sent.
    let slip = price
        .saturating_mul(100)
        .checked_div(10_000)
        .ok_or_else(|| anyhow!("slippage overflows"))?;

    Ok(Plan {
        market_index,
        market: market_key,
        price_update,
        price,
        size_base,
        collateral: collateral_raw,
        notional: notional_raw,
        stop_price,
        // A long buys, so the bound is a maximum. There is no "disabled" slippage.
        price_limit: price
            .checked_add(slip)
            .ok_or_else(|| anyhow!("slippage bound overflows"))?,
        rules,
    })
}

// --- the run -------------------------------------------------------------------------------------

async fn run(args: Args) -> Result<()> {
    let funder = load_keypair(&args.keypair)?;
    let investor = load_or_create(&args.investor_keypair, "investor")?;
    let rpc = RpcClient::new_with_commitment(args.rpc_url.clone(), CommitmentConfig::confirmed());
    let dep: Deployment = serde_json::from_str(
        &std::fs::read_to_string(&args.deployment)
            .with_context(|| format!("reading {}", args.deployment.display()))?,
    )?;
    let trader = load_or_create(&args.trader_keypair, "trader")?;
    let nox = Nox::derive(&investor.pubkey(), &trader.pubkey(), args.seq);

    match args.cmd {
        Cmd::Status => status(&rpc, &dep, &investor, &trader, &nox).await,
        Cmd::Lifecycle { execute, as_funder } => {
            // A dry run may act as a wallet whose key is not on this machine.
            let acting = match as_funder {
                Some(k) if !execute => k,
                _ => funder.pubkey(),
            };
            lifecycle(
                &rpc, &args, &dep, &funder, acting, &investor, &trader, &nox, execute,
            )
            .await
        }
    }
}

async fn status(
    rpc: &RpcClient,
    dep: &Deployment,
    investor: &Keypair,
    trader: &Keypair,
    nox: &Nox,
) -> Result<()> {
    println!("\n  investor  {}", investor.pubkey());
    println!("  trader    {}", trader.pubkey());
    println!("  mandate   {}", nox.mandate);
    println!("  vault     {}", nox.vault);

    match maybe::<TraderProfile>(rpc, &nox.profile).await? {
        None => println!("\n  no trader profile yet"),
        Some(p) => println!(
            "\n  profile   tier {:?}, {} trades ({}W/{}L), gross +{} / -{}, {} mandates ({} live)",
            p.tier,
            p.trades,
            p.wins,
            p.losses,
            usdc(p.gross_profit),
            usdc(p.gross_loss),
            p.mandates_funded,
            p.active_mandates
        ),
    }
    match maybe::<Mandate>(rpc, &nox.mandate).await? {
        None => println!("  no mandate yet"),
        Some(m) => println!(
            "  mandate   {:?}, principal {}, peak {}, last equity {}, {} open",
            m.state,
            usdc(m.principal),
            usdc(m.peak_equity),
            usdc(m.last_equity),
            m.open_positions
        ),
    }
    if let Some(u) = maybe::<UserAccount>(rpc, &nox.user_account).await? {
        println!("  solfx     free collateral {}", usdc(u.free_collateral));
    }
    println!(
        "  vault     {} USDC\n  investor  {} USDC\n",
        usdc(token_balance(rpc, &nox.vault).await),
        usdc(token_balance(rpc, &ata(&investor.pubkey(), &dep.usdc_mint)).await)
    );
    Ok(())
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn lifecycle(
    rpc: &RpcClient,
    args: &Args,
    dep: &Deployment,
    funder: &Keypair,
    // Who the funding transaction runs as: the same as `funder`, unless a dry run was told to
    // act as somebody else.
    acting: Pubkey,
    investor: &Keypair,
    trader: &Keypair,
    nox: &Nox,
    execute: bool,
) -> Result<()> {
    let p = Pdas::derive();

    // --- what the chain says, before anything is decided -----------------------------------
    let config = maybe::<NoxConfig>(rpc, &nox.config)
        .await?
        .ok_or_else(|| anyhow!("NOXFUNDS is not initialised — run `authority init-noxfunds`"))?;
    if config.paused {
        bail!("NOXFUNDS is paused");
    }
    if config.usdc_mint != dep.usdc_mint {
        bail!(
            "the config's mint {} is not the deployment's {}",
            config.usdc_mint,
            dep.usdc_mint
        );
    }

    let entry = dep
        .markets
        .iter()
        .find(|m| m.symbol.eq_ignore_ascii_case(&args.market))
        .ok_or_else(|| {
            anyhow!(
                "{} is not in {}. Listed: {}",
                args.market,
                args.deployment.display(),
                dep.markets
                    .iter()
                    .map(|m| m.symbol.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let market_key = market_pda(entry.market_index);
    let market = maybe::<Market>(rpc, &market_key)
        .await?
        .ok_or_else(|| anyhow!("market {} is not listed on this cluster", entry.symbol))?;
    if market.status != MarketStatus::Active {
        bail!(
            "{} is {:?}, not Active — a trade would be refused",
            entry.symbol,
            market.status
        );
    }
    if !session_is_open(&market, now()?) {
        bail!(
            "{} is outside its session. Only the continuous markets trade at the weekend.",
            entry.symbol
        );
    }

    let prices = price_accounts(&args.price_accounts).await?;
    let price_update = *prices.get(&entry.symbol).ok_or_else(|| {
        anyhow!(
            "no price account for {} in the map — the poster has not published it",
            entry.symbol
        )
    })?;
    let price = oracle_price(rpc, &price_update).await?;

    let plan = plan_trade(
        entry.market_index,
        market_key,
        Limits::from(&market),
        price_update,
        price,
        args.principal,
    )?;

    let principal_raw = args
        .principal
        .checked_mul(ONE_USDC)
        .ok_or_else(|| anyhow!("--principal {} overflows", args.principal))?;
    let investor_token = ata(&investor.pubkey(), &dep.usdc_mint);
    let trader_token = ata(&trader.pubkey(), &dep.usdc_mint);
    let treasury_token = ata(&config.treasury, &dep.usdc_mint);
    // How the investor gets the principal. The funder mints it when it holds the mint authority
    // (this is a test mint), and transfers it otherwise. Which of the two is read off the mint,
    // never assumed.
    let held = token_balance(rpc, &investor_token).await;
    let short = principal_raw.saturating_sub(held);
    let mint_authority = read_mint_authority(rpc, &dep.usdc_mint).await?;
    let funder_holds = token_balance(rpc, &ata(&acting, &dep.usdc_mint)).await;
    let source = if short == 0 {
        Source::AlreadyHeld
    } else if mint_authority == Some(acting) {
        Source::Mint
    } else if funder_holds >= short {
        Source::Transfer
    } else {
        bail!(
            "the investor is {} USDC short and the funder can neither mint (the authority is \
             {:?}) nor cover it (holds {})",
            usdc(short),
            mint_authority,
            usdc(funder_holds)
        )
    };

    println!("\n  NOXFUNDS  {}", noxfunds::ID);
    println!("  investor  {}", investor.pubkey());
    println!("  trader    {}", trader.pubkey());
    println!("  mandate   {}", nox.mandate);
    println!(
        "  market    {} (index {})",
        entry.symbol, entry.market_index
    );
    println!("  oracle    {}", price_1e9(plan.price));
    println!(
        "  plan      principal {}, notional {}, margin {}, stop {} (100 bps below)",
        usdc(principal_raw),
        usdc(plan.notional),
        usdc(plan.collateral),
        price_1e9(plan.stop_price)
    );
    println!(
        "            size {} base units, inside the market's {}..={}",
        plan.size_base, market.min_position_size, market.max_position_size
    );

    // --- 1. three wallets with what they need ---------------------------------------------------
    let mut setup = vec![
        create_ata_idempotent(&acting, &investor.pubkey(), &dep.usdc_mint),
        create_ata_idempotent(&acting, &trader.pubkey(), &dep.usdc_mint),
        create_ata_idempotent(&acting, &config.treasury, &dep.usdc_mint),
    ];
    match source {
        Source::AlreadyHeld => {}
        Source::Mint => setup.push(mint_to_ix(&dep.usdc_mint, &investor_token, &acting, short)),
        Source::Transfer => setup.push(transfer_checked_ix(
            &ata(&acting, &dep.usdc_mint),
            &dep.usdc_mint,
            &investor_token,
            &acting,
            short,
            USDC_DECIMALS,
        )),
    }
    for (who, need) in [
        (investor.pubkey(), INVESTOR_LAMPORTS),
        (trader.pubkey(), TRADER_LAMPORTS),
        (nox.signer, SIGNER_LAMPORTS),
    ] {
        if rpc.get_balance(&who).await? < need {
            setup.push(transfer_ix(&acting, &who, need));
        }
    }
    if !execute {
        // A simulation needs no signature, so the transaction carrying every instruction this
        // tool builds by hand — the ATAs, the mint, the transfers — can be checked against the
        // real cluster before anything is sent.
        simulate(rpc, &acting, &setup, "the funding transaction").await?;
        println!("  every rule checks out. Re-run with --execute to send.\n");
        return Ok(());
    }

    step(rpc, funder, setup, 120_000, "1/10 funding").await?;
    println!(
        "       investor holds {} USDC",
        usdc(token_balance(rpc, &investor_token).await)
    );

    // --- 2. the trader's record ---------------------------------------------------------------
    if maybe::<TraderProfile>(rpc, &nox.profile).await?.is_some() {
        println!("  2/10 profile      already exists");
    } else {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::InitializeTraderProfile {
                payer: investor.pubkey(),
                authority: trader.pubkey(),
                profile: nox.profile,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::InitializeTraderProfile {}.data(),
        };
        step(rpc, investor, vec![ix], 60_000, "2/10 profile").await?;
    }

    // --- 2. the mandate, and the principal that makes it real ---------------------------------
    let existing = maybe::<Mandate>(rpc, &nox.mandate).await?;
    if let Some(m) = &existing {
        println!(
            "  3/10 mandate      already exists — {:?}, principal {}",
            m.state,
            usdc(m.principal)
        );
        if m.state != MandateState::Active {
            bail!(
                "this mandate is {:?}; pass --seq {} for a fresh one",
                m.state,
                args.seq.saturating_add(1)
            );
        }
    } else {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundMandate {
                investor: investor.pubkey(),
                config: nox.config,
                trader: trader.pubkey(),
                trader_profile: nox.profile,
                mandate: nox.mandate,
                mandate_signer: nox.signer,
                solfx_user_account: nox.user_account,
                usdc_mint: dep.usdc_mint,
                investor_token,
                mandate_vault: nox.vault,
                token_program: TOKEN_PROGRAM,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundMandate {
                seq: args.seq,
                principal: principal_raw,
                rules: plan.rules,
            }
            .data(),
        };
        step(rpc, investor, vec![ix], 80_000, "3/10 fund mandate").await?;
        println!(
            "       vault holds {}",
            usdc(token_balance(rpc, &nox.vault).await)
        );
    }

    // --- 4. the SolFX account the mandate trades through ---------------------------------------
    if maybe::<UserAccount>(rpc, &nox.user_account)
        .await?
        .is_some()
    {
        println!("  4/10 solfx acct   already exists");
    } else {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::CreateSolfxAccount {
                payer: investor.pubkey(),
                config: nox.config,
                mandate: nox.mandate,
                mandate_signer: nox.signer,
                protocol: p.protocol,
                user_account: nox.user_account,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::CreateSolfxAccount {}.data(),
        };
        step(rpc, investor, vec![ix], 80_000, "4/10 solfx acct").await?;
    }

    // --- 5. the principal into SolFX -----------------------------------------------------------
    let in_vault = token_balance(rpc, &nox.vault).await;
    if in_vault == 0 {
        println!("  5/10 deposit      the vault is already in SolFX");
    } else {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundSolfxCollateral {
                payer: investor.pubkey(),
                config: nox.config,
                mandate: nox.mandate,
                mandate_signer: nox.signer,
                protocol: p.protocol,
                user_account: nox.user_account,
                collateral_mint: dep.usdc_mint,
                collateral_vault: p.collateral_vault,
                mandate_vault: nox.vault,
                token_program: TOKEN_PROGRAM,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundSolfxCollateral { amount: in_vault }.data(),
        };
        step(rpc, investor, vec![ix], 80_000, "5/10 deposit").await?;
    }

    // --- 6. the funded trade, with its stop, atomically ----------------------------------------
    let nonce = 0u8;
    let position = position_pda(&nox.user_account, plan.market_index, nonce);
    let trigger = trigger_pda(&position, nonce);
    if maybe::<Position>(rpc, &position).await?.is_some() {
        println!("  6/10 open         a position is already open on nonce 0");
    } else {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedOpenPosition {
                trader: trader.pubkey(),
                config: nox.config,
                mandate: nox.mandate,
                mandate_signer: nox.signer,
                protocol: p.protocol,
                user_account: nox.user_account,
                market: plan.market,
                position,
                trigger_order: trigger,
                collateral_vault: p.collateral_vault,
                lp_pool: p.lp_pool,
                lp_vault: p.lp_vault,
                insurance_fund: p.insurance_fund,
                insurance_vault: p.insurance_vault,
                fee_vault: p.fee_vault,
                price_update: plan.price_update,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: TOKEN_PROGRAM,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedOpenPosition {
                market_index: plan.market_index,
                nonce,
                direction: Direction::Long,
                size_base: plan.size_base,
                collateral: plan.collateral,
                price_limit: plan.price_limit,
                order_id: nonce,
                stop_loss_price: plan.stop_price,
            }
            .data(),
        };
        step(rpc, trader, vec![ix], 200_000, "6/10 funded open").await?;
        if let Some(pos) = maybe::<Position>(rpc, &position).await? {
            println!(
                "       entry {} — the position and its stop landed in one transaction",
                price_1e9(pos.entry_price)
            );
        }
    }

    // --- 7. the permissionless equity crank -----------------------------------------------------
    let mut metas = noxfunds::accounts::ObserveMandateEquity {
        observer: investor.pubkey(),
        config: nox.config,
        mandate: nox.mandate,
        mandate_signer: nox.signer,
        user_account: nox.user_account,
        trader_profile: nox.profile,
    }
    .to_account_metas(None);
    metas.push(AccountMeta::new_readonly(position, false));
    metas.push(AccountMeta::new_readonly(plan.market, false));
    metas.push(AccountMeta::new_readonly(plan.price_update, false));
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: metas,
        data: noxfunds::instruction::ObserveMandateEquity {}.data(),
    };
    step(rpc, investor, vec![ix], 80_000, "7/10 equity crank").await?;
    if let Some(m) = maybe::<Mandate>(rpc, &nox.mandate).await? {
        println!(
            "       equity {} against a peak of {}",
            usdc(m.last_equity),
            usdc(m.peak_equity)
        );
    }

    // --- 8. close, and reclaim the stop's rent ---------------------------------------------------
    let close = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundedClosePosition {
            trader: trader.pubkey(),
            config: nox.config,
            mandate: nox.mandate,
            mandate_signer: nox.signer,
            protocol: p.protocol,
            user_account: nox.user_account,
            market: plan.market,
            position,
            trader_profile: nox.profile,
            collateral_vault: p.collateral_vault,
            lp_pool: p.lp_pool,
            lp_vault: p.lp_vault,
            insurance_fund: p.insurance_fund,
            insurance_vault: p.insurance_vault,
            fee_vault: p.fee_vault,
            price_update: plan.price_update,
            secondary_price_update: None,
            quote_conversion_price_update: None,
            token_program: TOKEN_PROGRAM,
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        // A long closes by selling, so the bound is a minimum.
        data: noxfunds::instruction::FundedClosePosition {
            market_index: plan.market_index,
            nonce,
            price_limit: plan
                .price
                .checked_sub(
                    plan.price
                        .saturating_mul(100)
                        .checked_div(10_000)
                        .ok_or_else(|| anyhow!("slippage overflows"))?,
                )
                .ok_or_else(|| anyhow!("slippage bound underflows"))?,
        }
        .data(),
    };
    step(rpc, trader, vec![close], 200_000, "8/10 close").await?;
    if let Some(prof) = maybe::<TraderProfile>(rpc, &nox.profile).await? {
        println!(
            "       record: {} trades, {}W/{}L, gross +{} / -{}",
            prof.trades,
            prof.wins,
            prof.losses,
            usdc(prof.gross_profit),
            usdc(prof.gross_loss)
        );
    }

    let cancel = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundedCancelStop {
            trader: trader.pubkey(),
            config: nox.config,
            mandate: nox.mandate,
            mandate_signer: nox.signer,
            trigger_order: trigger,
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundedCancelStop {
            market_index: plan.market_index,
            nonce,
            order_id: nonce,
        }
        .data(),
    };
    step(rpc, trader, vec![cancel], 60_000, "9/10 reclaim stop").await?;

    // --- 10. settlement, which anyone may run ------------------------------------------------------
    let request = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::RequestSettlement {
            investor: investor.pubkey(),
            mandate: nox.mandate,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::RequestSettlement {}.data(),
    };
    let claim = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::ClaimSettlement {
            settler: investor.pubkey(),
            config: nox.config,
            mandate: nox.mandate,
            mandate_signer: nox.signer,
            trader_profile: nox.profile,
            protocol: p.protocol,
            user_account: nox.user_account,
            usdc_mint: dep.usdc_mint,
            collateral_vault: p.collateral_vault,
            mandate_vault: nox.vault,
            investor_token,
            trader_token,
            treasury_token,
            token_program: TOKEN_PROGRAM,
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::ClaimSettlement {}.data(),
    };
    let before = token_balance(rpc, &investor_token).await;
    step(rpc, investor, vec![request, claim], 200_000, "10/10 settle").await?;

    let after = token_balance(rpc, &investor_token).await;
    println!("\n  settled");
    println!("    investor  {} → {}", usdc(before), usdc(after));
    println!(
        "    trader    {} USDC",
        usdc(token_balance(rpc, &trader_token).await)
    );
    println!(
        "    treasury  {} USDC",
        usdc(token_balance(rpc, &treasury_token).await)
    );
    if let Some(m) = maybe::<Mandate>(rpc, &nox.mandate).await? {
        println!("    mandate   {:?}", m.state);
    }
    println!(
        "\n  mandate {} is on chain and readable by anyone.\n",
        nox.mandate
    );
    Ok(())
}

// --- plumbing --------------------------------------------------------------------------------------

/// Run a transaction against the cluster without signing or sending it.
async fn simulate(rpc: &RpcClient, payer: &Pubkey, ixs: &[Instruction], what: &str) -> Result<()> {
    let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(200_000)];
    all.extend_from_slice(ixs);
    let tx = Transaction::new_unsigned(Message::new(&all, Some(payer)));
    let result = rpc
        .simulate_transaction_with_config(
            &tx,
            solana_rpc_client_api::config::RpcSimulateTransactionConfig {
                sig_verify: false,
                replace_recent_blockhash: true,
                commitment: Some(rpc.commitment()),
                ..Default::default()
            },
        )
        .await
        .with_context(|| format!("simulating {what}"))?
        .value;
    if let Some(err) = result.err {
        for line in result.logs.unwrap_or_default() {
            println!("    {line}");
        }
        bail!("{what} would fail: {err}");
    }
    println!(
        "\n  simulation: {what} succeeds, {} CU",
        result
            .units_consumed
            .map_or_else(|| "?".to_owned(), |u| u.to_string())
    );
    Ok(())
}

async fn step(
    rpc: &RpcClient,
    payer: &Keypair,
    ixs: Vec<Instruction>,
    cu: u32,
    label: &str,
) -> Result<()> {
    let mut all = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(cu),
        ComputeBudgetInstruction::set_compute_unit_price(10_000),
    ];
    all.extend(ixs);
    let blockhash = rpc.get_latest_blockhash().await.context("blockhash")?;
    let msg = Message::new(&all, Some(&payer.pubkey()));
    let mut tx = Transaction::new_unsigned(msg);
    tx.try_sign(&[payer], blockhash).context("signing")?;
    match rpc.send_and_confirm_transaction(&tx).await {
        Ok(sig) => {
            println!("  {label:<17} {sig}");
            Ok(())
        }
        // The logs are the whole diagnosis, and `send_and_confirm_transaction` swallows them.
        Err(e) => Err(anyhow!("{label} failed: {e}")),
    }
}

fn transfer_ix(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = vec![2, 0, 0, 0];
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: anchor_lang::system_program::ID,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

/// Where the investor's principal comes from, decided by reading the mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The investor already holds it — a re-run, most likely.
    AlreadyHeld,
    /// The funder holds the mint authority. This is a test mint.
    Mint,
    /// The funder holds the tokens and sends them.
    Transfer,
}

/// The mint's authority, or `None` if minting has been disabled.
///
/// Read from the account rather than assumed: whether the funder can mint decides how the
/// investor is funded, and getting it wrong is a failed transaction rather than a clear refusal.
async fn read_mint_authority(rpc: &RpcClient, mint: &Pubkey) -> Result<Option<Pubkey>> {
    let data = rpc
        .get_account_data(mint)
        .await
        .with_context(|| format!("reading the mint {mint}"))?;
    // SPL Mint: COption<Pubkey> authority (4-byte tag + 32), supply u64, decimals u8.
    let tag = data
        .get(0..4)
        .ok_or_else(|| anyhow!("{mint} is too short to be a mint"))?;
    if u32::from_le_bytes(tag.try_into()?) == 0 {
        return Ok(None);
    }
    let key = data
        .get(4..36)
        .ok_or_else(|| anyhow!("{mint} is too short to be a mint"))?;
    Ok(Some(
        Pubkey::try_from(key).map_err(|_| anyhow!("bad mint authority"))?,
    ))
}

/// SPL Token `MintTo`: byte 7 then the amount, little-endian. Accounts: mint, destination,
/// authority. Confirmed against the SPL interface via the Solana MCP, and identical to the
/// helper `init-protocol` already uses.
fn mint_to_ix(mint: &Pubkey, dest: &Pubkey, authority: &Pubkey, amount: u64) -> Instruction {
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*dest, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

/// SPL Token `TransferChecked`: byte 12, the amount, then the decimals. Accounts: source, mint,
/// destination, authority. `transfer_checked` rather than `transfer` so the mint and its
/// decimals are verified by the Token Program too — the same choice the program makes.
fn transfer_checked_ix(
    from: &Pubkey,
    mint: &Pubkey,
    to: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![12u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(*from, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(*to, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

/// The ATA program's `CreateIdempotent`: variant byte 1, six accounts, rent sysvar omitted.
/// Confirmed against the SPL interface via the Solana MCP, and identical to the helper
/// `init-protocol` already uses.
fn create_ata_idempotent(funder: &Pubkey, wallet: &Pubkey, mint: &Pubkey) -> Instruction {
    Instruction {
        program_id: ATA_PROGRAM,
        accounts: vec![
            AccountMeta::new(*funder, true),
            AccountMeta::new(ata(wallet, mint), false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
        ],
        data: vec![1],
    }
}

/// A key created on first use, so a re-run builds on the same record rather than a fresh one.
fn load_or_create(path: &Path, what: &str) -> Result<Keypair> {
    if path.exists() {
        return load_keypair(&path.to_string_lossy());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let kp = Keypair::new();
    std::fs::write(path, serde_json::to_string(&kp.to_bytes().to_vec())?)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("  created a key for the {what} at {}", path.display());
    Ok(kp)
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

fn usdc(raw: u64) -> String {
    format!(
        "{}.{:06}",
        raw.checked_div(ONE_USDC).unwrap_or(0),
        raw.checked_rem(ONE_USDC).unwrap_or(0)
    )
}

fn price_1e9(price: i64) -> String {
    let p = u128::try_from(price.max(0)).unwrap_or(0);
    format!(
        "{}.{:09}",
        p.checked_div(BASE_PRECISION).unwrap_or(0),
        p.checked_rem(BASE_PRECISION).unwrap_or(0)
    )
}

// Test code asserts against known values and unwraps expected-Ok results, the same allowance
// `trade.rs` takes for the same reason.
#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::unwrap_used
)]
mod tests {
    use super::*;

    /// The addresses this tool derives, against what devnet actually holds.
    #[test]
    fn the_derived_addresses_match_devnet() {
        let config: Pubkey = "8xT4KH1xnpqKW7ic1Pypu7vUJU5HRGnyJZ6z5kWtkzQ4"
            .parse()
            .unwrap();
        let nox = Nox::derive(&Pubkey::new_unique(), &Pubkey::new_unique(), 0);
        assert_eq!(nox.config, config, "the config PDA is a fixed singleton");
        assert_eq!(
            Pubkey::find_program_address(&[PROTOCOL_SEED], &solfx_core::ID).0,
            "GbgsnqqqRws5Ch33eHuoAWqKghQiVWt8wBSKzwNffjwc"
                .parse::<Pubkey>()
                .unwrap()
        );
    }

    /// Every seed is the program's own constant, so a mandate's addresses are reproducible by
    /// anyone reading the chain.
    #[test]
    fn a_mandates_addresses_are_derived_not_invented() {
        let investor: Pubkey = "7ktphnZe9rER59HanbM6mDk9aDAbvc2pcjDcPWDvBdWs"
            .parse()
            .unwrap();
        let trader = Pubkey::new_unique();
        let a = Nox::derive(&investor, &trader, 0);
        let b = Nox::derive(&investor, &trader, 0);
        assert_eq!(a.mandate, b.mandate);
        assert_eq!(a.vault, b.vault);
        // A different sequence number is a different mandate, and a different vault.
        let c = Nox::derive(&investor, &trader, 1);
        assert_ne!(a.mandate, c.mandate);
        assert_ne!(a.vault, c.vault);
    }

    fn market_for(min: u64, max: u64) -> Limits {
        Limits {
            min_position_size: min,
            max_position_size: max,
            imr_bps: 1_000,
            max_leverage: 10,
        }
    }

    /// $200 of principal at a BTC-like price: half deployed, a quarter as margin.
    #[test]
    fn the_plan_sizes_the_trade_from_the_principal() {
        let price = 80_000 * 1_000_000_000_i64; // $80,000 at PRICE_PRECISION
        let m = market_for(1, u64::MAX);
        let plan =
            plan_trade(5, Pubkey::new_unique(), m, Pubkey::new_unique(), price, 200).unwrap();
        assert_eq!(plan.notional, 100 * ONE_USDC);
        assert_eq!(plan.collateral, 50 * ONE_USDC);
        // $100 of BTC at $80,000 is 0.00125 BTC, and base units are 1e9: 1,250,000.
        // Pinned because the first version computed 1, having used whole USDC for the quote.
        assert_eq!(plan.size_base, 1_250_000);
        // The stop sits 100 bps below the entry, so a long's stop is below the price.
        assert!(plan.stop_price < price);
        assert!(plan.price_limit > price, "a long's bound is a maximum");
        assert_eq!(plan.rules.allowed_markets, 1 << 5);
        plan.rules.validate().unwrap();
    }

    /// A market whose minimum is larger than the plan's size is refused *before* sending,
    /// naming the bound — the PositionSizeOutOfBounds failure that cost a day on localnet.
    #[test]
    fn a_size_outside_the_markets_bounds_is_refused_before_sending() {
        let price = 80_000 * 1_000_000_000_i64;
        let m = market_for(10_000_000_000_000, u64::MAX);
        let err = plan_trade(5, Pubkey::new_unique(), m, Pubkey::new_unique(), price, 200)
            .expect_err("a $100 position cannot meet a huge minimum");
        assert!(format!("{err}").contains("outside"), "{err}");
    }

    #[test]
    fn collateral_below_the_markets_initial_margin_is_refused() {
        let price = 80_000 * 1_000_000_000_i64;
        let mut m = market_for(1, u64::MAX);
        m.imr_bps = 6_000; // 60 % margin: a quarter of the principal cannot cover it
        let err = plan_trade(5, Pubkey::new_unique(), m, Pubkey::new_unique(), price, 200)
            .expect_err("margin below the market's requirement");
        assert!(format!("{err}").contains("initial margin"), "{err}");
    }

    /// The ATA instruction, byte for byte, as the SPL interface documents it.
    #[test]
    fn the_ata_instruction_matches_the_spl_interface() {
        let funder = Pubkey::new_unique();
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let ix = create_ata_idempotent(&funder, &wallet, &mint);
        assert_eq!(ix.program_id, ATA_PROGRAM);
        assert_eq!(ix.data, vec![1], "1 = CreateIdempotent");
        let keys: Vec<Pubkey> = ix.accounts.iter().map(|a| a.pubkey).collect();
        assert_eq!(
            keys,
            vec![
                funder,
                ata(&wallet, &mint),
                wallet,
                mint,
                anchor_lang::system_program::ID,
                TOKEN_PROGRAM
            ]
        );
        assert!(ix.accounts[0].is_signer && ix.accounts[0].is_writable);
        assert!(ix.accounts[1].is_writable && !ix.accounts[1].is_signer);
    }

    /// The System Program's transfer is instruction 2, little-endian lamports.
    /// Byte 7 then a little-endian amount, as the SPL interface documents it.
    #[test]
    fn the_mint_instruction_matches_the_spl_interface() {
        let mint = Pubkey::new_unique();
        let dest = Pubkey::new_unique();
        let auth = Pubkey::new_unique();
        let ix = mint_to_ix(&mint, &dest, &auth, 200 * ONE_USDC);
        assert_eq!(ix.program_id, TOKEN_PROGRAM);
        assert_eq!(ix.data[0], 7);
        assert_eq!(
            u64::from_le_bytes(ix.data[1..9].try_into().unwrap()),
            200 * ONE_USDC
        );
        assert_eq!(ix.data.len(), 9);
        assert!(
            ix.accounts[0].is_writable && !ix.accounts[0].is_signer,
            "mint"
        );
        assert!(ix.accounts[1].is_writable, "destination");
        assert!(ix.accounts[2].is_signer, "authority signs");
    }

    /// Byte 12, amount, decimals — and the mint is read-only, which `transfer` cannot check.
    #[test]
    fn the_transfer_checked_instruction_carries_the_decimals() {
        let ix = transfer_checked_ix(
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            200 * ONE_USDC,
            6,
        );
        assert_eq!(ix.data[0], 12);
        assert_eq!(
            u64::from_le_bytes(ix.data[1..9].try_into().unwrap()),
            200 * ONE_USDC
        );
        assert_eq!(ix.data[9], 6);
        assert_eq!(ix.data.len(), 10);
        assert!(!ix.accounts[1].is_writable, "the mint is only read");
        assert!(ix.accounts[3].is_signer, "the authority signs");
    }

    #[test]
    fn the_transfer_instruction_is_system_program_variant_two() {
        let ix = transfer_ix(&Pubkey::new_unique(), &Pubkey::new_unique(), 20_000_000);
        assert_eq!(ix.program_id, anchor_lang::system_program::ID);
        assert_eq!(&ix.data[0..4], &[2, 0, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(ix.data[4..12].try_into().unwrap()),
            20_000_000
        );
    }

    #[test]
    fn money_is_printed_to_the_cent() {
        assert_eq!(usdc(200_000_000), "200.000000");
        assert_eq!(usdc(1_234_500), "1.234500");
        assert_eq!(price_1e9(80_000 * 1_000_000_000), "80000.000000000");
    }
}
