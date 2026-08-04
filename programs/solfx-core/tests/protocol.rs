//! Protocol initialisation and the authority model (`ARCHITECTURE.md` ADR-008).
//!
//! The asymmetry under test: the guardian can restrict but never release, and never move
//! funds. A hot key is a key that will eventually leak, so the blast radius of holding one
//! must be capped at "trading stops".

// Test code asserts against known values and unwraps expected-Ok results. The workspace
// denies `unwrap`, `expect` and `panic` because a panic in a *program* is a failed
// transaction with no named error; in a test a panic is the reporting mechanism, and
// routing every assertion through a Result would make the suite unreadable without making
// anything safer.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::{InstructionData, ToAccountMetas};
use common::*;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solfx_core::state::MarketStatus;

#[test]
fn initialize_protocol_writes_every_field() {
    let mut env = Env::new();
    env.init_protocol();

    let p = env.protocol_state();
    assert_eq!(p.admin, env.admin.pubkey());
    assert_eq!(p.guardian, env.guardian.pubkey());
    assert_eq!(p.usdc_mint, env.usdc_mint);
    assert_eq!(p.pending_admin, Pubkey::default());
    assert_eq!(p.num_markets, 0);
    assert!(!p.paused);
    assert_eq!(p.total_user_collateral, 0);

    // § 8.3's launch split. The LP share must be the largest.
    assert_eq!(p.fee_split_lp_bps, 5_500);
    assert_eq!(p.fee_split_treasury_bps, 2_500);
    assert_eq!(p.fee_split_insurance_bps, 1_000);
    assert_eq!(p.fee_split_referral_bps, 1_000);

    // The LP mint and vaults exist from the first instruction, so their addresses never move.
    let lp = env.lp_state();
    assert_eq!(lp.lp_mint, env.lp_mint);
    assert_eq!(lp.lp_vault, env.lp_vault);
    assert_eq!(lp.aum, 0);
    assert_eq!(lp.withdrawal_cooldown_seconds, 86_400);

    let ins = env.insurance_state();
    assert_eq!(ins.balance, 0);
    assert_eq!(ins.target_balance, 100_000 * ONE_USDC);
}

/// USDC has six decimals and `QUOTE_PRECISION` is 1e6. A mint with a different scale would
/// make every quote-denominated figure in the engine wrong by a power of ten, and nothing
/// downstream would notice.
#[test]
fn rejects_a_collateral_mint_with_the_wrong_decimals() {
    let mut env = Env::new();
    let wrong = Pubkey::new_unique();
    env.write_mint_with_decimals(wrong, 9);
    env.usdc_mint = wrong;

    let ix = env.init_protocol_ix(env.default_protocol_params());
    let admin = env.admin.insecure_clone();
    assert_err_contains(
        env.send(ix, &[&admin]),
        "Collateral mint must have 6 decimals",
    );
}

#[test]
fn rejects_a_fee_split_that_does_not_total_one_hundred_percent() {
    let mut env = Env::new();
    let mut params = env.default_protocol_params();
    params.fee_split_lp_bps = 5_000; // now sums to 9500

    let ix = env.init_protocol_ix(params);
    let admin = env.admin.insecure_clone();
    assert_err_contains(
        env.send(ix, &[&admin]),
        "Fee splits must total exactly 10000",
    );
}

/// Under-paying LPs is the single most common cause of death for pool-backed perp protocols
/// (§ 8.3): no liquidity, no depth, no traders, no fees. The constraint makes it impossible
/// to quietly invert the split rather than merely inadvisable.
#[test]
fn rejects_a_treasury_share_larger_than_the_lp_share() {
    let mut env = Env::new();
    let mut params = env.default_protocol_params();
    params.fee_split_lp_bps = 2_500;
    params.fee_split_treasury_bps = 5_500;

    let ix = env.init_protocol_ix(params);
    let admin = env.admin.insecure_clone();
    assert_err_contains(
        env.send(ix, &[&admin]),
        "Fee splits must total exactly 10000",
    );
}

#[test]
fn protocol_cannot_be_initialised_twice() {
    let mut env = Env::new();
    env.init_protocol();

    let ix = env.init_protocol_ix(env.default_protocol_params());
    let admin = env.admin.insecure_clone();
    assert!(
        env.send(ix, &[&admin]).is_err(),
        "a second initialize_protocol must fail — it would reset the fee split and \
         authorities while the vaults still hold user funds"
    );
}

// --- authority handover ------------------------------------------------------------------

/// Two steps, because a single-step transfer to a mistyped address is unrecoverable: the
/// protocol would be left with an admin key nobody holds and every risk parameter frozen.
#[test]
fn admin_handover_requires_the_new_authority_to_sign() {
    let mut env = Env::new();
    env.init_protocol();

    let new_admin = Keypair::new();
    env.svm.airdrop(&new_admin.pubkey(), 1_000_000_000).unwrap();

    let admin = env.admin.insecure_clone();
    let ix = env.transfer_admin_ix(new_admin.pubkey());
    env.send(ix, &[&admin]).unwrap();

    // Still the old admin until the handover is accepted.
    assert_eq!(env.protocol_state().admin, admin.pubkey());
    assert_eq!(env.protocol_state().pending_admin, new_admin.pubkey());

    let ix = env.accept_admin_ix(new_admin.pubkey());
    env.send(ix, &[&new_admin]).unwrap();

    let p = env.protocol_state();
    assert_eq!(p.admin, new_admin.pubkey());
    assert_eq!(
        p.pending_admin,
        Pubkey::default(),
        "the pending slot must clear, or the old nominee could claim the role again later"
    );
}

#[test]
fn a_third_party_cannot_accept_a_pending_handover() {
    let mut env = Env::new();
    env.init_protocol();

    let nominee = Keypair::new();
    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    let admin = env.admin.insecure_clone();
    let ix = env.transfer_admin_ix(nominee.pubkey());
    env.send(ix, &[&admin]).unwrap();

    let ix = env.accept_admin_ix(attacker.pubkey());
    assert_err_contains(
        env.send(ix, &[&attacker]),
        "Signer is not the pending admin",
    );
}

#[test]
fn accepting_with_no_handover_pending_fails() {
    let mut env = Env::new();
    env.init_protocol();

    let someone = Keypair::new();
    env.svm.airdrop(&someone.pubkey(), 1_000_000_000).unwrap();

    let ix = env.accept_admin_ix(someone.pubkey());
    assert_err_contains(env.send(ix, &[&someone]), "No admin transfer is pending");
}

#[test]
fn the_old_admin_loses_authority_after_handover() {
    let mut env = Env::new();
    env.init_protocol();
    env.init_market(0, &MarketSpec::eur_usd());

    let new_admin = Keypair::new();
    env.svm.airdrop(&new_admin.pubkey(), 1_000_000_000).unwrap();
    let old = env.admin.insecure_clone();

    let ix = env.transfer_admin_ix(new_admin.pubkey());
    env.send(ix, &[&old]).unwrap();
    let ix = env.accept_admin_ix(new_admin.pubkey());
    env.send(ix, &[&new_admin]).unwrap();

    let ix = env.set_status_ix(0, MarketStatus::Active);
    assert_err_contains(env.send(ix, &[&old]), "Signer is not the protocol admin");
}

// --- guardian ----------------------------------------------------------------------------

#[test]
fn guardian_can_pause_but_not_unpause() {
    let mut env = Env::new();
    env.init_protocol();

    let guardian = env.guardian.insecure_clone();
    let ix = env.pause_ix();
    env.send(ix, &[&guardian]).unwrap();
    assert!(env.protocol_state().paused);

    // The whole point of the split: a stolen hot key cannot restart a protocol the real
    // operators deliberately stopped.
    let ix = env.unpause_ix(guardian.pubkey());
    assert_err_contains(
        env.send(ix, &[&guardian]),
        "Signer is not the protocol admin",
    );
    assert!(env.protocol_state().paused);

    let admin = env.admin.insecure_clone();
    let ix = env.unpause_ix(admin.pubkey());
    env.send(ix, &[&admin]).unwrap();
    assert!(!env.protocol_state().paused);
}

#[test]
fn a_stranger_cannot_pause() {
    let mut env = Env::new();
    env.init_protocol();

    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::GuardianPause {
            guardian: attacker.pubkey(),
            protocol: env.protocol,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::EmergencyPause {}.data(),
    };
    assert_err_contains(env.send(ix, &[&attacker]), "Signer is not the guardian");
}

#[test]
fn guardian_can_halt_a_single_market() {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.list_and_activate(1, &MarketSpec::xau_usd());

    let guardian = env.guardian.insecure_clone();
    let ix = env.halt_market_ix(0);
    env.send(ix, &[&guardian]).unwrap();

    assert_eq!(env.market_state(0).status, MarketStatus::Halted);
    assert_eq!(
        env.market_state(1).status,
        MarketStatus::Active,
        "halting one market must not touch another"
    );
}

/// The guardian key holds no spending authority at all. Verified by construction rather than
/// by policy: there is no instruction that takes the guardian as a signer and moves a token.
#[test]
fn guardian_has_no_instruction_that_moves_funds() {
    let mut env = Env::new();
    env.init_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 500 * ONE_USDC).unwrap();

    let guardian = env.guardian.insecure_clone();

    // The only collateral-moving instruction takes the account owner as its signer, so the
    // guardian has to put its own key in that slot to produce a transaction it can sign at
    // all. Doing so changes the `["user", authority]` derivation, and the victim's
    // UserAccount no longer matches the seeds — the substitution is rejected on chain rather
    // than merely being awkward to attempt.
    let mut ix = env.withdraw_ix(&user, 500 * ONE_USDC);
    ix.accounts[0].pubkey = guardian.pubkey();

    assert!(
        env.send(ix, &[&guardian]).is_err(),
        "the guardian must not be able to withdraw another account's collateral"
    );
    env.assert_i1();
    assert_eq!(env.user_state(&user).free_collateral, 500 * ONE_USDC);
}

// Needed for the hand-built instruction above.
use anchor_lang::solana_program::instruction::Instruction;
