//! Collateral custody and **invariant I1** (`ARCHITECTURE.md` § 12.3).
//!
//! `CollateralVault.amount == Σ(UserAccount.free) + Σ(Position.collateral)`
//!
//! Checked after every instruction, three ways: the vault's token balance, the sum over user
//! accounts, and the running total the program maintains. Comparing only two would let a bug
//! that corrupts both in the same direction pass.
//!
//! The other property here is the load-bearing product claim: **your collateral never leaves
//! your own account.** If a pause, an admin, or a guardian can hold a user's free collateral,
//! SolFX is XM with extra steps.

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
use common::*;
use solana_keypair::Keypair;
use solana_signer::Signer;

fn env_with_protocol() -> Env {
    let mut env = Env::new();
    env.init_protocol();
    env
}

#[test]
fn deposit_and_withdraw_preserve_the_invariant() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    env.assert_i1();

    env.deposit(&user, 400 * ONE_USDC).unwrap();
    env.assert_i1();
    assert_eq!(env.user_state(&user).free_collateral, 400 * ONE_USDC);
    assert_eq!(env.token_balance(&env.collateral_vault), 400 * ONE_USDC);
    assert_eq!(env.token_balance(&user.token_account), 600 * ONE_USDC);

    env.withdraw(&user, 150 * ONE_USDC).unwrap();
    env.assert_i1();
    assert_eq!(env.user_state(&user).free_collateral, 250 * ONE_USDC);
    assert_eq!(env.token_balance(&user.token_account), 750 * ONE_USDC);

    // Full exit must be possible, and must leave the vault empty.
    env.withdraw(&user, 250 * ONE_USDC).unwrap();
    env.assert_i1();
    assert_eq!(env.user_state(&user).free_collateral, 0);
    assert_eq!(env.token_balance(&env.collateral_vault), 0);
    assert_eq!(env.token_balance(&user.token_account), 1_000 * ONE_USDC);
}

/// Many users, interleaved, with the invariant re-checked after each step. This is the shape
/// of bug the three-way check exists for: an accounting error that only appears once more
/// than one account is in play.
#[test]
fn the_invariant_holds_across_interleaved_users() {
    let mut env = env_with_protocol();
    let users: Vec<_> = (0..5)
        .map(|_| env.new_user(1_000 * ONE_USDC, Pubkey::default()))
        .collect();

    let script: &[(usize, i64)] = &[
        (0, 100),
        (1, 250),
        (2, 75),
        (0, -50),
        (3, 900),
        (1, -250),
        (4, 33),
        (2, -75),
        (3, -400),
        (0, 500),
        (4, -33),
        (3, -500),
    ];

    for &(who, delta) in script {
        let u = &users[who];
        if delta > 0 {
            env.deposit(u, u64::try_from(delta).unwrap() * ONE_USDC)
                .unwrap();
        } else {
            env.withdraw(u, u64::try_from(-delta).unwrap() * ONE_USDC)
                .unwrap();
        }
        env.assert_i1();
    }

    let p = env.protocol_state();
    assert_eq!(p.total_deposits, 1_858 * ONE_USDC);
    assert_eq!(p.total_withdrawals, 1_308 * ONE_USDC);
    assert_eq!(p.total_user_collateral, 550 * ONE_USDC);
}

#[test]
fn cannot_withdraw_more_than_the_free_balance() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 100 * ONE_USDC).unwrap();

    assert_err_contains(
        env.withdraw(&user, 100 * ONE_USDC + 1),
        "Insufficient free collateral",
    );
    env.assert_i1();
}

#[test]
fn cannot_withdraw_from_an_empty_account() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());

    assert_err_contains(env.withdraw(&user, 1), "Insufficient free collateral");
    env.assert_i1();
}

#[test]
fn zero_amount_transfers_are_rejected() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());

    assert_err_contains(env.deposit(&user, 0), "Amount must be greater than zero");
    assert_err_contains(env.withdraw(&user, 0), "Amount must be greater than zero");
    env.assert_i1();
}

/// **The load-bearing claim.** A pause exists to stop new risk being taken. It must never
/// hold a user's own money, or "non-custodial" is not true.
#[test]
fn withdrawals_survive_a_global_pause() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 600 * ONE_USDC).unwrap();

    let guardian = env.guardian.insecure_clone();
    let ix = env.pause_ix();
    env.send(ix, &[&guardian]).unwrap();
    assert!(env.protocol_state().paused);

    env.withdraw(&user, 600 * ONE_USDC)
        .expect("a paused protocol must still release free collateral");
    env.assert_i1();
    assert_eq!(env.token_balance(&user.token_account), 1_000 * ONE_USDC);
}

/// Adding collateral reduces risk, so blocking it during a pause would push positions toward
/// liquidation during exactly the incident the pause was called for.
#[test]
fn deposits_survive_a_global_pause() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());

    let guardian = env.guardian.insecure_clone();
    let ix = env.pause_ix();
    env.send(ix, &[&guardian]).unwrap();

    env.deposit(&user, 100 * ONE_USDC).unwrap();
    env.assert_i1();
}

// --- account substitution (threat T4) ----------------------------------------------------

#[test]
fn cannot_deposit_from_someone_elses_token_account() {
    let mut env = env_with_protocol();
    let victim = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    let attacker = env.new_user(0, Pubkey::default());

    // Attacker's own user account, but pointing at the victim's wallet.
    let mut ix = env.deposit_ix(&attacker, 500 * ONE_USDC);
    let victim_token = victim.token_account;
    for meta in &mut ix.accounts {
        if meta.pubkey == attacker.token_account {
            meta.pubkey = victim_token;
        }
    }

    let kp = attacker.keypair.insecure_clone();
    assert!(
        env.send(ix, &[&kp]).is_err(),
        "a token account owned by someone else must not be spendable here"
    );
    env.assert_i1();
}

#[test]
fn cannot_withdraw_into_someone_elses_account_using_their_pda() {
    let mut env = env_with_protocol();
    let victim = env.new_user(1_000 * ONE_USDC, Pubkey::default());
    env.deposit(&victim, 800 * ONE_USDC).unwrap();

    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let attacker_token = Pubkey::new_unique();
    let mint = env.usdc_mint;
    env.write_token_account(attacker_token, mint, attacker.pubkey(), 0);

    // The victim's UserAccount PDA, the attacker's signature and destination.
    let mut ix = env.withdraw_ix(&victim, 800 * ONE_USDC);
    ix.accounts[0].pubkey = attacker.pubkey();
    let victim_token = victim.token_account;
    for meta in &mut ix.accounts {
        if meta.pubkey == victim_token {
            meta.pubkey = attacker_token;
        }
    }

    assert!(
        env.send(ix, &[&attacker]).is_err(),
        "the UserAccount PDA is derived from its authority — another signer must not reach it"
    );
    env.assert_i1();
    assert_eq!(env.token_balance(&attacker_token), 0);
}

#[test]
fn a_token_account_for_the_wrong_mint_is_rejected() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());

    let other_mint = Pubkey::new_unique();
    env.write_mint_with_decimals(other_mint, 6);
    let wrong = Pubkey::new_unique();
    let authority = user.pubkey();
    env.write_token_account(wrong, other_mint, authority, 1_000 * ONE_USDC);

    let mut ix = env.deposit_ix(&user, 100 * ONE_USDC);
    let user_token = user.token_account;
    for meta in &mut ix.accounts {
        if meta.pubkey == user_token {
            meta.pubkey = wrong;
        }
    }

    let kp = user.keypair.insecure_clone();
    assert!(
        env.send(ix, &[&kp]).is_err(),
        "a token account for a different mint must not credit collateral the vault \
         does not hold"
    );
    env.assert_i1();
}

// --- referral binding (§ 8.5) ------------------------------------------------------------

/// The referrer is written once at account creation and there is no instruction anywhere in
/// the protocol that changes it. Every IB in retail FX has the same complaint — the broker
/// reassigns their clients — and an immutable binding is the structural fix.
#[test]
fn the_referrer_is_bound_at_creation() {
    let mut env = env_with_protocol();
    let ib = Pubkey::new_unique();
    let user = env.new_user(1_000 * ONE_USDC, ib);

    assert_eq!(env.user_state(&user).referrer, ib);

    // Activity does not disturb it.
    env.deposit(&user, 500 * ONE_USDC).unwrap();
    env.withdraw(&user, 200 * ONE_USDC).unwrap();
    assert_eq!(env.user_state(&user).referrer, ib);
    env.assert_i1();
}

#[test]
fn a_user_cannot_refer_themselves() {
    let mut env = env_with_protocol();
    let keypair = Keypair::new();
    env.svm.airdrop(&keypair.pubkey(), 1_000_000_000).unwrap();

    let ix = env.create_user_ix(&keypair.pubkey(), keypair.pubkey());
    assert_err_contains(env.send(ix, &[&keypair]), "Parameter out of range");
}

#[test]
fn an_account_cannot_be_created_twice() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::new_unique());

    // A second init with a different referrer would be a reassignment.
    let ix = env.create_user_ix(&user.pubkey(), Pubkey::new_unique());
    let kp = user.keypair.insecure_clone();
    assert!(
        env.send(ix, &[&kp]).is_err(),
        "re-initialising a UserAccount would rewrite the referrer binding"
    );
}

#[test]
fn lifetime_totals_track_every_movement() {
    let mut env = env_with_protocol();
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());

    env.deposit(&user, 300 * ONE_USDC).unwrap();
    env.deposit(&user, 200 * ONE_USDC).unwrap();
    env.withdraw(&user, 100 * ONE_USDC).unwrap();

    let u = env.user_state(&user);
    assert_eq!(u.total_deposits, 500 * ONE_USDC);
    assert_eq!(u.total_withdrawals, 100 * ONE_USDC);
    assert_eq!(u.free_collateral, 400 * ONE_USDC);
    env.assert_i1();
}
