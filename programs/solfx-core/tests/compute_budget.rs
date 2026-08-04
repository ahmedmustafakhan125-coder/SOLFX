//! Compute-unit baselines and regression ceilings (`ARCHITECTURE.md` § 5.5, § 14.3).
//!
//! § 5.5 budgets `open_position` at ~120k CU and requires liquidation to stay under 200k.
//! Those are Phase 3 and Phase 4 instructions; what can be measured now is everything Phase 2
//! ships, and — more usefully — **the marginal cost of the oracle path**, which is what the
//! later budgets are built on top of.
//!
//! # Why this is a test rather than a note
//!
//! CU regressions are how instructions silently start failing at scale: nothing breaks in
//! development, and then liquidations stop landing during the congestion spike that made them
//! necessary. A ceiling that fails CI is the only version of this measurement that stays
//! true.
//!
//! Ceilings are set well above the measured figure. They are a tripwire for a step change —
//! a new account, an unbounded loop, an accidental clone — not a target to optimise against.
//!
//! Run `cargo test -p solfx-core --test compute_budget -- --nocapture` to print the table.

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
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{InstructionData, ToAccountMetas};
use common::*;
use solana_signer::Signer;
use solfx_core::state::{Direction, MarketStatus};

/// Assert and report. `limit` is the CI tripwire, not the expected value.
fn record(name: &str, used: u64, limit: u64) {
    println!("{name:<34} {used:>7} CU   (ceiling {limit})");
    assert!(
        used <= limit,
        "{name} consumed {used} CU, above its {limit} CU ceiling.\n\
         If this is a deliberate change, update the ceiling here and the table in \
         docs/compute-budget.md in the same commit."
    );
}

#[test]
fn compute_budgets_stay_within_their_ceilings() {
    println!("\n=== SolFX Phase 2 compute-unit baselines ===");

    let mut env = Env::new();

    // --- protocol setup ---
    let admin = env.admin.insecure_clone();
    let ix = env.init_protocol_ix(env.default_protocol_params());
    record(
        "initialize_protocol",
        env.send_metered(ix, &[&admin]),
        200_000,
    );

    // --- market listing ---
    let ix = env.init_market_ix(0, &MarketSpec::eur_usd());
    record(
        "initialize_market (direct)",
        env.send_metered(ix, &[&admin]),
        60_000,
    );

    let ix = env.init_market_ix(1, &MarketSpec::eur_gbp_synthetic());
    record(
        "initialize_market (synthetic)",
        env.send_metered(ix, &[&admin]),
        60_000,
    );

    let ix = env.init_market_ix(2, &MarketSpec::usd_inr());
    record(
        "initialize_market (converted)",
        env.send_metered(ix, &[&admin]),
        60_000,
    );

    for i in 0..3u16 {
        let ix = env.set_status_ix(i, MarketStatus::Active);
        let used = env.send_metered(ix, &[&admin]);
        if i == 0 {
            record("set_market_status", used, 30_000);
        }
    }

    // --- trader lifecycle ---
    let user = env.new_user(1_000 * ONE_USDC, Pubkey::default());

    let ix = env.deposit_ix(&user, 500 * ONE_USDC);
    let kp = user.keypair.insecure_clone();
    record("deposit_collateral", env.send_metered(ix, &[&kp]), 60_000);

    let ix = env.withdraw_ix(&user, 100 * ONE_USDC);
    record("withdraw_collateral", env.send_metered(ix, &[&kp]), 60_000);

    // --- the oracle path: one, two and three price accounts ---
    //
    // The delta between these three rows is the number that matters for Phase 3. § 5.5 warns
    // that a synthetic, non-USD-quoted market needs up to three price updates in one
    // transaction and may not fit; this measures the program-side half of that cost.
    let eur = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let ix = env.crank_ix(0, eur, None, None, admin.pubkey());
    record(
        "crank_market_price (1 feed)",
        env.send_metered(ix, &[&admin]),
        60_000,
    );

    let eur2 = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(13_893));
    let gbp = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(17_272));
    let ix = env.crank_ix(1, eur2, Some(gbp), None, admin.pubkey());
    record(
        "crank_market_price (2 feeds)",
        env.send_metered(ix, &[&admin]),
        80_000,
    );

    let inr = env.post_price(FEED_USD_INR, PriceSpec::at(8_842_000_000).conf(5_180_000));
    let conv = env.post_price(FEED_USD_INR, PriceSpec::at(8_842_000_000).conf(5_180_000));
    let ix = env.crank_ix(2, inr, None, Some(conv), admin.pubkey());
    record(
        "crank_market_price (converted)",
        env.send_metered(ix, &[&admin]),
        80_000,
    );

    // --- authority ---
    let ix = env.pause_ix();
    let guardian = env.guardian.insecure_clone();
    record(
        "emergency_pause",
        env.send_metered(ix, &[&guardian]),
        30_000,
    );

    let ix = env.unpause_ix(admin.pubkey());
    record("unpause", env.send_metered(ix, &[&admin]), 30_000);

    let ix = env.halt_market_ix(0);
    record("halt_market", env.send_metered(ix, &[&guardian]), 30_000);

    println!("===========================================\n");
}

/// The position lifecycle, measured separately because it is the budget § 5.5 actually sets:
/// `open_position` around 120k CU, liquidation under 200k.
///
/// These carry the full stack — a validated oracle read, execution pricing, margin checks,
/// the four-vault settlement and up to three SPL Token CPIs.
#[test]
fn position_instructions_stay_within_their_ceilings() {
    println!("\n=== SolFX Phase 3 compute-unit baselines ===");

    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.list_and_activate(1, &MarketSpec::eur_gbp_synthetic());
    env.list_and_activate(2, &MarketSpec::usd_inr());
    env.seed_pool(1_000_000 * ONE_USDC);

    let user = env.new_user(500_000 * ONE_USDC, Pubkey::default());
    env.deposit(&user, 200_000 * ONE_USDC).unwrap();
    let kp = user.keypair.insecure_clone();

    let mini = ONE_LOT / 10;
    let no_buy = i64::MAX;
    let no_sell = 1_i64;

    // --- direct, USD-quoted: the common case ---
    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let ix = env.open_ix(
        &user,
        0,
        0,
        Direction::Long,
        mini,
        2_000 * ONE_USDC,
        no_buy,
        price,
        None,
        None,
    );
    record(
        "open_position (direct)",
        env.send_metered(ix, &[&kp]),
        120_000,
    );
    env.track_position(Env::position_pda(&user.account, 0, 0));

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: env.modify_metas(&user, 0, 0, price, None, None),
        data: solfx_core::instruction::IncreasePosition {
            size_delta: mini,
            collateral_delta: 2_000 * ONE_USDC,
            price_limit: no_buy,
        }
        .data(),
    };
    record("increase_position", env.send_metered(ix, &[&kp]), 120_000);

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: env.modify_metas(&user, 0, 0, price, None, None),
        data: solfx_core::instruction::DecreasePosition {
            size_delta: mini / 2,
            price_limit: no_sell,
        }
        .data(),
    };
    record("decrease_position", env.send_metered(ix, &[&kp]), 120_000);

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: env.modify_metas(&user, 0, 0, price, None, None),
        data: solfx_core::instruction::AddPositionCollateral {
            amount: 100 * ONE_USDC,
        }
        .data(),
    };
    record(
        "add_position_collateral",
        env.send_metered(ix, &[&kp]),
        60_000,
    );

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: env.modify_metas(&user, 0, 0, price, None, None),
        data: solfx_core::instruction::RemovePositionCollateral {
            amount: 100 * ONE_USDC,
        }
        .data(),
    };
    record(
        "remove_position_collateral",
        env.send_metered(ix, &[&kp]),
        80_000,
    );

    let price = env.post_price(FEED_EUR_USD, PriceSpec::default());
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: env.close_metas(&user, 0, 0, price, None, None),
        data: solfx_core::instruction::ClosePosition {
            price_limit: no_sell,
        }
        .data(),
    };
    record(
        "close_position (direct)",
        env.send_metered(ix, &[&kp]),
        120_000,
    );

    // --- synthetic: two oracle reads ---
    let eur = env.post_price(FEED_EUR_USD, PriceSpec::at(108_500_000).conf(13_893));
    let gbp = env.post_price(FEED_GBP_USD, PriceSpec::at(127_000_000).conf(17_272));
    let ix = env.open_ix(
        &user,
        1,
        0,
        Direction::Long,
        mini,
        2_000 * ONE_USDC,
        no_buy,
        eur,
        Some(gbp),
        None,
    );
    record(
        "open_position (synthetic)",
        env.send_metered(ix, &[&kp]),
        140_000,
    );
    env.track_position(Env::position_pda(&user.account, 1, 0));

    // --- non-USD-quoted: primary plus conversion feed ---
    let tick = PriceSpec::at(8_842_000_000).conf(5_180_000);
    let inr = env.post_price(FEED_USD_INR, tick);
    let conv = env.post_price(FEED_USD_INR, tick);
    let ix = env.open_ix(
        &user,
        2,
        0,
        Direction::Long,
        1_000_000_000_000,
        500 * ONE_USDC,
        no_buy,
        inr,
        None,
        Some(conv),
    );
    record(
        "open_position (converted)",
        env.send_metered(ix, &[&kp]),
        140_000,
    );
    env.track_position(Env::position_pda(&user.account, 2, 0));

    // --- liquidity ---
    let lp = solana_keypair::Keypair::new();
    env.svm.airdrop(&lp.pubkey(), 100 * 1_000_000_000).unwrap();
    let lp_usdc = Pubkey::new_unique();
    let lp_shares = Pubkey::new_unique();
    let (mint, lp_mint) = (env.usdc_mint, env.lp_mint);
    env.write_token_account(lp_usdc, mint, lp.pubkey(), 10_000 * ONE_USDC);
    env.write_token_account(lp_shares, lp_mint, lp.pubkey(), 0);
    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::AddLiquidity {
            provider: lp.pubkey(),
            protocol: env.protocol,
            lp_pool: env.lp_pool,
            lp_vault: env.lp_vault,
            lp_mint: env.lp_mint,
            provider_token_account: lp_usdc,
            provider_lp_account: lp_shares,
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::AddLiquidity {
            amount: 10_000 * ONE_USDC,
            min_lp_out: 0,
        }
        .data(),
    };
    record("add_liquidity", env.send_metered(ix, &[&lp]), 60_000);

    println!("===========================================\n");
}

/// The account-count guard.
///
/// `initialize_protocol` carries thirteen accounts, and Anchor materialises the whole
/// `Accounts` struct on a 4 KB BPF stack frame. It already had to be boxed onto the heap once
/// to fit. If it ever grows again this fails first, with a message that says what happened —
/// rather than as an "Access violation in stack frame 5", which says nothing.
#[test]
fn the_widest_instruction_still_fits_its_stack_frame() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let ix = env.init_protocol_ix(env.default_protocol_params());
    assert_eq!(
        ix.accounts.len(),
        13,
        "initialize_protocol's account count changed. Every Account<'info, T> in it must be \
         Boxed or the instruction will overflow the BPF stack frame at runtime."
    );
    env.send(ix, &[&admin]).unwrap();
}
