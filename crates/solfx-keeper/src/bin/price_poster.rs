//! Publishes Pyth prices on a cluster where nobody else does.
//!
//! # Why this exists
//!
//! Pyth is a **pull** oracle: the on-chain price account only advances when somebody posts a
//! signed update to it. On mainnet a sponsor does that for every SolFX feed, so the protocol
//! can simply read. On devnet almost nobody does — measured 2026-08-23, only 8 of the 33
//! markets had a live feed, and 9 more had accounts frozen between 15 and 205 days
//! (`docs/CONTEXT.md`). Deploying against that gives a protocol that lists 33 markets and
//! rejects trades on 25 of them, because `MAX_ALLOWED_STALENESS_SECONDS` is 60 and it is
//! doing its job.
//!
//! So we post them ourselves. **The data is still Pyth's** — guardian-signed on Pythnet,
//! verified on chain by the receiver against the Wormhole guardian set. This binary is a
//! courier, not a source. It cannot forge a price; the worst it can do is fail to deliver
//! one, which the protocol already handles as staleness.
//!
//! # Why this takes four transactions and not one
//!
//! `post_update_atomic` puts the VAA inline and verifies it in the same instruction. It is
//! the obvious choice, and it does not work here: it can only ever produce
//! `VerificationLevel::Partial`, and SolFX requires `Full` (`oracle.rs`, gate 3). That is
//! the right call on SolFX's part — the receiver SDK itself warns that partial updates
//! "lower the threshold of guardians that need to collude to produce a malicious price
//! update".
//!
//! Passing every signature to the atomic path does not rescue it. **Measured: 2364 bytes
//! against a 1232-byte limit**, roughly double. That is why the receiver offers
//! `minimum_signatures` at all.
//!
//! So each feed goes the long way, which is what the sponsored publishers do:
//!
//! 1. `init_encoded_vaa` — allocate a buffer account on the Wormhole program
//! 2. `write_encoded_vaa` — stream the VAA into it, in chunks that fit a packet
//! 3. `verify_encoded_vaa_v1` — check every guardian signature
//! 4. `post_update` — the receiver reads the verified VAA and writes `Full`
//!
//! Then the buffer is closed to reclaim its rent.
//!
//! # Why the protocol accepts what we post
//!
//! `price_update` is declared `Box<Account<'info, PriceUpdateV2>>` with **no seeds
//! constraint**. Two things are checked, and we satisfy both honestly:
//!
//! 1. the account is owned by the Pyth receiver — Anchor's `Account<T>` owner check;
//! 2. the update's `feed_id` equals `market.pyth_feed_id` — `get_price_unchecked` in
//!    `solfx_core::oracle`.
//!
//! Nothing here weakens either. An account this binary creates is created *by the receiver*,
//! from a VAA the receiver verified.
//!
//! # Addresses are stable across restarts
//!
//! `post_update_atomic` needs the price-update account to sign, so each feed gets a keypair,
//! persisted under `--keys-dir`. Re-running reuses them, so the addresses handed to the
//! keeper and the frontend do not move. Losing that directory is not fatal — it orphans the
//! old accounts and mints new ones — but it does mean re-publishing the map, so keep it.
//!
//! # Running it
//!
//! ```text
//! solana airdrop 5                       # devnet SOL for rent and the per-update fee
//! cargo run -p solfx-keeper --bin price-poster -- --once      # one pass, then exit
//! cargo run -p solfx-keeper --bin price-poster                # keep posting every 10s
//! ```
//!
//! `--once` writes `price-accounts.json`, the feed-id → account map everything else needs.

#[allow(dead_code)]
#[path = "../pyth.rs"]
mod pyth;

use std::collections::HashMap;
use std::path::PathBuf;

use anchor_lang::AccountDeserialize;
use anyhow::{anyhow, bail, Context as _, Result};
use clap::Parser as _;
use pyth_solana_receiver_sdk::config::Config as ReceiverConfig;
use pyth_solana_receiver_sdk::pda::{get_config_address, get_treasury_address};
use pyth_solana_receiver_sdk::PostUpdateParams;
use pythnet_sdk::wire::v1::{AccumulatorUpdateData, Proof};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer as _;
use solana_transaction::Transaction;

/// The markets to keep fresh. Mirrors `devnet_feed_probe.rs`; the two are meant to be read
/// together, one measuring the gap and one closing it.
const MARKETS: &[&str] = &[
    "EUR/USD",
    "GBP/USD",
    "USD/JPY",
    "USD/CAD",
    "AUD/USD",
    "EUR/JPY",
    "GBP/JPY",
    "CAD/JPY",
    "CHF/JPY",
    "EUR/GBP",
    "EUR/AUD",
    "EUR/CHF",
    "AUD/JPY",
    "USD/CHF",
    "NZD/USD",
    "XAU/USD",
    "XAG/USD",
    "XPT/USD",
    "XPD/USD",
    "USD/MXN",
    "USD/ZAR",
    "USD/PHP",
    "USD/INR",
    "USD/TRY",
    "USD/TWD",
    "USD/KRW",
    "USD/BRL",
    "USD/CLP",
    "USD/PEN",
    "BTC/USD",
    "ETH/USD",
    "SOL/USD",
    "USOILSPOT/USD",
];

/// Wormhole's guardian-set PDA seed. The receiver checks the VAA against this account.
const GUARDIAN_SET_SEED: &[u8] = b"GuardianSet";

#[derive(clap::Parser)]
#[command(
    name = "price-poster",
    about = "Post Pyth prices to a cluster with no sponsor"
)]
struct Args {
    #[arg(long, default_value = "https://api.devnet.solana.com")]
    rpc_url: String,
    #[arg(long, default_value = pyth::DEFAULT_HERMES_URL)]
    hermes_url: String,

    /// Hermes API key. Pyth put the public endpoint behind authentication on 26 Aug 2026;
    /// without this every request returns 401 and no price can be posted on any cluster.
    /// Get one free at https://pythdata.app.
    #[arg(long, env = "PYTH_API_KEY")]
    hermes_token: Option<String>,
    /// Payer and write authority. Must hold SOL for rent and the receiver's update fee.
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
    /// Where per-feed price-update keypairs live, so addresses survive a restart.
    #[arg(long, default_value = "price-accounts")]
    keys_dir: PathBuf,
    /// Where to write the feed-id → account map.
    #[arg(long, default_value = "price-accounts.json")]
    map_out: PathBuf,
    /// One pass and exit, rather than looping.
    #[arg(long)]
    once: bool,
    /// Seconds between passes.
    #[arg(long, default_value_t = 10)]
    interval_secs: u64,
    /// Build and log transactions without sending them.
    #[arg(long)]
    dry_run: bool,
    /// Post only the feeds named in this deployment file.
    ///
    /// The full-verification path costs ~5 transactions per feed, so a 33-feed sweep takes
    /// minutes and every feed is stale by its next turn. Posting only what is actually
    /// listed is the difference between a working cluster and one that rejects every trade
    /// on `OracleStale`. Pass `--all-feeds` to override.
    #[arg(long, default_value = "deployment.json")]
    deployment: PathBuf,
    /// Post every feed in `MARKETS`, ignoring the deployment file.
    #[arg(long)]
    all_feeds: bool,
    /// Print the `--clone` arguments a local validator needs, then exit.
    ///
    /// `scripts/deploy-localnet.sh` clones the Pyth *programs*, which is not enough:
    /// `post_update_atomic` also reads the receiver's config and treasury PDAs and the
    /// Wormhole guardian set, and a bare validator has none of them. Their addresses are
    /// derived rather than listed, so a guardian-set rotation is picked up automatically.
    #[arg(long)]
    print_clone_args: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

async fn run(args: Args) -> Result<()> {
    if args.print_clone_args {
        return print_clone_args(&args).await;
    }
    let payer = load_keypair(&args.keypair)?;
    let rpc = RpcClient::new_with_commitment(args.rpc_url.clone(), CommitmentConfig::confirmed());

    println!("price-poster");
    println!("  rpc      {}", args.rpc_url);
    println!("  payer    {}", payer.pubkey());
    println!("  receiver {}", pyth_solana_receiver_sdk::ID);

    // Read the receiver's own config rather than hardcoding: it names the Wormhole program
    // this deployment trusts, the fee per update, and — the part that decides whether a
    // transaction fits at all — how many guardian signatures it will settle for.
    let config_addr = get_config_address();
    let config_data = rpc
        .get_account_data(&config_addr)
        .await
        .context("reading the Pyth receiver config — is the receiver deployed on this cluster?")?;
    let config = ReceiverConfig::try_deserialize(&mut config_data.as_slice())
        .map_err(|e| anyhow!("deserializing receiver config: {e}"))?;
    println!("  wormhole {}", config.wormhole);
    println!("  min sigs {}", config.minimum_signatures);
    println!(
        "  fee      {} lamports/update\n",
        config.single_update_fee_in_lamports
    );

    let balance = rpc.get_balance(&payer.pubkey()).await.unwrap_or(0);
    if balance == 0 && !args.dry_run {
        bail!(
            "payer has no SOL. On devnet: solana airdrop 5 {}",
            payer.pubkey()
        );
    }

    // Narrow to the listed markets before touching Hermes: publishing a price nothing can
    // trade against costs ~5 transactions, and doing it 28 extra times is what pushes the
    // markets that *are* listed past their staleness window.
    let wanted: Vec<String> = if args.all_feeds {
        MARKETS.iter().map(|s| (*s).to_string()).collect()
    } else {
        match listed_symbols(&args.deployment) {
            Ok(listed) if !listed.is_empty() => {
                println!(
                    "  posting the {} markets listed in {}",
                    listed.len(),
                    args.deployment.display()
                );
                listed
            }
            _ => {
                println!(
                    "  no usable {} — posting all {} feeds (slow: every feed may go stale)",
                    args.deployment.display(),
                    MARKETS.len()
                );
                MARKETS.iter().map(|s| (*s).to_string()).collect()
            }
        }
    };
    let wanted_refs: Vec<&str> = wanted.iter().map(String::as_str).collect();

    // One shared resolver for every binary — see `pyth::resolve_feed_ids`. The poster used to
    // carry its own copy and, after `init-protocol` was corrected, still resolved BTC/USD to
    // a Deribit funding rate while the protocol had the spot feed.
    let resolved =
        pyth::resolve_feed_ids(&args.hermes_url, args.hermes_token.as_deref(), &wanted_refs)
            .await?;
    // Preserve the requested order so the map and the posting sequence are stable.
    let feeds: Vec<(String, String)> = wanted
        .iter()
        .filter_map(|sym| resolved.get(sym).map(|f| (sym.clone(), f.feed_id.clone())))
        .collect();
    println!(
        "resolved {} of {} symbols from Hermes\n",
        feeds.len(),
        wanted.len()
    );

    let feeds = screen_entitlements(&args.hermes_url, args.hermes_token.as_deref(), feeds).await?;

    std::fs::create_dir_all(&args.keys_dir)
        .with_context(|| format!("creating {}", args.keys_dir.display()))?;

    // One keypair per feed, created once and reused, so downstream addresses are stable.
    let mut accounts: HashMap<String, Keypair> = HashMap::new();
    for (symbol, feed_id) in &feeds {
        let path = args.keys_dir.join(format!("{}.json", feed_id));
        let kp = if path.exists() {
            load_keypair(&path.to_string_lossy())
                .with_context(|| format!("loading {}", path.display()))?
        } else {
            let kp = Keypair::new();
            std::fs::write(&path, serde_json::to_string(&kp.to_bytes().to_vec())?)
                .with_context(|| format!("writing {}", path.display()))?;
            kp
        };
        accounts.insert(symbol.clone(), kp);
    }
    write_map(&args.map_out, &feeds, &accounts)?;
    println!("wrote {}\n", args.map_out.display());

    loop {
        match post_pass(&rpc, &args, &payer, &config, &feeds, &accounts).await {
            Ok((ok, failed)) => println!("pass complete: {ok} posted, {failed} failed"),
            Err(e) => eprintln!("pass failed: {e:#}"),
        }
        if args.once {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(args.interval_secs)).await;
    }
}

/// One full sweep: fetch every feed's latest update from Hermes and post each on chain.
///
/// Feeds are fetched in chunks but posted one transaction at a time. Batching instructions
/// would not help: `PostUpdateAtomicParams` carries its own copy of the VAA, so two
/// instructions in one transaction means two VAAs, and the packet limit arrives immediately.
async fn post_pass(
    rpc: &RpcClient,
    args: &Args,
    payer: &Keypair,
    config: &ReceiverConfig,
    feeds: &[(String, String)],
    accounts: &HashMap<String, Keypair>,
) -> Result<(usize, usize)> {
    let (mut ok, mut failed) = (0usize, 0usize);

    for chunk in feeds.chunks(10) {
        let ids: Vec<String> = chunk.iter().map(|(_, id)| id.clone()).collect();
        let blobs = match fetch_updates(&args.hermes_url, args.hermes_token.as_deref(), &ids).await
        {
            Ok(b) => b,
            Err(e) => {
                eprintln!("  hermes fetch failed for {} feeds: {e:#}", ids.len());
                failed = failed.saturating_add(chunk.len());
                continue;
            }
        };

        for blob in blobs {
            let decoded = match AccumulatorUpdateData::try_from_slice(&blob) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("  undecodable accumulator blob: {e:?}");
                    failed = failed.saturating_add(1);
                    continue;
                }
            };
            let Proof::WormholeMerkle { vaa, updates } = decoded.proof;
            let vaa_bytes: Vec<u8> = vaa.into();

            // A full 13-signature VAA does not fit alongside everything else in a 1232-byte
            // packet. The receiver anticipates this: it accepts `minimum_signatures`, and
            // trimming to exactly that is what makes the atomic path viable.
            let guardian_set = guardian_set_address(&config.wormhole, &vaa_bytes)?;

            for update in updates {
                // The feed id sits at bytes 1..33 of the price message.
                let msg: Vec<u8> = update.message.clone().into();
                let Some(feed_id) = msg.get(1..33).map(hex::encode) else {
                    failed = failed.saturating_add(1);
                    continue;
                };
                let Some((symbol, _)) = feeds.iter().find(|(_, id)| *id == feed_id) else {
                    continue; // not one of ours
                };
                let Some(account) = accounts.get(symbol) else {
                    continue;
                };

                if args.dry_run {
                    println!("  [dry-run] {symbol:<14} -> {}", account.pubkey());
                    ok = ok.saturating_add(1);
                    continue;
                }

                match post_one_full(
                    rpc,
                    payer,
                    &config.wormhole,
                    guardian_set,
                    &vaa_bytes,
                    &update,
                    account,
                )
                .await
                {
                    Ok(sig) => {
                        println!("  {symbol:<14} {} {sig}", account.pubkey());
                        ok = ok.saturating_add(1);
                    }
                    Err(e) => {
                        eprintln!("  {symbol:<14} FAILED: {e:#}");
                        failed = failed.saturating_add(1);
                    }
                }
            }
        }
    }
    Ok((ok, failed))
}

/// Anchor discriminators on the Wormhole core bridge and the Pyth receiver.
/// `sha256("global:<name>")[..8]`; `discriminators_match_anchors_derivation` re-derives them.
const IX_INIT_ENCODED_VAA: [u8; 8] = [209, 193, 173, 25, 91, 202, 181, 218];
const IX_WRITE_ENCODED_VAA: [u8; 8] = [199, 208, 110, 177, 150, 76, 118, 42];
const IX_VERIFY_ENCODED_VAA: [u8; 8] = [103, 56, 177, 229, 240, 103, 68, 73];
const IX_CLOSE_ENCODED_VAA: [u8; 8] = [48, 221, 174, 198, 231, 7, 152, 38];
const IX_POST_UPDATE: [u8; 8] = [133, 95, 207, 175, 11, 79, 118, 44];

/// Bytes an `EncodedVaa` needs before its payload:
/// discriminator(8) + status(1) + write_authority(32) + version(1) + `Vec` length(4).
const ENCODED_VAA_HEADER: usize = 46;

/// How much VAA to put in one `write_encoded_vaa`. Conservative: the instruction also
/// carries a discriminator, an index and a length prefix, and the transaction carries two
/// signatures and three account metas.
const VAA_CHUNK: usize = 700;

fn meta(pubkey: Pubkey, is_signer: bool, is_writable: bool) -> solana_instruction::AccountMeta {
    solana_instruction::AccountMeta {
        pubkey,
        is_signer,
        is_writable,
    }
}

/// Publish one feed at `Full` verification.
///
/// Returns the signature of the `post_update` that actually wrote the price. Earlier
/// transactions in the sequence are plumbing; if any fails the whole feed fails, and the
/// buffer account is closed on the way out so a failure does not leak rent.
async fn post_one_full(
    rpc: &RpcClient,
    payer: &Keypair,
    wormhole: &Pubkey,
    guardian_set: Pubkey,
    vaa: &[u8],
    merkle_price_update: &pythnet_sdk::wire::v1::MerklePriceUpdate,
    price_update_account: &Keypair,
) -> Result<String> {
    let encoded_vaa = Keypair::new();
    let space = ENCODED_VAA_HEADER
        .checked_add(vaa.len())
        .ok_or_else(|| anyhow!("VAA too large"))?;
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(space)
        .await
        .context("rent for the VAA buffer")?;

    // 1. Allocate, owned by Wormhole, and hand it to `init_encoded_vaa` in the same
    //    transaction: `#[account(zero)]` requires the account to exist, be owned by the
    //    program, and still be zeroed, which is only reliably true right after creation.
    let create = create_account_ix(
        &payer.pubkey(),
        &encoded_vaa.pubkey(),
        rent,
        u64::try_from(space)?,
        wormhole,
    );
    let init = Instruction {
        program_id: *wormhole,
        accounts: vec![
            meta(payer.pubkey(), true, false),       // write_authority
            meta(encoded_vaa.pubkey(), false, true), // encoded_vaa
        ],
        data: IX_INIT_ENCODED_VAA.to_vec(),
    };
    send_signed(rpc, payer, &[&encoded_vaa], vec![create, init], 100_000)
        .await
        .context("init_encoded_vaa")?;

    // 2. Stream the VAA in. A 13-signature VAA is ~900 bytes and does not fit in one
    //    instruction alongside its own overhead.
    let mut offset = 0usize;
    while offset < vaa.len() {
        let end = offset.saturating_add(VAA_CHUNK).min(vaa.len());
        let chunk = vaa
            .get(offset..end)
            .ok_or_else(|| anyhow!("chunk out of range"))?;
        let mut data = IX_WRITE_ENCODED_VAA.to_vec();
        data.extend_from_slice(&u32::try_from(offset)?.to_le_bytes());
        data.extend_from_slice(&u32::try_from(chunk.len())?.to_le_bytes());
        data.extend_from_slice(chunk);
        let write = Instruction {
            program_id: *wormhole,
            accounts: vec![
                meta(payer.pubkey(), true, false),
                meta(encoded_vaa.pubkey(), false, true),
            ],
            data,
        };
        send_signed(rpc, payer, &[], vec![write], 100_000)
            .await
            .with_context(|| format!("write_encoded_vaa at offset {offset}"))?;
        offset = end;
    }

    // 3. Verify every signature against the guardian set. This is the step that earns
    //    `Full`, and the reason the Wormhole program has to be on the cluster at all.
    let verify = Instruction {
        program_id: *wormhole,
        accounts: vec![
            meta(payer.pubkey(), true, false),
            meta(encoded_vaa.pubkey(), false, true),
            meta(guardian_set, false, false),
        ],
        data: IX_VERIFY_ENCODED_VAA.to_vec(),
    };
    send_signed(rpc, payer, &[], vec![verify], 400_000)
        .await
        .context("verify_encoded_vaa_v1 — is the guardian set cloned and current?")?;

    // 4. The receiver reads the verified VAA and writes the price at `Full`.
    let mut data = IX_POST_UPDATE.to_vec();
    data.extend_from_slice(
        &borsh::to_vec(&PostUpdateParams {
            merkle_price_update: merkle_price_update.clone(),
            treasury_id: 0,
        })
        .unwrap_or_default(),
    );
    let post = Instruction {
        program_id: pyth_solana_receiver_sdk::ID,
        accounts: vec![
            meta(payer.pubkey(), true, true),                // payer
            meta(encoded_vaa.pubkey(), false, false),        // encoded_vaa
            meta(get_config_address(), false, false),        // config
            meta(get_treasury_address(0), false, true),      // treasury
            meta(price_update_account.pubkey(), true, true), // price_update_account
            meta(anchor_lang::system_program::ID, false, false),
            meta(payer.pubkey(), true, false), // write_authority
        ],
        data,
    };
    let sig = send_signed(rpc, payer, &[price_update_account], vec![post], 400_000)
        .await
        .context("post_update")?;

    // 5. Reclaim the buffer's rent. Best-effort: the price is already written, and a leaked
    //    buffer is a cost, not a correctness problem.
    let close = Instruction {
        program_id: *wormhole,
        accounts: vec![
            meta(payer.pubkey(), true, true),
            meta(encoded_vaa.pubkey(), false, true),
        ],
        data: IX_CLOSE_ENCODED_VAA.to_vec(),
    };
    let _ = send_signed(rpc, payer, &[], vec![close], 60_000).await;

    Ok(sig)
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

async fn send_signed(
    rpc: &RpcClient,
    payer: &Keypair,
    extra: &[&Keypair],
    ixs: Vec<Instruction>,
    cu: u32,
) -> Result<String> {
    let mut all = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(cu),
        ComputeBudgetInstruction::set_compute_unit_price(1_000),
    ];
    all.extend(ixs);
    let blockhash = rpc.get_latest_blockhash().await.context("blockhash")?;
    let message = Message::new(&all, Some(&payer.pubkey()));
    let mut tx = Transaction::new_unsigned(message);
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra);
    tx.try_sign(&signers, blockhash).context("signing")?;
    Ok(rpc.send_and_confirm_transaction(&tx).await?.to_string())
}

/// The guardian set the receiver will check this VAA against.
///
/// The index is in the VAA header, so it follows the VAA rather than being configured —
/// a guardian-set rotation then needs no change here.
fn guardian_set_address(wormhole: &Pubkey, vaa: &[u8]) -> Result<Pubkey> {
    let raw: [u8; 4] = vaa
        .get(1..5)
        .ok_or_else(|| anyhow!("VAA too short for a guardian set index"))?
        .try_into()?;
    let index = u32::from_be_bytes(raw);
    Ok(Pubkey::find_program_address(&[GUARDIAN_SET_SEED, &index.to_be_bytes()], wormhole).0)
}

/// Work out which Pyth accounts a local validator must be seeded with, and print them as
/// `solana-test-validator` arguments.
///
/// Everything is read from a live cluster rather than hardcoded: the Wormhole address comes
/// from the receiver's own config, and the guardian-set index from a VAA fetched right now.
async fn print_clone_args(args: &Args) -> Result<()> {
    // Read from a cluster that actually has Pyth on it — by definition not the local one.
    let source = if args.rpc_url.contains("127.0.0.1") || args.rpc_url.contains("localhost") {
        "https://api.devnet.solana.com".to_string()
    } else {
        args.rpc_url.clone()
    };
    let rpc = RpcClient::new_with_commitment(source.clone(), CommitmentConfig::confirmed());

    let config_addr = get_config_address();
    let config_data = rpc
        .get_account_data(&config_addr)
        .await
        .context("reading the Pyth receiver config")?;
    let config = ReceiverConfig::try_deserialize(&mut config_data.as_slice())
        .map_err(|e| anyhow!("deserializing receiver config: {e}"))?;

    // The guardian set index is a property of the VAAs currently being signed, so ask for a
    // real one rather than assuming. BTC/USD is the safest probe: it publishes everywhere.
    let btc = "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43".to_string();
    let blobs = fetch_updates(&args.hermes_url, args.hermes_token.as_deref(), &[btc]).await?;
    let blob = blobs
        .first()
        .ok_or_else(|| anyhow!("Hermes returned no update"))?;
    let decoded = AccumulatorUpdateData::try_from_slice(blob)
        .map_err(|e| anyhow!("decoding accumulator: {e:?}"))?;
    let Proof::WormholeMerkle { vaa, .. } = decoded.proof;
    let vaa_bytes: Vec<u8> = vaa.into();
    let guardian_set = guardian_set_address(&config.wormhole, &vaa_bytes)?;

    eprintln!("# read from {source}");
    eprintln!("# receiver     {}", pyth_solana_receiver_sdk::ID);
    eprintln!(
        "# wormhole     {} (program — needed by verify_encoded_vaa_v1)",
        config.wormhole
    );
    eprintln!("# guardian set {guardian_set}");
    eprintln!();
    // The Wormhole program itself, not just its accounts: reaching `VerificationLevel::Full`
    // means calling `verify_encoded_vaa_v1` *on* it. `post_update_atomic` avoids that, but it
    // can only ever produce `Partial`, which SolFX refuses.
    println!(
        "--clone-upgradeable-program {} --clone {config_addr} --clone {} --clone {guardian_set}",
        config.wormhole,
        get_treasury_address(0)
    );
    Ok(())
}

/// Symbols from a deployment written by `init-protocol`.
fn listed_symbols(path: &PathBuf) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Deployment {
        markets: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        symbol: String,
    }
    let text = std::fs::read_to_string(path)?;
    let d: Deployment = serde_json::from_str(&text)?;
    Ok(d.markets.into_iter().map(|m| m.symbol).collect())
}

// --- Hermes ------------------------------------------------------------------------------

/// Symbol → feed id, for the symbols asked for, in the order given.
#[derive(serde::Deserialize)]
struct LatestResponse {
    binary: BinaryData,
}

#[derive(serde::Deserialize)]
struct BinaryData {
    data: Vec<String>,
}

/// The signed accumulator blobs for these feeds, hex-decoded.
/// Drop feeds this API key is not entitled to, once, at startup.
///
/// # Why this is necessary rather than defensive
///
/// Hermes has no partial-entitlement mode. A single request carrying one feed the key cannot
/// read returns **403 for the whole batch** — verified: five feeds together 403, the three
/// entitled ones alone 200, and the documented `ignore_invalid_price_ids=true` does not help
/// because it covers malformed ids, not ungranted ones.
///
/// So one unentitled market silently takes down every other market in its chunk. Before this
/// screen the poster reported `0 posted, 5 failed` forever while three of those five were
/// perfectly readable.
///
/// The probe runs once. In the normal case (everything entitled) it costs a single request
/// and the loop proceeds untouched; only on a 403 does it fall back to one request per feed
/// to find out which ones are the problem. Steady state is unchanged either way.
async fn screen_entitlements(
    hermes: &str,
    token: Option<&str>,
    feeds: Vec<(String, String)>,
) -> Result<Vec<(String, String)>> {
    if feeds.is_empty() {
        return Ok(feeds);
    }

    let all: Vec<String> = feeds.iter().map(|(_, id)| id.clone()).collect();
    if fetch_updates(hermes, token, &all).await.is_ok() {
        return Ok(feeds);
    }

    println!("hermes refused the full feed set — checking each feed individually");
    let mut usable = Vec::new();
    let mut denied = Vec::new();
    for (symbol, id) in feeds {
        match fetch_updates(hermes, token, std::slice::from_ref(&id)).await {
            Ok(_) => usable.push((symbol, id)),
            Err(e) => denied.push((symbol, format!("{e:#}"))),
        }
    }

    if !denied.is_empty() {
        println!("\n  {} feed(s) this API key cannot read:", denied.len());
        for (symbol, why) in &denied {
            let reason = if why.contains("403") {
                "not entitled on this Pyth plan"
            } else {
                "unavailable"
            };
            println!("    {symbol:<12} {reason}");
        }
        println!(
            "  Those markets will have no price and every trade on them will be refused on\n               staleness. Raise the plan at https://pythdata.app to include them.\n"
        );
    }

    if usable.is_empty() {
        anyhow::bail!(
            "no feed is readable with this API key — nothing to post. Check PYTH_API_KEY, \
             then confirm the plan covers the listed markets."
        );
    }
    println!("  posting {} usable feed(s)\n", usable.len());
    Ok(usable)
}

async fn fetch_updates(hermes: &str, token: Option<&str>, ids: &[String]) -> Result<Vec<Vec<u8>>> {
    let query = ids
        .iter()
        .map(|id| format!("ids[]={id}"))
        .collect::<Vec<_>>()
        .join("&");
    let url = format!(
        "{}/v2/updates/price/latest?{query}&encoding=hex",
        hermes.trim_end_matches('/')
    );
    let http = pyth::hermes_client(std::time::Duration::from_secs(30), token)?;
    let body: LatestResponse = http
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("parsing the Hermes update response")?;

    body.binary
        .data
        .iter()
        .map(|h| hex::decode(h.trim_start_matches("0x")).context("decoding an update blob"))
        .collect()
}

// --- plumbing ----------------------------------------------------------------------------

fn load_keypair(path: &str) -> Result<Keypair> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").context("HOME is not set")?;
        format!("{home}/{rest}")
    } else {
        path.to_string()
    };
    let bytes: Vec<u8> = serde_json::from_str(
        &std::fs::read_to_string(&expanded).with_context(|| format!("reading {expanded}"))?,
    )
    .with_context(|| format!("parsing {expanded} as a keypair array"))?;
    Keypair::try_from(bytes.as_slice()).map_err(|e| anyhow!("invalid keypair in {expanded}: {e}"))
}

/// The artefact everything downstream reads: which account carries which feed.
fn write_map(
    path: &PathBuf,
    feeds: &[(String, String)],
    accounts: &HashMap<String, Keypair>,
) -> Result<()> {
    let entries: Vec<serde_json::Value> = feeds
        .iter()
        .filter_map(|(symbol, feed_id)| {
            accounts.get(symbol).map(|kp| {
                serde_json::json!({
                    "symbol": symbol,
                    "feed_id": feed_id,
                    "price_account": kp.pubkey().to_string(),
                })
            })
        })
        .collect();
    std::fs::write(path, serde_json::to_string_pretty(&entries)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
// Same reasoning as `programs/solfx-core/tests/compute_budget.rs`: the workspace denies
// `unwrap` and panicking indexes because in a service a panic is an outage, but in a test a
// panic *is* the reporting mechanism. Routing every assertion through a `Result` would make
// the suite less readable without making it safer.
#[allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    /// The one constant here that cannot be checked by reading it. If the receiver ever
    /// renames the instruction, or this was mistyped, the failure would otherwise surface as
    /// an opaque `InstructionFallbackNotFound` from a devnet transaction.
    #[test]
    fn discriminators_match_anchors_derivation() {
        use sha2::{Digest, Sha256};
        let expect = |name: &str| {
            let mut h = Sha256::new();
            h.update(format!("global:{name}").as_bytes());
            let out = h.finalize();
            let mut d = [0u8; 8];
            d.copy_from_slice(&out[..8]);
            d
        };
        assert_eq!(IX_INIT_ENCODED_VAA, expect("init_encoded_vaa"));
        assert_eq!(IX_WRITE_ENCODED_VAA, expect("write_encoded_vaa"));
        assert_eq!(IX_VERIFY_ENCODED_VAA, expect("verify_encoded_vaa_v1"));
        assert_eq!(IX_CLOSE_ENCODED_VAA, expect("close_encoded_vaa"));
        assert_eq!(IX_POST_UPDATE, expect("post_update"));
    }

    /// The guardian set index is read from the VAA, so a rotation needs no code change.
    #[test]
    fn the_guardian_set_follows_the_vaa() {
        let wormhole = Pubkey::new_unique();
        let vaa_a = [1u8, 0, 0, 0, 4, 0];
        let vaa_b = [1u8, 0, 0, 0, 5, 0];
        let a = guardian_set_address(&wormhole, &vaa_a).unwrap();
        let b = guardian_set_address(&wormhole, &vaa_b).unwrap();
        assert_ne!(
            a, b,
            "different guardian sets must resolve to different accounts"
        );
        assert_eq!(
            a,
            guardian_set_address(&wormhole, &vaa_a).unwrap(),
            "derivation is stable"
        );
    }
}
