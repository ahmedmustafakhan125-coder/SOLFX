//! Who controls SolFX, the referral programme and NOXFUNDS — checked, and claimed.
//!
//! # Why this exists
//!
//! Each of the three programs has a one-time setup instruction that creates a singleton, and
//! whoever creates it first owns it for good: the SolFX `Protocol`, the referral programme's
//! `ReferralConfig`, and the NOXFUNDS `NoxConfig`. The operating rule is that **every one of
//! them belongs to the deployer**, and a rule nobody checks is a rule that is only believed.
//!
//! Measured on devnet on 2026-09-17, before this tool existed:
//!
//! | Singleton | State |
//! |---|---|
//! | SolFX `Protocol` | initialised, admin = the deployer |
//! | `ReferralConfig` | **not initialised** — and `initialize_referral` takes any signer |
//! | `NoxConfig` | **not initialised** — NOXFUNDS was live at slot 499,915,146 |
//!
//! The referral check was first run against the wrong address, derived from a retyped
//! `"config"` seed rather than the programme's own `"referral_config"`. Every address here is
//! derived from the programs' exported constants, and the tests pin each one to the address
//! measured on devnet, so that cannot happen twice.
//!
//! # What it does
//!
//! ```text
//! authority check                      read-only; exits non-zero if anything is not yours
//! authority init-noxfunds              simulates against the cluster, sends nothing
//! authority init-noxfunds --execute    sends, then re-reads and verifies what landed
//! authority init-referral --execute
//! ```
//!
//! Every initialisation is simulated before it is sent, and the default is to stop there.
//! These instructions cannot be undone — neither program has an instruction to change the
//! treasury or the guardian afterwards — so the cheapest place to catch a wrong value is a
//! human reading the plan, not a transaction.
//!
//! # What deliberately does *not* depend on the deployer
//!
//! Liquidations, stop execution, and the NOXFUNDS equity crank, wind-down and settlement are
//! permissionless by design and stay that way. A venue that needs its operator present to
//! liquidate, or an investor who needs the operator to release their capital, fails exactly
//! when the operator is absent. Ownership is for configuration; operation is for anyone.

use anchor_lang::prelude::{AccountDeserialize as _, ProgramData, UpgradeableLoaderState};
use anchor_lang::solana_program::bpf_loader_upgradeable;
use anchor_lang::{InstructionData as _, ToAccountMetas as _};
use anyhow::{anyhow, bail, Context as _, Result};
use clap::Parser as _;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_rpc_client_api::config::RpcSimulateTransactionConfig;
use solana_signer::Signer as _;
use solana_transaction::Transaction;

use noxfunds::state::NoxConfig;
use solfx_core::constants::PROTOCOL_SEED;
use solfx_core::state::Protocol;
use solfx_referral::ReferralConfig;

/// Enough for either initialisation with room to spare. `initialize_config` creates one account
/// and reads two; the simulation prints what it actually used.
const COMPUTE_UNIT_LIMIT: u32 = 60_000;

#[derive(clap::Parser)]
#[command(
    name = "authority",
    about = "Check and claim ownership of every SolFX and NOXFUNDS singleton"
)]
struct Args {
    #[arg(long, env = "SOLFX_RPC_URL", default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    /// The deployer. Must hold the upgrade authority of whichever program is being initialised.
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Read-only. Reports who holds every upgrade authority and every singleton, and exits
    /// non-zero if any of them is uninitialised or held by someone other than `--owner`.
    Check {
        /// The key everything should belong to. Defaults to `--keypair`'s public key.
        #[arg(long)]
        owner: Option<Pubkey>,
    },
    /// Initialise the NOXFUNDS configuration. Simulates and stops unless `--execute` is given.
    InitNoxfunds {
        /// Simulate as this address without reading any keypair. A simulation needs no
        /// signature, so the plan can be reviewed on a machine that does not hold the key.
        /// With `--execute` it must match `--keypair`, which is what signs.
        #[arg(long)]
        admin: Option<Pubkey>,
        /// Where the 5% performance fee is paid. Defaults to the deployer. Permanent.
        #[arg(long)]
        treasury: Option<Pubkey>,
        /// May pause NOXFUNDS but never unpause it. Defaults to the deployer. Permanent.
        ///
        /// Deliberately *not* copied from SolFX: on devnet SolFX's guardian is the all-zero
        /// key, and copying it would silently leave NOXFUNDS with no guardian at all.
        #[arg(long)]
        guardian: Option<Pubkey>,
        #[arg(long)]
        execute: bool,
    },
    /// Initialise the referral programme's configuration. Simulates and stops unless
    /// `--execute` is given.
    InitReferral {
        /// Simulate as this address without reading any keypair. See `init-noxfunds --admin`.
        #[arg(long)]
        admin: Option<Pubkey>,
        /// The IB override, in bps. Defaults to the programme's own `DEFAULT_OVERRIDE_BPS`.
        /// Permanent.
        #[arg(long, default_value_t = solfx_referral::DEFAULT_OVERRIDE_BPS)]
        override_bps: u16,
        #[arg(long)]
        execute: bool,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

async fn run(args: Args) -> Result<()> {
    let rpc = RpcClient::new_with_commitment(args.rpc_url.clone(), CommitmentConfig::confirmed());
    match args.cmd {
        // A read-only audit must not need a secret. The keypair is read only to learn the
        // owner's address, and only when `--owner` does not already say it.
        Cmd::Check { owner } => {
            let owner = match owner {
                Some(o) => o,
                None => load_keypair(&args.keypair)?.pubkey(),
            };
            check(&rpc, owner).await
        }
        Cmd::InitNoxfunds {
            admin,
            treasury,
            guardian,
            execute,
        } => {
            let who = Who::resolve(admin, execute, &args.keypair)?;
            init_noxfunds(&rpc, &who, treasury, guardian).await
        }
        Cmd::InitReferral {
            admin,
            override_bps,
            execute,
        } => {
            let who = Who::resolve(admin, execute, &args.keypair)?;
            init_referral(&rpc, &who, override_bps).await
        }
    }
}

/// Who an initialisation runs as, and whether it may be sent.
///
/// Sending needs the secret key; simulating needs only the address. Keeping the two apart means
/// the plan can be checked on the server while the key never leaves the deployer's machine.
enum Who {
    Simulate(Pubkey),
    Execute(Keypair),
}

impl Who {
    fn resolve(admin: Option<Pubkey>, execute: bool, keypair: &str) -> Result<Self> {
        if execute {
            let kp = load_keypair(keypair)?;
            if let Some(a) = admin {
                if a != kp.pubkey() {
                    bail!(
                        "--admin {a} does not match --keypair {}; --execute signs with the keypair",
                        kp.pubkey()
                    );
                }
            }
            return Ok(Self::Execute(kp));
        }
        Ok(Self::Simulate(match admin {
            Some(a) => a,
            None => load_keypair(keypair)?.pubkey(),
        }))
    }

    fn pubkey(&self) -> Pubkey {
        match self {
            Self::Simulate(k) => *k,
            Self::Execute(kp) => kp.pubkey(),
        }
    }
}

// --- addresses ------------------------------------------------------------------------------
//
// Every seed comes from the program that owns it. Pinned to devnet in the tests.

fn find(seeds: &[&[u8]], program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(seeds, program).0
}

fn protocol_address() -> Pubkey {
    find(&[PROTOCOL_SEED], &solfx_core::ID)
}

fn referral_config_address() -> Pubkey {
    find(&[solfx_referral::CONFIG_SEED], &solfx_referral::ID)
}

fn nox_config_address() -> Pubkey {
    find(&[noxfunds::constants::CONFIG_SEED], &noxfunds::ID)
}

/// Where the upgradeable loader keeps a program's bytecode and its upgrade authority.
fn program_data_address(program: &Pubkey) -> Pubkey {
    find(&[program.as_ref()], &bpf_loader_upgradeable::ID)
}

// --- reading the chain ----------------------------------------------------------------------

/// An account as far as this tool cares: who owns it, and its bytes.
struct Fetched {
    owner: Pubkey,
    data: Vec<u8>,
}

async fn fetch(rpc: &RpcClient, key: &Pubkey) -> Result<Option<Fetched>> {
    let value = rpc
        .get_account_with_commitment(key, rpc.commitment())
        .await
        .with_context(|| format!("reading {key}"))?
        .value;
    Ok(value.map(|a| Fetched {
        owner: a.owner,
        data: a.data,
    }))
}

/// Decode an Anchor account, checking its owner as well as its discriminator.
///
/// The discriminator alone is not enough: another program could hold an account with the same
/// eight leading bytes. `Account<T>` checks the owner on chain, so this does too off chain.
fn decode<T: anchor_lang::AccountDeserialize + anchor_lang::Owner>(
    fetched: &Fetched,
    what: &str,
) -> Result<T> {
    if fetched.owner != T::owner() {
        bail!(
            "{what} is owned by {}, not by {}",
            fetched.owner,
            T::owner()
        );
    }
    T::try_deserialize(&mut fetched.data.as_slice()).map_err(|e| anyhow!("decoding {what}: {e}"))
}

/// Who may upgrade a program — and so, for NOXFUNDS, who may initialise it.
#[derive(Debug, PartialEq, Eq)]
enum Upgrade {
    NotDeployed,
    /// Deployed with a loader that has no upgrade authority at all.
    NotUpgradeable,
    /// Upgradeable loader, authority revoked.
    Immutable,
    Authority(Pubkey),
}

async fn upgrade_authority(rpc: &RpcClient, program: &Pubkey) -> Result<Upgrade> {
    let Some(account) = fetch(rpc, program).await? else {
        return Ok(Upgrade::NotDeployed);
    };
    if account.owner != bpf_loader_upgradeable::ID {
        return Ok(Upgrade::NotUpgradeable);
    }
    // Read the pointer the program account actually holds rather than trusting the derivation,
    // mirroring the `programdata_address()` constraint `initialize_config` applies on chain.
    let pointed = match UpgradeableLoaderState::try_deserialize(&mut account.data.as_slice()) {
        Ok(UpgradeableLoaderState::Program {
            programdata_address,
        }) => programdata_address,
        _ => bail!("{program} is owned by the upgradeable loader but is not a program account"),
    };
    let derived = program_data_address(program);
    if pointed != derived {
        bail!("{program} points at program data {pointed}, expected {derived}");
    }
    let data = fetch(rpc, &derived)
        .await?
        .ok_or_else(|| anyhow!("program data {derived} for {program} does not exist"))?;
    let decoded = ProgramData::try_deserialize(&mut data.data.as_slice())
        .map_err(|e| anyhow!("decoding program data for {program}: {e}"))?;
    Ok(match decoded.upgrade_authority_address {
        Some(k) => Upgrade::Authority(k),
        None => Upgrade::Immutable,
    })
}

// --- check ----------------------------------------------------------------------------------

/// One line of the ownership report.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Status {
    /// Held by the owner.
    Yours,
    /// A singleton nobody has created yet. For the referral programme, anyone can.
    Open,
    /// Held by a different key, or in a state the owner does not control.
    NotYours,
    /// Worth reading, not a failure.
    Note,
}

impl Status {
    const fn label(self) -> &'static str {
        match self {
            Self::Yours => "yours",
            Self::Open => "OPEN",
            Self::NotYours => "NOT YOURS",
            Self::Note => "note",
        }
    }

    const fn is_failure(self) -> bool {
        matches!(self, Self::Open | Self::NotYours)
    }
}

/// Classify a single recorded authority against the expected owner.
fn classify(recorded: Option<Pubkey>, owner: &Pubkey) -> Status {
    match recorded {
        None => Status::Open,
        Some(k) if k == *owner => Status::Yours,
        Some(_) => Status::NotYours,
    }
}

struct Row {
    subject: &'static str,
    status: Status,
    detail: String,
}

async fn check(rpc: &RpcClient, owner: Pubkey) -> Result<()> {
    let mut rows = Vec::new();

    for (subject, program) in [
        ("solfx-core upgrade authority", solfx_core::ID),
        ("solfx-referral upgrade authority", solfx_referral::ID),
        ("noxfunds upgrade authority", noxfunds::ID),
    ] {
        let (status, detail) = match upgrade_authority(rpc, &program).await? {
            Upgrade::Authority(k) => (classify(Some(k), &owner), k.to_string()),
            Upgrade::Immutable => (
                Status::NotYours,
                "immutable — no one can upgrade or initialise it".into(),
            ),
            Upgrade::NotUpgradeable => (Status::NotYours, "not an upgradeable program".into()),
            Upgrade::NotDeployed => (Status::Note, format!("{program} is not deployed here")),
        };
        rows.push(Row {
            subject,
            status,
            detail,
        });
    }

    let protocol = match fetch(rpc, &protocol_address()).await? {
        Some(f) => Some(decode::<Protocol>(&f, "SolFX protocol")?),
        None => None,
    };
    match &protocol {
        None => rows.push(Row {
            subject: "SolFX protocol",
            status: Status::Open,
            detail: "not initialised — `initialize_protocol` takes any signer".into(),
        }),
        Some(p) => {
            rows.push(Row {
                subject: "SolFX protocol admin",
                status: classify(Some(p.admin), &owner),
                detail: p.admin.to_string(),
            });
            if p.pending_admin != Pubkey::default() {
                rows.push(Row {
                    subject: "SolFX pending admin",
                    status: classify(Some(p.pending_admin), &owner),
                    detail: format!("{} can accept the admin role", p.pending_admin),
                });
            }
            rows.push(Row {
                subject: "SolFX guardian",
                status: Status::Note,
                detail: if p.guardian == Pubkey::default() {
                    "unset — only the admin can pause".into()
                } else {
                    p.guardian.to_string()
                },
            });
            rows.push(Row {
                subject: "SolFX referral authority",
                status: Status::Note,
                detail: if p.referral_authority == Pubkey::default() {
                    "unset — referral payouts are disabled".into()
                } else if p.referral_authority == referral_config_address() {
                    "the referral programme's config".into()
                } else {
                    format!("{} (not the referral config)", p.referral_authority)
                },
            });
        }
    }

    match fetch(rpc, &referral_config_address()).await? {
        None => rows.push(Row {
            subject: "referral config",
            status: Status::Open,
            detail: "not initialised — `initialize_referral` takes any signer; run init-referral"
                .into(),
        }),
        Some(f) => {
            let cfg = decode::<ReferralConfig>(&f, "referral config")?;
            rows.push(Row {
                subject: "referral config admin",
                status: classify(Some(cfg.admin), &owner),
                detail: format!("{}, override {} bps", cfg.admin, cfg.override_bps),
            });
        }
    }

    match fetch(rpc, &nox_config_address()).await? {
        None => rows.push(Row {
            subject: "NOXFUNDS config",
            status: Status::Open,
            detail: "not initialised — run init-noxfunds".into(),
        }),
        Some(f) => {
            let cfg = decode::<NoxConfig>(&f, "NOXFUNDS config")?;
            rows.push(Row {
                subject: "NOXFUNDS config admin",
                status: classify(Some(cfg.admin), &owner),
                detail: cfg.admin.to_string(),
            });
            rows.push(Row {
                subject: "NOXFUNDS treasury",
                status: classify(Some(cfg.treasury), &owner),
                detail: cfg.treasury.to_string(),
            });
            if let Some(p) = &protocol {
                if cfg.usdc_mint != p.usdc_mint {
                    rows.push(Row {
                        subject: "NOXFUNDS collateral mint",
                        status: Status::NotYours,
                        detail: format!(
                            "{} differs from SolFX's {} — settlement cannot work",
                            cfg.usdc_mint, p.usdc_mint
                        ),
                    });
                }
            }
        }
    }

    println!("\n  owner {owner}\n");
    for r in &rows {
        println!("  {:<10} {:<34} {}", r.status.label(), r.subject, r.detail);
    }
    let failures = rows.iter().filter(|r| r.status.is_failure()).count();
    println!();
    if failures > 0 {
        bail!("{failures} item(s) are open or not held by {owner}");
    }
    println!("  everything is held by {owner}\n");
    Ok(())
}

// --- init-noxfunds --------------------------------------------------------------------------

/// What the NOXFUNDS configuration should hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NoxWanted {
    admin: Pubkey,
    guardian: Pubkey,
    treasury: Pubkey,
    usdc_mint: Pubkey,
}

fn initialize_config_ix(w: &NoxWanted) -> Instruction {
    Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeConfig {
            admin: w.admin,
            config: nox_config_address(),
            program: noxfunds::ID,
            program_data: program_data_address(&noxfunds::ID),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeConfig {
            guardian: w.guardian,
            treasury: w.treasury,
            usdc_mint: w.usdc_mint,
        }
        .data(),
    }
}

/// Every field of an existing configuration that differs from what was asked for.
fn nox_differences(cfg: &NoxConfig, w: &NoxWanted) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = |name: &str, have: Pubkey, want: Pubkey| {
        if have != want {
            out.push(format!("{name}: on chain {have}, wanted {want}"));
        }
    };
    field("admin", cfg.admin, w.admin);
    field("guardian", cfg.guardian, w.guardian);
    field("treasury", cfg.treasury, w.treasury);
    field("usdc_mint", cfg.usdc_mint, w.usdc_mint);
    field("solfx_program", cfg.solfx_program, solfx_core::ID);
    out
}

async fn init_noxfunds(
    rpc: &RpcClient,
    who: &Who,
    treasury: Option<Pubkey>,
    guardian: Option<Pubkey>,
) -> Result<()> {
    let me = who.pubkey();
    require_upgrade_authority(rpc, &noxfunds::ID, "noxfunds", &me).await?;

    // The collateral mint is not a choice. Settlement withdraws from SolFX's collateral vault
    // into the mandate vault, so the two must be the same mint — and SolFX's own
    // `initialize_protocol` already refused anything but 6 decimals when it was created.
    let protocol = decode::<Protocol>(
        &fetch(rpc, &protocol_address())
            .await?
            .ok_or_else(|| anyhow!("SolFX is not initialised on this cluster"))?,
        "SolFX protocol",
    )?;
    let wanted = NoxWanted {
        admin: me,
        guardian: guardian.unwrap_or(me),
        treasury: treasury.unwrap_or(me),
        usdc_mint: protocol.usdc_mint,
    };

    if let Some(existing) = fetch(rpc, &nox_config_address()).await? {
        let cfg = decode::<NoxConfig>(&existing, "NOXFUNDS config")?;
        return report_existing("NOXFUNDS config", nox_differences(&cfg, &wanted));
    }

    println!("\n  NOXFUNDS {}", noxfunds::ID);
    println!("  config        {}", nox_config_address());
    println!("  admin         {}", wanted.admin);
    println!("  guardian      {}", wanted.guardian);
    println!("  treasury      {}", wanted.treasury);
    println!("  usdc mint     {}  (SolFX's)", wanted.usdc_mint);
    println!("\n  treasury and guardian are permanent: NOXFUNDS has no instruction to change");
    println!("  either, so moving one later means a program upgrade.\n");

    let ix = initialize_config_ix(&wanted);
    simulate(rpc, &me, ix.clone(), "initialize_config").await?;
    let Who::Execute(signer) = who else {
        println!("  simulated only — re-run with --execute to send\n");
        return Ok(());
    };

    let sig = send(rpc, signer, ix).await?;
    println!("  sent {sig}");
    let landed = decode::<NoxConfig>(
        &fetch(rpc, &nox_config_address())
            .await?
            .ok_or_else(|| anyhow!("the transaction confirmed but the config does not exist"))?,
        "NOXFUNDS config",
    )?;
    report_landed("NOXFUNDS config", nox_differences(&landed, &wanted))
}

// --- init-referral --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReferralWanted {
    admin: Pubkey,
    override_bps: u16,
}

fn initialize_referral_ix(w: &ReferralWanted) -> Instruction {
    Instruction {
        program_id: solfx_referral::ID,
        accounts: solfx_referral::accounts::InitializeReferral {
            admin: w.admin,
            config: referral_config_address(),
            protocol: protocol_address(),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: solfx_referral::instruction::InitializeReferral {
            override_bps: w.override_bps,
        }
        .data(),
    }
}

fn referral_differences(cfg: &ReferralConfig, w: &ReferralWanted) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.admin != w.admin {
        out.push(format!("admin: on chain {}, wanted {}", cfg.admin, w.admin));
    }
    if cfg.protocol != protocol_address() {
        out.push(format!(
            "protocol: on chain {}, wanted {}",
            cfg.protocol,
            protocol_address()
        ));
    }
    if cfg.override_bps != w.override_bps {
        out.push(format!(
            "override_bps: on chain {}, wanted {}",
            cfg.override_bps, w.override_bps
        ));
    }
    out
}

async fn init_referral(rpc: &RpcClient, who: &Who, override_bps: u16) -> Result<()> {
    let me = who.pubkey();
    if override_bps > 10_000 {
        bail!("--override-bps {override_bps} is above 10,000 bps");
    }

    // The programme itself accepts any signer here. The tool does not: only the key that owns
    // both the referral programme and SolFX may create the configuration that SolFX will later
    // be asked to trust.
    require_upgrade_authority(rpc, &solfx_referral::ID, "solfx-referral", &me).await?;
    let protocol = decode::<Protocol>(
        &fetch(rpc, &protocol_address())
            .await?
            .ok_or_else(|| anyhow!("SolFX is not initialised on this cluster"))?,
        "SolFX protocol",
    )?;
    if protocol.admin != me {
        bail!(
            "SolFX's admin is {}, not {me} — the referral config must be created by the same key",
            protocol.admin
        );
    }

    let wanted = ReferralWanted {
        admin: me,
        override_bps,
    };
    if let Some(existing) = fetch(rpc, &referral_config_address()).await? {
        let cfg = decode::<ReferralConfig>(&existing, "referral config")?;
        return report_existing("referral config", referral_differences(&cfg, &wanted));
    }

    println!("\n  solfx-referral {}", solfx_referral::ID);
    println!("  config         {}", referral_config_address());
    println!("  admin          {me}");
    println!("  override       {override_bps} bps  (permanent)");
    println!("  protocol       {}\n", protocol_address());

    let ix = initialize_referral_ix(&wanted);
    simulate(rpc, &me, ix.clone(), "initialize_referral").await?;
    let Who::Execute(signer) = who else {
        println!("  simulated only — re-run with --execute to send\n");
        return Ok(());
    };

    let sig = send(rpc, signer, ix).await?;
    println!("  sent {sig}");
    let landed = decode::<ReferralConfig>(
        &fetch(rpc, &referral_config_address())
            .await?
            .ok_or_else(|| anyhow!("the transaction confirmed but the config does not exist"))?,
        "referral config",
    )?;
    report_landed("referral config", referral_differences(&landed, &wanted))?;
    println!(
        "  referral payouts stay disabled until the SolFX admin registers {} with \
         `set_referral_authority`\n",
        referral_config_address()
    );
    Ok(())
}

// --- shared steps ---------------------------------------------------------------------------

async fn require_upgrade_authority(
    rpc: &RpcClient,
    program: &Pubkey,
    name: &str,
    me: &Pubkey,
) -> Result<()> {
    match upgrade_authority(rpc, program).await? {
        Upgrade::Authority(k) if k == *me => Ok(()),
        Upgrade::Authority(k) => bail!(
            "{name}'s upgrade authority is {k}, not {me}. Sign with the deployer's keypair \
             (--keypair)."
        ),
        Upgrade::Immutable => bail!(
            "{name} is immutable. It has no upgrade authority, so it can never be initialised."
        ),
        Upgrade::NotUpgradeable => bail!("{name} is not deployed with the upgradeable loader"),
        Upgrade::NotDeployed => bail!("{name} ({program}) is not deployed on this cluster"),
    }
}

/// A singleton that already exists: fine if it matches, an error naming every difference if not.
/// Neither program can rewrite its configuration, so a mismatch is reported, never "fixed".
fn report_existing(what: &str, differences: Vec<String>) -> Result<()> {
    if differences.is_empty() {
        println!("\n  {what} is already initialised and matches — nothing to do\n");
        return Ok(());
    }
    for d in &differences {
        println!("  {d}");
    }
    bail!(
        "{what} already exists and differs in {} field(s)",
        differences.len()
    )
}

fn report_landed(what: &str, differences: Vec<String>) -> Result<()> {
    if differences.is_empty() {
        println!("  {what} verified on chain\n");
        return Ok(());
    }
    for d in &differences {
        println!("  {d}");
    }
    bail!("{what} landed but differs from what was sent")
}

/// Run the transaction against the cluster without signing or sending it, and refuse to go
/// further if it would fail.
///
/// This is also how the logs reach the terminal: `send_and_confirm_transaction` reports a
/// failure without them, and a setup failure with no logs is an hour lost.
async fn simulate(rpc: &RpcClient, payer: &Pubkey, ix: Instruction, name: &str) -> Result<()> {
    let message = Message::new(
        &[
            ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
            ix,
        ],
        Some(payer),
    );
    let tx = Transaction::new_unsigned(message);
    let result = rpc
        .simulate_transaction_with_config(
            &tx,
            RpcSimulateTransactionConfig {
                sig_verify: false,
                replace_recent_blockhash: true,
                commitment: Some(rpc.commitment()),
                ..RpcSimulateTransactionConfig::default()
            },
        )
        .await
        .with_context(|| format!("simulating {name}"))?
        .value;

    let logs = result.logs.unwrap_or_default();
    if let Some(err) = result.err {
        for line in &logs {
            println!("    {line}");
        }
        if logs.iter().any(|l| l.contains("InvalidProgramId")) && name == "initialize_config" {
            bail!(
                "{name} would fail: the deployed NOXFUNDS predates the initialisation guard. \
                 Upgrade it first, then run this again."
            );
        }
        bail!("{name} would fail: {err}");
    }
    println!(
        "  simulation: {name} succeeds, {} CU",
        result
            .units_consumed
            .map_or_else(|| "?".to_string(), |u| u.to_string())
    );
    Ok(())
}

async fn send(rpc: &RpcClient, payer: &Keypair, ix: Instruction) -> Result<String> {
    let ixs = [
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        ix,
    ];
    let blockhash = rpc.get_latest_blockhash().await.context("blockhash")?;
    let msg = Message::new(&ixs, Some(&payer.pubkey()));
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
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use anchor_lang::AnchorDeserialize as _;
    use sha2::{Digest as _, Sha256};

    fn key(s: &str) -> Pubkey {
        s.parse().unwrap()
    }

    /// Every address this tool derives, against what devnet actually holds — measured with
    /// `solana program show` and `solana find-program-derived-address` on 2026-09-17.
    ///
    /// The referral config is the one that matters most: it was first checked at
    /// `Cg8oeL5S…`, derived from a retyped `"config"` seed, which is not the programme's seed.
    #[test]
    fn every_derived_address_matches_devnet() {
        assert_eq!(
            protocol_address(),
            key("GbgsnqqqRws5Ch33eHuoAWqKghQiVWt8wBSKzwNffjwc")
        );
        assert_eq!(
            referral_config_address(),
            key("9s7xiSziQnWxozwFfSz7uyRfLKyvh79bsm6xVTYUodkL")
        );
        assert_ne!(
            referral_config_address(),
            key("Cg8oeL5SqbxYPWh8TU9AWSxVRrS5cCvTEebJErhxzZa4"),
            "that is the address the wrong seed produces"
        );
        assert_eq!(
            nox_config_address(),
            key("8xT4KH1xnpqKW7ic1Pypu7vUJU5HRGnyJZ6z5kWtkzQ4")
        );
        assert_eq!(
            program_data_address(&noxfunds::ID),
            key("F56gLx1QypJgMuYRkyM8GVxTNE72m2Sk9vPAMAGdiULC")
        );
        assert_eq!(
            program_data_address(&solfx_core::ID),
            key("G4dD8wCTd27mxxaEDe8om7Aw8pwhMgAeT86QzCCa2geb")
        );
        assert_eq!(
            program_data_address(&solfx_referral::ID),
            key("Esgjfu8qfRprsbrmKPKYR4sVXycT5YTvGsUEGmgkvoNC")
        );
    }

    fn discriminator(name: &str) -> [u8; 8] {
        let hash = Sha256::digest(format!("global:{name}").as_bytes());
        let mut out = [0u8; 8];
        out.copy_from_slice(&hash[..8]);
        out
    }

    fn nox_wanted() -> NoxWanted {
        NoxWanted {
            admin: Pubkey::new_unique(),
            guardian: Pubkey::new_unique(),
            treasury: Pubkey::new_unique(),
            usdc_mint: Pubkey::new_unique(),
        }
    }

    /// The wire contract of `initialize_config`, independent of the program crate that built
    /// it: the discriminator re-derived from the name, the arguments decoded back, and the
    /// five accounts in the order and with the flags the guard needs.
    #[test]
    fn initialize_config_carries_the_guard_accounts() {
        let w = nox_wanted();
        let ix = initialize_config_ix(&w);
        assert_eq!(ix.program_id, noxfunds::ID);
        assert_eq!(ix.data[..8], discriminator("initialize_config"));

        let args =
            noxfunds::instruction::InitializeConfig::deserialize(&mut &ix.data[8..]).unwrap();
        assert_eq!(
            (args.guardian, args.treasury, args.usdc_mint),
            (w.guardian, w.treasury, w.usdc_mint)
        );

        let m = &ix.accounts;
        assert_eq!(m.len(), 5);
        assert_eq!(
            (m[0].pubkey, m[0].is_signer, m[0].is_writable),
            (w.admin, true, true)
        );
        assert_eq!(
            (m[1].pubkey, m[1].is_signer, m[1].is_writable),
            (nox_config_address(), false, true)
        );
        assert_eq!((m[2].pubkey, m[2].is_writable), (noxfunds::ID, false));
        assert_eq!(
            (m[3].pubkey, m[3].is_writable),
            (program_data_address(&noxfunds::ID), false)
        );
        assert_eq!(m[4].pubkey, anchor_lang::system_program::ID);
    }

    #[test]
    fn initialize_referral_carries_the_protocol_and_the_override() {
        let admin = Pubkey::new_unique();
        let ix = initialize_referral_ix(&ReferralWanted {
            admin,
            override_bps: 2_000,
        });
        assert_eq!(ix.program_id, solfx_referral::ID);
        assert_eq!(ix.data[..8], discriminator("initialize_referral"));
        let args = solfx_referral::instruction::InitializeReferral::deserialize(&mut &ix.data[8..])
            .unwrap();
        assert_eq!(args.override_bps, 2_000);

        let m = &ix.accounts;
        assert_eq!(m.len(), 4);
        assert_eq!((m[0].pubkey, m[0].is_signer), (admin, true));
        assert_eq!(m[1].pubkey, referral_config_address());
        assert_eq!(m[2].pubkey, protocol_address());
    }

    fn nox_config_from(w: &NoxWanted) -> NoxConfig {
        NoxConfig {
            admin: w.admin,
            guardian: w.guardian,
            treasury: w.treasury,
            solfx_program: solfx_core::ID,
            usdc_mint: w.usdc_mint,
            protocol_fee_bps: 500,
            paused: false,
            bump: 255,
            _reserved: [0; 64],
        }
    }

    #[test]
    fn a_matching_config_has_no_differences() {
        let w = nox_wanted();
        assert!(nox_differences(&nox_config_from(&w), &w).is_empty());
    }

    /// A config somebody else created must be reported field by field, never mistaken for a
    /// success — the whole reason `check` exists.
    #[test]
    fn a_foreign_config_names_every_field_it_differs_in() {
        let w = nox_wanted();
        let mut cfg = nox_config_from(&w);
        cfg.admin = Pubkey::new_unique();
        cfg.treasury = Pubkey::new_unique();
        let diffs = nox_differences(&cfg, &w);
        assert_eq!(diffs.len(), 2, "{diffs:?}");
        assert!(diffs[0].starts_with("admin:"));
        assert!(diffs[1].starts_with("treasury:"));
    }

    #[test]
    fn a_config_pointing_at_another_venue_is_a_difference() {
        let w = nox_wanted();
        let mut cfg = nox_config_from(&w);
        cfg.solfx_program = Pubkey::new_unique();
        assert!(nox_differences(&cfg, &w)[0].starts_with("solfx_program:"));
    }

    #[test]
    fn referral_differences_cover_admin_protocol_and_override() {
        let w = ReferralWanted {
            admin: Pubkey::new_unique(),
            override_bps: 2_000,
        };
        let mut cfg = ReferralConfig {
            admin: w.admin,
            protocol: protocol_address(),
            override_bps: 2_000,
            pool_split_bps: 0,
            total_accrued: 0,
            total_claimed: 0,
            ib_count: 0,
            bump: 255,
            _reserved: [0; 64],
        };
        assert!(referral_differences(&cfg, &w).is_empty());
        cfg.override_bps = 9_000;
        cfg.protocol = Pubkey::new_unique();
        assert_eq!(referral_differences(&cfg, &w).len(), 2);
    }

    #[test]
    fn ownership_is_classified_three_ways() {
        let owner = Pubkey::new_unique();
        assert_eq!(classify(Some(owner), &owner), Status::Yours);
        assert_eq!(
            classify(Some(Pubkey::new_unique()), &owner),
            Status::NotYours
        );
        assert_eq!(classify(None, &owner), Status::Open);
        assert!(Status::Open.is_failure());
        assert!(Status::NotYours.is_failure());
        assert!(!Status::Yours.is_failure());
        assert!(!Status::Note.is_failure());
    }
}
