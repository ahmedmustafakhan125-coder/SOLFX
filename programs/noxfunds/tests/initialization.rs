//! Who may initialise NOXFUNDS — and every way of pretending to be them.
//!
//! `initialize_config` creates a singleton. Whoever runs it first becomes `admin` for good and
//! chooses `treasury`, the account every mandate's 5% performance fee is paid into. Before
//! this guard existed, any wallet could do that to a freshly deployed program. Measured on
//! devnet on 2026-09-17: NOXFUNDS was deployed at slot 499,915,146 and sat uninitialised with
//! that instruction open to anyone.
//!
//! The permit case is covered implicitly by every other test file, since each fixture
//! initialises through this path. What is here is the block cases — the attacks — plus the
//! one permit test that checks what initialisation actually wrote.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

#[path = "../../solfx-core/tests/common/mod.rs"]
mod common;
#[allow(dead_code)]
mod support;

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use common::*;
use solana_keypair::Keypair;
use solana_signer::Signer;

fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[noxfunds::constants::CONFIG_SEED], &noxfunds::ID).0
}

/// A cluster with SolFX initialised and NOXFUNDS freshly deployed, **not yet initialised**.
/// `authority` is who holds the NOXFUNDS upgrade authority.
fn fresh_deployment(authority: Option<Pubkey>) -> Env {
    let mut env = Env::new();
    let so = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/noxfunds.so"),
    )
    .expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
    support::set_upgrade_authority(&mut env, authority);
    env.init_protocol();
    env
}

fn funded(env: &mut Env) -> Keypair {
    let k = Keypair::new();
    env.svm.airdrop(&k.pubkey(), 10 * 1_000_000_000).unwrap();
    k
}

/// `initialize_config` as a caller would build it. `program` and `program_data` are
/// parameters because substituting them is exactly how the guard would be attacked.
fn initialize_config_ix(
    env: &Env,
    signer: &Pubkey,
    program: Pubkey,
    program_data: Pubkey,
    treasury: Pubkey,
) -> Instruction {
    Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeConfig {
            admin: *signer,
            config: config_pda(),
            program,
            program_data,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeConfig {
            guardian: *signer,
            treasury,
            usdc_mint: env.usdc_mint,
        }
        .data(),
    }
}

fn config_exists(env: &Env) -> bool {
    env.svm
        .get_account(&config_pda())
        .is_some_and(|a| !a.data.is_empty())
}

// --- the permit case -------------------------------------------------------------------------

/// The upgrade authority initialises, and the configuration records exactly what was asked.
#[test]
fn the_upgrade_authority_initialises_and_becomes_admin() {
    let owner = Keypair::new();
    let mut env = fresh_deployment(Some(owner.pubkey()));
    env.svm
        .airdrop(&owner.pubkey(), 10 * 1_000_000_000)
        .unwrap();
    let treasury = Pubkey::new_unique();

    let ix = initialize_config_ix(
        &env,
        &owner.pubkey(),
        noxfunds::ID,
        support::program_data_address(),
        treasury,
    );
    env.send(ix, &[&owner]).expect("the upgrade authority may");

    let cfg: noxfunds::state::NoxConfig = env.read(&config_pda());
    assert_eq!(cfg.admin, owner.pubkey());
    assert_eq!(cfg.guardian, owner.pubkey());
    assert_eq!(cfg.treasury, treasury);
    assert_eq!(cfg.usdc_mint, env.usdc_mint);
    assert_eq!(cfg.solfx_program, solfx_core::ID);
    assert_eq!(cfg.protocol_fee_bps, 500);
    assert!(!cfg.paused);
    env.assert_invariants();
}

// --- the attacks -----------------------------------------------------------------------------

/// **The attack this guard exists for.** A stranger watching for deployments tries to
/// initialise first and point the treasury at themselves.
#[test]
fn a_stranger_cannot_initialise_a_fresh_deployment() {
    let owner = Keypair::new();
    let mut env = fresh_deployment(Some(owner.pubkey()));
    let stranger = funded(&mut env);

    let ix = initialize_config_ix(
        &env,
        &stranger.pubkey(),
        noxfunds::ID,
        support::program_data_address(),
        stranger.pubkey(),
    );
    let err = env
        .send(ix, &[&stranger])
        .expect_err("only the upgrade authority may initialise");
    assert!(
        format!("{err:?}").contains("NotTheUpgradeAuthority"),
        "{err:?}"
    );
    assert!(
        !config_exists(&env),
        "a refused attempt must leave nothing behind for the owner to fight over"
    );
}

/// The subtler attack. The stranger holds the upgrade authority of **some other** upgradeable
/// program, and passes *that* program's data account. Its authority genuinely is the signer,
/// so the second constraint alone would pass — which is why the first constraint, pinning
/// `program_data` to this program, is not optional.
#[test]
fn another_programs_data_account_does_not_satisfy_the_check() {
    let owner = Keypair::new();
    let mut env = fresh_deployment(Some(owner.pubkey()));
    let stranger = funded(&mut env);

    // `solfx-core` stands in for "a program the attacker controls": hand its upgrade
    // authority to the stranger, then offer its data account as NOXFUNDS'.
    support::set_upgrade_authority_of(&mut env, &solfx_core::ID, Some(stranger.pubkey()));
    let foreign_data = support::program_data_address_of(&solfx_core::ID);

    let ix = initialize_config_ix(
        &env,
        &stranger.pubkey(),
        noxfunds::ID,
        foreign_data,
        stranger.pubkey(),
    );
    let err = env
        .send(ix, &[&stranger])
        .expect_err("a data account that does not belong to this program proves nothing");
    assert!(
        format!("{err:?}").contains("NotTheUpgradeAuthority"),
        "{err:?}"
    );
    assert!(!config_exists(&env));
}

/// The same substitution one account earlier: pass the attacker's program *and* its matching
/// data account, so the two agree with each other. `Program<Noxfunds>` checks the address, so
/// the pair is refused before either constraint is consulted.
#[test]
fn a_consistent_pair_from_another_program_is_refused_too() {
    let owner = Keypair::new();
    let mut env = fresh_deployment(Some(owner.pubkey()));
    let stranger = funded(&mut env);
    support::set_upgrade_authority_of(&mut env, &solfx_core::ID, Some(stranger.pubkey()));

    let ix = initialize_config_ix(
        &env,
        &stranger.pubkey(),
        solfx_core::ID,
        support::program_data_address_of(&solfx_core::ID),
        stranger.pubkey(),
    );
    let err = env
        .send(ix, &[&stranger])
        .expect_err("the program account must be NOXFUNDS itself");
    assert!(format!("{err:?}").contains("InvalidProgramId"), "{err:?}");
    assert!(!config_exists(&env));
}

/// A program made immutable has no upgrade authority, so nobody — including whoever deployed
/// it — can initialise it. That is the right failure, and it is the operational rule this
/// test pins: **initialise before finalising**, never after.
#[test]
fn an_immutable_program_cannot_be_initialised_by_anyone() {
    let mut env = fresh_deployment(None);
    let deployer = funded(&mut env);

    let ix = initialize_config_ix(
        &env,
        &deployer.pubkey(),
        noxfunds::ID,
        support::program_data_address(),
        deployer.pubkey(),
    );
    let err = env
        .send(ix, &[&deployer])
        .expect_err("no upgrade authority means no initialiser");
    assert!(
        format!("{err:?}").contains("NotTheUpgradeAuthority"),
        "{err:?}"
    );
    assert!(!config_exists(&env));
}

/// Initialisation happens once. Even the upgrade authority cannot run it a second time to
/// quietly move the treasury — a change to the configuration has to be a visible program
/// upgrade, not a replayed setup call.
#[test]
fn initialisation_cannot_be_repeated_to_move_the_treasury() {
    let owner = Keypair::new();
    let mut env = fresh_deployment(Some(owner.pubkey()));
    env.svm
        .airdrop(&owner.pubkey(), 10 * 1_000_000_000)
        .unwrap();
    let original = Pubkey::new_unique();

    let ix = initialize_config_ix(
        &env,
        &owner.pubkey(),
        noxfunds::ID,
        support::program_data_address(),
        original,
    );
    env.send(ix, &[&owner]).expect("first initialisation");

    let ix = initialize_config_ix(
        &env,
        &owner.pubkey(),
        noxfunds::ID,
        support::program_data_address(),
        Pubkey::new_unique(),
    );
    env.send(ix, &[&owner])
        .expect_err("the config already exists");

    let cfg: noxfunds::state::NoxConfig = env.read(&config_pda());
    assert_eq!(cfg.treasury, original, "the treasury did not move");
}
