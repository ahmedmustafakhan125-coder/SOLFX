//! A trading CLI, so the protocol can be exercised before a frontend exists.
//!
//! # Why this exists
//!
//! Phase 8 builds the terminal. Until then there is no way to place a trade, and "deploy the
//! frontend and find out" is a bad first test: when something fails you cannot tell whether
//! the program rejected the trade or the browser built it wrong. This keeps that boundary
//! sharp — everything here is the program's answer.
//!
//! # The one thing that is not obvious
//!
//! `open_position` takes the price account as a **passed account**, and on a cluster with no
//! sponsored feeds it is not the canonical `[shard, feed_id]` PDA — it is whatever
//! `price-poster` created. So this reads `price-accounts.json` rather than deriving. Deriving
//! would look right, compile, and then fail with `AccountNotFound` on an address that is
//! correct in every sense except that nobody publishes to it.
//!
//! # Running it
//!
//! ```text
//! cargo run -p solfx-keeper --bin trade -- setup --amount 100000
//! cargo run -p solfx-keeper --bin trade -- status
//! cargo run -p solfx-keeper --bin trade -- open  --market BTC/USD --direction long --lots 0.01 --collateral 1000
//! cargo run -p solfx-keeper --bin trade -- close --market BTC/USD
//! ```

#[allow(dead_code)]
#[path = "../pyth.rs"]
mod pyth;

#[allow(dead_code)]
#[path = "../contracts.rs"]
mod contracts;

// The feed id → price account map, shared with the keeper rather than reimplemented. Both
// have to agree about where a self-posted price lives; two loaders would drift and the
// symptom would be a transaction rejected on chain, not a test failure.
#[allow(dead_code)]
#[path = "../price_map.rs"]
mod price_map;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

use solfx_core::constants::{
    COLLATERAL_VAULT_SEED, FEE_VAULT_SEED, INSURANCE_FUND_SEED, INSURANCE_VAULT_SEED, LP_POOL_SEED,
    LP_VAULT_SEED, MARKET_SEED, POSITION_SEED, PROTOCOL_SEED, USER_SEED,
};
use solfx_core::state::{Direction, Market, Position, UserAccount};

const ONE_USDC: u64 = 1_000_000;
/// `size_base * price / NOTIONAL_DIVISOR` gives notional in quote units.
/// Nonces probed when looking for open positions. A trader may hold several positions on one
/// market, distinguished only by nonce, and nothing on chain enumerates them — so a client
/// has to scan. Eight is well past the practical limit and costs eight cheap reads.
const MAX_NONCE_SCAN: u8 = 8;

const NOTIONAL_DIVISOR: u128 = 1_000_000_000_000;
const BASE_PRECISION: u128 = 1_000_000_000;

const TOKEN_PROGRAM: Pubkey = solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ATA_PROGRAM: Pubkey = solana_pubkey::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

#[derive(clap::Parser)]
#[command(name = "trade", about = "Exercise SolFX from the command line")]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
    /// Written by `init-protocol`.
    #[arg(long, default_value = "deployment.json")]
    deployment: PathBuf,
    /// Written by `price-poster`. Holds the price accounts actually being published to.
    #[arg(long, default_value = "price-accounts.json")]
    price_accounts: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Create the user account and fund it with test USDC.
    Setup {
        #[arg(long, default_value_t = 100_000)]
        amount: u64,
    },
    /// Show balances, markets and open positions.
    Status,
    Open {
        #[arg(long)]
        market: String,
        #[arg(long)]
        direction: Side,
        /// Size in standard lots: `1`, `0.1`, `0.01`. A lot means different amounts per
        /// asset class — see `units_per_lot`.
        ///
        /// Parsed as an exact decimal, never a float: rounding a *size* is a mis-sized
        /// trade rather than a rejected one, and it would be invisible.
        #[arg(long, conflicts_with = "notional")]
        lots: Option<String>,
        /// Size in units of the base asset, as a decimal: `--quantity 1000` on EUR/USD is
        /// 1,000 EUR; on BTC/USD it is 1,000 BTC.
        ///
        /// XM and Exness both offer this beside Lots, and it is the mode that needs no
        /// contract-size knowledge at all — 1,000 EUR is 1,000 EUR whatever a lot happens
        /// to be.
        #[arg(long, conflicts_with_all = ["lots", "notional"])]
        quantity: Option<String>,
        /// Size by exposure in whole USD — `--notional 1000` opens $1,000 of the asset at
        /// the current oracle price.
        ///
        /// How people think about crypto ("$1,000 of BTC"), and likewise contract-size free.
        #[arg(long, conflicts_with_all = ["lots", "quantity"])]
        notional: Option<u64>,
        /// Margin to post, in whole test USDC.
        #[arg(long)]
        collateral: u64,
        #[arg(long, default_value_t = 0)]
        nonce: u8,
        /// Slippage tolerance in basis points. 100 = 1%.
        ///
        /// There is no "off": `validate_slippage` requires `exec <= limit` on a buy, so a
        /// zero bound rejects every fill. That is deliberate on the protocol's part — a
        /// trade cannot accidentally go out unbounded — so the bound is always computed
        /// from the current oracle price.
        #[arg(long, default_value_t = 100)]
        slippage_bps: u32,
    },
    Close {
        #[arg(long)]
        market: String,
        /// Which position, when several are open on one market.
        ///
        /// Omitted, the single open position is closed. If more than one is open the command
        /// refuses and lists them — silently picking nonce 0 is how you close the wrong
        /// position and only notice from the PnL.
        #[arg(long)]
        nonce: Option<u8>,
        #[arg(long, default_value_t = 100)]
        slippage_bps: u32,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Side {
    Long,
    Short,
}

impl From<Side> for Direction {
    fn from(s: Side) -> Self {
        match s {
            Side::Long => Self::Long,
            Side::Short => Self::Short,
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
    let me = load_keypair(&args.keypair)?;
    let rpc = RpcClient::new_with_commitment(args.rpc_url.clone(), CommitmentConfig::confirmed());
    let dep = Deployment::load(&args.deployment)?;
    let prices = load_price_accounts(&args.price_accounts)?;
    let p = Pdas::derive();
    let user_account =
        Pubkey::find_program_address(&[USER_SEED, me.pubkey().as_ref()], &solfx_core::ID).0;

    match args.cmd {
        Cmd::Setup { amount } => setup(&rpc, &me, &dep, &p, user_account, amount).await,
        Cmd::Status => status(&rpc, &me, &dep, &p, user_account).await,
        Cmd::Open {
            market,
            direction,
            lots,
            quantity,
            notional,
            collateral,
            nonce,
            slippage_bps,
        } => {
            open(
                &rpc,
                &me,
                &dep,
                &p,
                user_account,
                &prices,
                &market,
                direction,
                lots.as_deref(),
                quantity.as_deref(),
                notional,
                collateral,
                nonce,
                slippage_bps,
            )
            .await
        }
        Cmd::Close {
            market,
            nonce,
            slippage_bps,
        } => {
            close(
                &rpc,
                &me,
                &dep,
                &p,
                user_account,
                &prices,
                &market,
                nonce,
                slippage_bps,
            )
            .await
        }
    }
}

// --- commands -------------------------------------------------------------------------------

async fn setup(
    rpc: &RpcClient,
    me: &Keypair,
    dep: &Deployment,
    p: &Pdas,
    user_account: Pubkey,
    amount: u64,
) -> Result<()> {
    if rpc.get_account(&user_account).await.is_err() {
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::InitializeUserAccount {
                authority: me.pubkey(),
                protocol: p.protocol,
                user_account,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            // No referrer. The binding is immutable once set, so it is not something to
            // default to a placeholder.
            data: solfx_core::instruction::InitializeUserAccount {
                referrer: Pubkey::default(),
            }
            .data(),
        };
        println!(
            "initialize_user_account  {}",
            send(rpc, me, vec![ix], 100_000).await?
        );
    } else {
        println!("user account already exists at {user_account}");
    }

    let lamports = amount.saturating_mul(ONE_USDC);
    let ix = Instruction {
        program_id: solfx_core::ID,
        // `deposit_collateral` and `withdraw_collateral` share one accounts struct.
        accounts: solfx_core::accounts::MoveCollateral {
            authority: me.pubkey(),
            protocol: p.protocol,
            user_account,
            collateral_mint: dep.usdc_mint,
            collateral_vault: p.collateral_vault,
            user_token_account: ata(&me.pubkey(), &dep.usdc_mint),
            token_program: TOKEN_PROGRAM,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::DepositCollateral { amount: lamports }.data(),
    };
    println!(
        "deposit_collateral {amount}  {}",
        send(rpc, me, vec![ix], 100_000).await?
    );
    Ok(())
}

async fn status(
    rpc: &RpcClient,
    me: &Keypair,
    dep: &Deployment,
    p: &Pdas,
    user_account: Pubkey,
) -> Result<()> {
    println!("wallet   {}", me.pubkey());
    println!("account  {user_account}");

    match rpc.get_account_data(&user_account).await {
        Ok(data) => {
            let ua = UserAccount::try_deserialize(&mut data.as_slice())
                .map_err(|e| anyhow!("decoding user account: {e}"))?;
            println!("  free collateral {}", usdc(ua.free_collateral));
            println!("  open positions {}", ua.open_positions);
        }
        Err(_) => println!("  (no user account — run `trade setup`)"),
    }

    // An SPL token account keeps its amount at offset 64, little-endian.
    if let Ok(data) = rpc.get_account_data(&p.lp_vault).await {
        if let Some(amount) = data.get(64..72).and_then(|b| <[u8; 8]>::try_from(b).ok()) {
            println!("lp vault   {}", usdc(u64::from_le_bytes(amount)));
        }
    }

    println!("\nmarkets:");
    for m in &dep.markets {
        let market_pda = Pubkey::find_program_address(
            &[MARKET_SEED, &m.market_index.to_le_bytes()],
            &solfx_core::ID,
        )
        .0;
        let Ok(data) = rpc.get_account_data(&market_pda).await else {
            println!("  [{}] {:<14} not listed", m.market_index, m.symbol);
            continue;
        };
        let market = Market::try_deserialize(&mut data.as_slice())
            .map_err(|e| anyhow!("decoding market {}: {e}", m.symbol))?;
        println!(
            "  [{}] {:<14} max {}x  oi long {}  short {}",
            m.market_index,
            m.symbol,
            market.max_leverage,
            usdc(market.oi_long),
            usdc(market.oi_short)
        );

        for nonce in 0..MAX_NONCE_SCAN {
            let pos_pda = position_pda(&user_account, m.market_index, nonce);
            let Ok(data) = rpc.get_account_data(&pos_pda).await else {
                continue;
            };
            if let Ok(pos) = Position::try_deserialize(&mut data.as_slice()) {
                println!(
                    "        position[nonce {nonce}] {:?} size {} collateral {} entry {}  \
                     -> close --market {} --nonce {nonce}",
                    pos.direction,
                    pos.size_base,
                    usdc(pos.collateral),
                    pos.entry_price,
                    m.symbol
                );
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn open(
    rpc: &RpcClient,
    me: &Keypair,
    dep: &Deployment,
    p: &Pdas,
    user_account: Pubkey,
    prices: &HashMap<String, Pubkey>,
    symbol: &str,
    side: Side,
    lots: Option<&str>,
    quantity: Option<&str>,
    notional: Option<u64>,
    collateral: u64,
    nonce: u8,
    slippage_bps: u32,
) -> Result<()> {
    let m = dep.market(symbol)?;
    let price_update = *prices.get(&symbol.to_uppercase()).ok_or_else(|| {
        anyhow!("no price account for {symbol} in price-accounts.json — is the poster running?")
    })?;

    if rpc.get_account(&price_update).await.is_err() {
        bail!(
            "price account {price_update} for {symbol} does not exist on this cluster.\n\
             Start the poster:  cargo run -p solfx-keeper --bin price-poster -- --rpc-url <rpc>"
        );
    }

    // Either sizing route ends in base units; the difference is only which one the trader
    // finds natural for this asset.
    let oracle_price = read_oracle_price(rpc, &price_update).await?;
    // Three ways to say the same thing, all ending in base units. Which one a trader
    // reaches for depends on the asset: lots for FX out of habit, quantity for equities and
    // metals, dollars for crypto.
    let (size_base, described) = match (lots, quantity, notional) {
        (Some(l), None, None) => (
            lots_to_base(l, contracts::units_per_lot(symbol))?,
            format!("{l} lots"),
        ),
        (None, Some(q), None) => (quantity_to_base(q)?, format!("{q} units")),
        (None, None, Some(n)) => (notional_to_base(n, oracle_price)?, format!("${n} notional")),
        _ => bail!("give exactly one of --lots, --quantity or --notional"),
    };
    let side_is_buy = matches!(side, Side::Long);
    let price_limit = slippage_bound(oracle_price, slippage_bps, side_is_buy)?;
    let market_pda = Pubkey::find_program_address(
        &[MARKET_SEED, &m.market_index.to_le_bytes()],
        &solfx_core::ID,
    )
    .0;
    let position = position_pda(&user_account, m.market_index, nonce);

    println!("opening {symbol} {side:?} {described}, {collateral} USDC margin");
    println!(
        "  size     {size_base} base units ({} per lot)",
        contracts::units_per_lot(symbol)
    );
    println!("  market   {market_pda}");
    println!("  position {position}");
    println!("  price    {price_update}");
    println!("  oracle   {oracle_price}  limit {price_limit} ({slippage_bps} bps)");

    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::OpenPosition {
            authority: me.pubkey(),
            protocol: p.protocol,
            user_account,
            market: market_pda,
            position,
            collateral_vault: p.collateral_vault,
            lp_pool: p.lp_pool,
            lp_vault: p.lp_vault,
            insurance_fund: p.insurance_fund,
            insurance_vault: p.insurance_vault,
            fee_vault: p.fee_vault,
            price_update,
            // Direct, USD-quoted markets need neither leg. Anchor encodes an absent optional
            // account as the program id itself.
            secondary_price_update: None,
            quote_conversion_price_update: None,
            token_program: TOKEN_PROGRAM,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::OpenPosition {
            market_index: m.market_index,
            nonce,
            direction: side.into(),
            size_base,
            collateral: collateral.saturating_mul(ONE_USDC),
            price_limit,
        }
        .data(),
    };
    println!(
        "\nopen_position  {}",
        send(rpc, me, vec![ix], 200_000).await?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn close(
    rpc: &RpcClient,
    me: &Keypair,
    dep: &Deployment,
    p: &Pdas,
    user_account: Pubkey,
    prices: &HashMap<String, Pubkey>,
    symbol: &str,
    nonce: Option<u8>,
    slippage_bps: u32,
) -> Result<()> {
    let m = dep.market(symbol)?;
    let price_update = *prices
        .get(&symbol.to_uppercase())
        .ok_or_else(|| anyhow!("no price account for {symbol}"))?;
    let market_pda = Pubkey::find_program_address(
        &[MARKET_SEED, &m.market_index.to_le_bytes()],
        &solfx_core::ID,
    )
    .0;
    // Resolve which position to close before building anything.
    let open = open_nonces(rpc, &user_account, m.market_index).await;
    let nonce = match (nonce, open.as_slice()) {
        (Some(n), _) => n,
        (None, []) => bail!("no open position on {symbol}"),
        (None, [only]) => *only,
        (None, many) => bail!(
            "{} positions open on {symbol} (nonces {}). Say which: --nonce <n>",
            many.len(),
            many.iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let position = position_pda(&user_account, m.market_index, nonce);

    let data = rpc.get_account_data(&position).await.map_err(|_| {
        if open.is_empty() {
            anyhow!("no open position on {symbol}")
        } else {
            anyhow!(
                "no position on {symbol} at nonce {nonce}. Open: {}",
                open.iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    })?;
    let pos = Position::try_deserialize(&mut data.as_slice())
        .map_err(|e| anyhow!("decoding position: {e}"))?;

    // Closing a long is a sell, so the bound is a floor rather than a ceiling.
    let oracle_price = read_oracle_price(rpc, &price_update).await?;
    let closing_a_long = matches!(pos.direction, Direction::Long);
    let price_limit = slippage_bound(oracle_price, slippage_bps, !closing_a_long)?;
    println!(
        "closing {symbol} {:?} [nonce {nonce}], oracle {oracle_price}, limit {price_limit}",
        pos.direction
    );

    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::ClosePosition {
            authority: me.pubkey(),
            protocol: p.protocol,
            user_account,
            market: market_pda,
            position,
            collateral_vault: p.collateral_vault,
            lp_pool: p.lp_pool,
            lp_vault: p.lp_vault,
            insurance_fund: p.insurance_fund,
            insurance_vault: p.insurance_vault,
            fee_vault: p.fee_vault,
            price_update,
            secondary_price_update: None,
            quote_conversion_price_update: None,
            token_program: TOKEN_PROGRAM,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::ClosePosition { price_limit }.data(),
    };
    println!(
        "close_position  {}",
        send(rpc, me, vec![ix], 200_000).await?
    );
    Ok(())
}

// --- helpers --------------------------------------------------------------------------------

/// A slippage bound `tolerance_bps` away from the oracle price.
///
/// A ceiling when buying, a floor when selling. There is no unbounded form: SolFX's
/// `validate_slippage` compares against the limit unconditionally, so a client that wants no
/// protection has to ask for a wide one and say so.
fn slippage_bound(oracle_price: i64, tolerance_bps: u32, buying: bool) -> Result<i64> {
    let price = u128::try_from(oracle_price).map_err(|_| anyhow!("negative oracle price"))?;
    let delta = price
        .checked_mul(u128::from(tolerance_bps))
        .and_then(|v| v.checked_div(10_000))
        .ok_or_else(|| anyhow!("slippage bound overflows"))?;
    let bound = if buying {
        price.checked_add(delta)
    } else {
        price.checked_sub(delta)
    }
    .ok_or_else(|| anyhow!("slippage bound overflows"))?;
    i64::try_from(bound).map_err(|_| anyhow!("slippage bound exceeds i64"))
}

/// Read the current price out of a posted Pyth update, **normalised to `PRICE_PRECISION`**.
///
/// A Pyth price is a mantissa plus its own exponent, and it is not the same exponent across
/// feeds — BTC/USD publishes at -8 while `PRICE_PRECISION` is 1e9. Using the raw mantissa
/// puts every derived number out by a factor of ten: the size, and the slippage bound the
/// size is checked against.
///
/// Normalisation goes through `solfx_math::oracle::normalize_price` — the same function the
/// program uses — rather than a local reimplementation, so the client and the chain cannot
/// disagree about what a price *is*.
async fn read_oracle_price(rpc: &RpcClient, price_update: &Pubkey) -> Result<i64> {
    let data = rpc
        .get_account_data(price_update)
        .await
        .context("reading the price account")?;
    let update = pyth::parse_update(&data)?;
    let m = update.price_message;
    if m.price <= 0 {
        bail!("oracle reports a non-positive price");
    }
    solfx_math::oracle::normalize_price(m.price, m.exponent).map_err(|e| {
        anyhow!(
            "normalising price (mantissa {}, exponent {}): {e:?}",
            m.price,
            m.exponent
        )
    })
}

/// Units of the base asset to base units — a straight scale by `BASE_PRECISION`.
///
/// The one sizing mode that needs no market data and no contract table: 1,000 EUR is 1,000
/// EUR regardless of price, lot convention, or asset class.
fn quantity_to_base(quantity: &str) -> Result<u64> {
    // A lot *is* a quantity, scaled. Reuse the decimal parser with a unit contract size so
    // the two modes cannot round differently.
    lots_to_base(quantity, 1)
}

/// Whole USD of exposure to base units, inverting `notional_in_quote`.
///
/// `notional = size_base * price / NOTIONAL_DIVISOR`, so
/// `size_base = notional * NOTIONAL_DIVISOR / price`.
fn notional_to_base(notional_usd: u64, price: i64) -> Result<u64> {
    if notional_usd == 0 {
        bail!("notional must be greater than zero");
    }
    let quote = u128::from(notional_usd)
        .checked_mul(u128::from(ONE_USDC))
        .ok_or_else(|| anyhow!("notional overflows"))?;
    let price = u128::try_from(price).map_err(|_| anyhow!("negative price"))?;
    let size = quote
        .checked_mul(NOTIONAL_DIVISOR)
        .and_then(|v| v.checked_div(price))
        .ok_or_else(|| anyhow!("size overflows"))?;
    if size == 0 {
        bail!("${notional_usd} is too small to buy one base unit at this price");
    }
    u64::try_from(size).map_err(|_| anyhow!("size exceeds u64"))
}

/// Lots to base units, parsed as an exact decimal.
///
/// `units_per_lot` differs per asset class, so this takes it rather than assuming FX.
/// Resolution is four decimal places, finer than any venue quotes.
///
/// No floating point anywhere: `"0.01"` becomes the integer 100 ten-thousandths and the
/// conversion is integer arithmetic from there. A float would make `0.07` land a hair off
/// and nobody would ever see it.
fn lots_to_base(lots: &str, units_per_lot: u128) -> Result<u64> {
    const PLACES: u32 = 4;
    let text = lots.trim();
    if text.is_empty() {
        bail!("no size given");
    }
    let (whole, frac) = match text.split_once('.') {
        Some((w, f)) => (w, f),
        None => (text, ""),
    };
    if whole.starts_with('-') {
        bail!("size must be positive");
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        bail!("{lots} is not a decimal number");
    }
    if frac.len() > PLACES as usize {
        bail!("at most {PLACES} decimal places; {lots} is finer than one ten-thousandth of a lot");
    }

    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().context("whole part")?
    };
    let frac_padded = format!("{frac:0<width$}", width = PLACES as usize);
    let frac: u128 = if frac_padded.is_empty() {
        0
    } else {
        frac_padded.parse().context("fraction")?
    };

    let scale = 10u128.pow(PLACES);
    let ten_thousandths = whole
        .checked_mul(scale)
        .and_then(|v| v.checked_add(frac))
        .ok_or_else(|| anyhow!("size overflows"))?;
    if ten_thousandths == 0 {
        bail!("size must be greater than zero");
    }
    let one_lot = units_per_lot
        .checked_mul(BASE_PRECISION)
        .ok_or_else(|| anyhow!("contract size overflows"))?;
    let base = one_lot
        .checked_mul(ten_thousandths)
        .and_then(|v| v.checked_div(scale))
        .ok_or_else(|| anyhow!("size overflows"))?;
    if base == 0 {
        bail!("{lots} lots rounds to zero base units");
    }
    u64::try_from(base).map_err(|_| anyhow!("size exceeds u64"))
}

/// USDC at 6dp, printed without floating point.
fn usdc(raw: u64) -> String {
    format!(
        "{}.{:06}",
        raw.checked_div(ONE_USDC).unwrap_or(0),
        raw.checked_rem(ONE_USDC).unwrap_or(0)
    )
}

/// Nonces with a live position on this market, ascending.
async fn open_nonces(rpc: &RpcClient, user_account: &Pubkey, market_index: u16) -> Vec<u8> {
    let mut found = Vec::new();
    for nonce in 0..MAX_NONCE_SCAN {
        if rpc
            .get_account(&position_pda(user_account, market_index, nonce))
            .await
            .is_ok()
        {
            found.push(nonce);
        }
    }
    found
}

fn position_pda(user_account: &Pubkey, market_index: u16, nonce: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            POSITION_SEED,
            user_account.as_ref(),
            &market_index.to_le_bytes(),
            &[nonce],
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

/// Parse a base58 pubkey from a JSON string.
///
/// `Pubkey`'s own `Deserialize` expects a 32-element byte array, but every file we read was
/// written with `.to_string()` and holds base58. Without this the failure is
/// `invalid type: string ..., expected an array of length 32`, which points at serde rather
/// than at the mismatch.
mod pubkey_str {
    use serde::Deserialize as _;
    use solana_pubkey::Pubkey;

    pub fn deserialize<'de, D>(d: D) -> Result<Pubkey, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

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

impl Deployment {
    fn load(path: &PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {} — run `init-protocol` first", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    fn market(&self, symbol: &str) -> Result<&MarketEntry> {
        self.markets
            .iter()
            .find(|m| m.symbol.eq_ignore_ascii_case(symbol))
            .ok_or_else(|| {
                anyhow!(
                    "{symbol} is not listed. Available: {}",
                    self.markets
                        .iter()
                        .map(|m| m.symbol.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}

fn load_price_accounts(path: &Path) -> Result<HashMap<String, Pubkey>> {
    Ok(price_map::by_symbol(&price_map::load(path)?))
}

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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::expect_used
)]
mod tests {
    use super::*;

    /// Lot sizing is the arithmetic a trader is most likely to be surprised by, and a
    /// mis-scaled size is a mis-sized trade rather than a failed one.
    #[test]
    fn lots_convert_to_base_units() {
        let fx = contracts::units_per_lot("EUR/USD");
        assert_eq!(lots_to_base("1", fx).unwrap(), 100_000_000_000_000);
        assert_eq!(lots_to_base("0.1", fx).unwrap(), 10_000_000_000_000);
        assert_eq!(lots_to_base("0.01", fx).unwrap(), 1_000_000_000_000);
        assert_eq!(
            lots_to_base(" 0.01 ", fx).unwrap(),
            1_000_000_000_000,
            "whitespace ok"
        );
    }

    /// The bug this table exists to prevent: one lot of Bitcoin is one Bitcoin, not the
    /// 100,000 units an FX lot carries. Getting it wrong oversizes a BTC order 100,000x
    /// while still looking like a reasonable number on screen.
    #[test]
    fn a_lot_means_different_amounts_per_asset_class() {
        assert_eq!(
            contracts::units_per_lot("EUR/USD"),
            100_000,
            "FX: 100,000 base units"
        );
        assert_eq!(contracts::units_per_lot("USD/INR"), 100_000);
        assert_eq!(
            contracts::units_per_lot("XAU/USD"),
            100,
            "gold: 100 troy ounces"
        );
        assert_eq!(contracts::units_per_lot("BTC/USD"), 1, "crypto: one coin");
        assert_eq!(contracts::units_per_lot("ETH/USD"), 1);

        // One lot of BTC is 1 BTC at BASE_PRECISION.
        assert_eq!(
            lots_to_base("1", contracts::units_per_lot("BTC/USD")).unwrap(),
            1_000_000_000
        );
        // One lot of gold is 100 oz.
        assert_eq!(
            lots_to_base("1", contracts::units_per_lot("XAU/USD")).unwrap(),
            100_000_000_000
        );
    }

    /// The bug that cost two live attempts: a raw Pyth mantissa is not a `PRICE_PRECISION`
    /// price. BTC/USD publishes at exponent -8, so the raw number is a tenth of what the
    /// engine works in — and both the position size and its slippage bound inherit the error.
    #[test]
    fn pyth_exponents_normalise_to_price_precision() {
        // $77,401.22 at exponent -8, as BTC/USD actually publishes it.
        let normalised = solfx_math::oracle::normalize_price(7_740_122_557_220, -8).unwrap();
        assert_eq!(normalised, 77_401_225_572_200, "must scale up to 1e9");

        // A feed already at -9 must pass through untouched.
        assert_eq!(
            solfx_math::oracle::normalize_price(1_085_430_000, -9).unwrap(),
            1_085_430_000
        );
    }

    /// A buy bound must sit above the price and a sell bound below, or every fill is
    /// rejected — which is exactly the bug that made the first live trade fail.
    #[test]
    fn slippage_bounds_sit_on_the_right_side() {
        let price = 77_000_000_000_000i64;
        let buy = slippage_bound(price, 100, true).unwrap();
        let sell = slippage_bound(price, 100, false).unwrap();
        assert!(buy > price, "a buy ceiling must be above the price");
        assert!(sell < price, "a sell floor must be below the price");
        assert_eq!(buy - price, price / 100, "100 bps is 1%");
        assert_eq!(price - sell, price / 100);
        assert_eq!(
            slippage_bound(price, 0, true).unwrap(),
            price,
            "zero tolerance is exact"
        );
    }

    /// The three sizing modes must agree where they overlap. `--quantity` is the anchor:
    /// it is the only one with no dependency on price or contract size.
    #[test]
    fn the_three_sizing_modes_agree() {
        // 1,000 EUR by quantity == 0.01 lots of a 100,000-unit FX contract.
        let by_quantity = quantity_to_base("1000").unwrap();
        let by_lots = lots_to_base("0.01", contracts::units_per_lot("EUR/USD")).unwrap();
        assert_eq!(by_quantity, by_lots, "1,000 EUR is 0.01 FX lots");

        // 1 BTC by quantity == 1 lot of BTC, because a BTC lot is one coin.
        assert_eq!(
            quantity_to_base("1").unwrap(),
            lots_to_base("1", contracts::units_per_lot("BTC/USD")).unwrap()
        );

        // And by dollars, at $77,000, $77,000 buys 1 BTC.
        let by_notional = notional_to_base(77_000, 77_000_000_000_000).unwrap();
        let one_btc = quantity_to_base("1").unwrap();
        let diff = by_notional.abs_diff(one_btc);
        assert!(
            diff < one_btc / 1_000,
            "within 0.1%: {by_notional} vs {one_btc}"
        );
    }

    #[test]
    fn quantity_accepts_decimals_and_rejects_nonsense() {
        assert_eq!(quantity_to_base("0.5").unwrap(), 500_000_000);
        assert_eq!(quantity_to_base("1000").unwrap(), 1_000_000_000_000);
        assert!(quantity_to_base("0").is_err());
        assert!(quantity_to_base("-1").is_err());
        assert!(quantity_to_base("abc").is_err());
    }

    /// `--notional 1000` must mean $1,000 of exposure whatever the asset costs.
    #[test]
    fn notional_sizing_inverts_the_notional_formula() {
        // BTC at $77,000 (PRICE_PRECISION = 1e9).
        let btc = 77_000_000_000_000i64;
        let size = notional_to_base(1_000, btc).unwrap();
        // Round-trip: size * price / NOTIONAL_DIVISOR back to quote units.
        let back = u128::from(size) * u128::try_from(btc).unwrap() / NOTIONAL_DIVISOR;
        let dollars = back / u128::from(ONE_USDC);
        assert!(
            (999..=1001).contains(&dollars),
            "got ${dollars}, want ~$1000"
        );
    }

    #[test]
    fn notional_rejects_nonsense() {
        assert!(notional_to_base(0, 77_000_000_000_000).is_err());
    }

    /// The case a float would get wrong silently: 0.07 is not representable in binary, so
    /// `0.07 * 10_000.0` is 699.9999... and truncation would size the trade one
    /// ten-thousandth of a lot short.
    #[test]
    fn awkward_decimals_are_exact() {
        let fx = contracts::units_per_lot("EUR/USD");
        assert_eq!(lots_to_base("0.07", fx).unwrap(), 7_000_000_000_000);
        assert_eq!(lots_to_base("0.29", fx).unwrap(), 29_000_000_000_000);
        assert_eq!(lots_to_base("2.35", fx).unwrap(), 235_000_000_000_000);
    }

    #[test]
    fn nonsense_sizes_are_rejected() {
        let fx = contracts::units_per_lot("EUR/USD");
        assert!(lots_to_base("0", fx).is_err());
        assert!(lots_to_base("-1", fx).is_err());
        assert!(lots_to_base("", fx).is_err());
        assert!(lots_to_base("abc", fx).is_err());
        assert!(lots_to_base("1.2.3", fx).is_err());
        assert!(
            lots_to_base("0.00001", fx).is_err(),
            "finer than the resolution"
        );
    }

    #[test]
    fn usdc_is_formatted_without_floating_point() {
        assert_eq!(usdc(1_000_000), "1.000000");
        assert_eq!(usdc(1_500_000), "1.500000");
        assert_eq!(usdc(0), "0.000000");
        assert_eq!(usdc(1), "0.000001");
    }

    /// Both files are written with `.to_string()`, so they hold base58 — and `Pubkey`'s
    /// derived `Deserialize` wants a byte array. Getting this wrong fails at the first
    /// command with an error that names serde, not the format mismatch.
    #[test]
    fn deployment_json_parses_base58_pubkeys() {
        let json = r#"{
            "usdc_mint": "RoRNXvd8ePRvk5oFFgd2ZRaeAwHfn68jXcwjU7CwNmp",
            "markets": [{"market_index": 0, "symbol": "BTC/USD"}]
        }"#;
        let d: Deployment = serde_json::from_str(json).expect("must accept base58 strings");
        assert_eq!(
            d.usdc_mint.to_string(),
            "RoRNXvd8ePRvk5oFFgd2ZRaeAwHfn68jXcwjU7CwNmp"
        );
        assert_eq!(
            d.market("btc/usd").unwrap().market_index,
            0,
            "lookup is case-insensitive"
        );
    }

    /// Two positions on the same market must not collide, or a second trade would silently
    /// overwrite the first.
    #[test]
    fn nonce_separates_positions_on_one_market() {
        let user = Pubkey::new_unique();
        assert_ne!(position_pda(&user, 0, 0), position_pda(&user, 0, 1));
        assert_ne!(position_pda(&user, 0, 0), position_pda(&user, 1, 0));
        assert_eq!(position_pda(&user, 0, 0), position_pda(&user, 0, 0));
    }
}
