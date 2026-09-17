//! Compute, packet size and account locks — measured, then asserted.
//!
//! The mirror of `programs/solfx-core/tests/compute_budget.rs`, and it exists for the same
//! reason that file gives: a ceiling nobody asserts is a ceiling that regresses silently, and
//! the regression shows up on a cluster rather than in CI. Every number printed here is what
//! the SVM actually charged.
//!
//! The three ceilings, all confirmed against the Solana MCP rather than recalled:
//!
//! | | Limit | Why it binds |
//! |---|---:|---|
//! | Compute per instruction | 200,000 default / 1,400,000 max | A funded trade must fit without a raise |
//! | Transaction packet | 1,232 bytes | Over it, the transaction is undeliverable, not slow |
//! | Account locks | 64 | `increase_tx_account_lock_limit` (128) is not activated |
//!
//! # Why NOXFUNDS is the interesting case
//!
//! `funded_open_position` is the most expensive instruction either program has, by
//! construction. It validates the mandate's rules against a price it reads itself, then makes
//! **two** CPIs into `solfx-core` — `open_position` and `place_trigger_order` — each of which
//! reads the oracle again. Three oracle loads and two cross-program invocations in one
//! instruction is the price of the guarantee that a funded position can never exist without
//! its stop.
//!
//! # Why the compute figures move between runs
//!
//! `funded_open_position` was measured six times at 109,236 / 110,736 / 112,236 (×3) /
//! 113,736 / 116,736 CU. The steps are exactly 1,500 apart, which is the cost of one
//! `create_program_address`: `solfx-core` creates the `Position` and the `TriggerOrder` with
//! `init` and an unstored `bump`, so Anchor runs `find_program_address`, which counts down
//! from 255 until it finds an off-curve address. These tests use fresh random keypairs, so
//! the canonical bumps — and therefore the number of misses — differ every run.
//!
//! Two things follow. **A given trade is deterministic**: the seeds are fixed, so one
//! trader's position always costs the same. And **the ceilings must cover the search**, which
//! is why they sit well above the base rather than snugly above one lucky measurement. It is
//! also a live demonstration of why `.claude/rules/solana.md` §3 says to store bumps — every
//! seed constraint NOXFUNDS owns uses `bump = account.bump` and costs a flat ~1,500 once.
//!
//! Run `cargo test -p noxfunds --test budgets -- --nocapture` to print the table.

// Test code asserts against known values and unwraps expected-Ok results. See the same block
// in `solfx-core/tests/compute_budget.rs` for the argument: in a program a panic is a failed
// transaction with no named error; in a test it is the reporting mechanism.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

#[path = "../../solfx-core/tests/common/mod.rs"]
mod common;

use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use common::*;
use noxfunds::instructions::investor::MandateRules;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solfx_core::state::Direction;

const MARKET_0: u128 = 1;
const SPOT: i64 = 1_085_430_000;

// --- ceilings ----------------------------------------------------------------------------
//
// Set roughly 25–35% above the measured figure: tight enough that adding an account, a loop or
// a fourth oracle read trips them, loose enough that they are not a target to optimise
// against. If one of these has to move, the table in `docs/NOXFUNDS-PLAN.md` moves in the same
// commit.

const OPEN_CEILING: u64 = 150_000;
const CLOSE_CEILING: u64 = 100_000;
const OBSERVE_CEILING: u64 = 30_000;
const CRANK_5_CEILING: u64 = 65_000;
const SETTLE_CEILING: u64 = 60_000;

const PACKET_LIMIT: usize = 1_232;
const ACCOUNT_LOCK_LIMIT: usize = 64;

// --- fixture -----------------------------------------------------------------------------
//
// The same shape as `stage6.rs`, split so that every helper hands back the `Instruction`
// instead of sending it. Measuring requires the instruction in hand.

fn nox_so_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/noxfunds.so")
}

fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[noxfunds::constants::CONFIG_SEED], &noxfunds::ID).0
}

fn mandate_pda(investor: &Pubkey, trader: &Pubkey, seq: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::MANDATE_SEED,
            investor.as_ref(),
            trader.as_ref(),
            &[seq],
        ],
        &noxfunds::ID,
    )
    .0
}

fn profile_pda(trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::TRADER_SEED, trader.as_ref()],
        &noxfunds::ID,
    )
    .0
}

/// A trader needs a record before anyone can fund them: `fund_mandate` reads the tier to bound
/// the mandate's size and the trader's concurrent count. Anyone may pay for it, which is why
/// the admin does here.
fn create_profile(env: &mut Env, payer: &solana_keypair::Keypair, trader: &Pubkey) {
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeTraderProfile {
            payer: payer.pubkey(),
            authority: *trader,
            profile: profile_pda(trader),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeTraderProfile {}.data(),
    };
    env.send(ix, &[payer]).expect("create the trader profile");
}

fn signer_pda(mandate: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::MANDATE_SIGNER_SEED, mandate.as_ref()],
        &noxfunds::ID,
    )
    .0
}

struct Nox {
    env: Env,
    investor: Keypair,
    trader: Keypair,
    mandate: Pubkey,
    signer: Pubkey,
    vault: Pubkey,
}

/// Destination accounts for the three payees.
struct Payees {
    investor: Pubkey,
    trader: Pubkey,
    treasury: Pubkey,
}

/// $1,000 principal, $1,000 deposited, generous drawdown — a mandate that survives a round
/// trip, because this suite measures cost rather than rule enforcement.
fn roomy_mandate() -> Nox {
    let rules = MandateRules {
        max_trade_notional: 2_000 * ONE_USDC,
        max_total_notional: 6_000 * ONE_USDC,
        max_drawdown_bps: 1_000,
        max_daily_loss_bps: 1_000,
        max_risk_per_trade_bps: 100,
        max_stop_distance_bps: 200,
        max_concurrent_positions: 5,
        allowed_markets: MARKET_0,
        min_hold_slots: 0,
    };

    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();

    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let investor = Keypair::new();
    let trader = Keypair::new();
    for k in [&investor, &trader] {
        env.svm.airdrop(&k.pubkey(), 100 * 1_000_000_000).unwrap();
    }

    let admin = env.admin.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeConfig {
            admin: admin.pubkey(),
            config: config_pda(),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeConfig {
            guardian: env.guardian.pubkey(),
            treasury: admin.pubkey(),
            usdc_mint: env.usdc_mint,
        }
        .data(),
    };
    env.send(ix, &[&admin]).unwrap();

    create_profile(&mut env, &admin, &trader.pubkey());

    let mandate = mandate_pda(&investor.pubkey(), &trader.pubkey(), 0);
    let signer = signer_pda(&mandate);
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundMandate {
            trader_profile: profile_pda(&trader.pubkey()),
            investor: investor.pubkey(),
            config: config_pda(),
            trader: trader.pubkey(),
            mandate,
            mandate_signer: signer,
            solfx_user_account: Env::user_pda(&signer),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundMandate {
            seq: 0,
            principal: 1_000 * ONE_USDC,
            rules,
        }
        .data(),
    };
    let inv = investor.insecure_clone();
    env.send(ix, &[&inv]).unwrap();

    let mut nox = Nox {
        env,
        investor,
        trader,
        mandate,
        signer,
        vault: Pubkey::default(),
    };
    nox.fund_for_trading(1_000 * ONE_USDC);
    nox
}

impl Nox {
    fn fund_for_trading(&mut self, usdc: u64) {
        self.env.svm.airdrop(&self.signer, 1_000_000_000).unwrap();
        let vault = Pubkey::new_unique();
        self.vault = vault;
        self.env
            .write_token_account(vault, self.env.usdc_mint, self.signer, usdc);
        let payer = self.investor.insecure_clone();
        let user_account = Env::user_pda(&self.signer);

        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::CreateSolfxAccount {
                payer: payer.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::CreateSolfxAccount {}.data(),
        };
        self.env.send(ix, &[&payer]).expect("create solfx account");
        self.env.track_user(user_account);

        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundSolfxCollateral {
                payer: payer.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                collateral_mint: self.env.usdc_mint,
                collateral_vault: self.env.collateral_vault,
                mandate_vault: vault,
                token_program: spl_token::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundSolfxCollateral { amount: usdc }.data(),
        };
        self.env.send(ix, &[&payer]).expect("fund collateral");
    }

    /// The trader's own transaction, so the trader is the fee payer.
    ///
    /// Stage 0 first measured this at 1,056 bytes rather than the derived 959 because the test
    /// used a fresh fee payer — a 25th account key and a second signature that no real client
    /// pays for. Keeping the signer and the payer the same keeps the measurement honest.
    fn open_ix(&mut self, nonce: u8, size_base: u64, stop: i64, collateral: u64) -> Instruction {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        let position = Env::position_pda(&user_account, 0, nonce);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedOpenPosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position,
                trigger_order: Env::trigger_pda(&position, nonce),
                collateral_vault: self.env.collateral_vault,
                lp_pool: self.env.lp_pool,
                lp_vault: self.env.lp_vault,
                insurance_fund: self.env.insurance_fund,
                insurance_vault: self.env.insurance_vault,
                fee_vault: self.env.fee_vault,
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedOpenPosition {
                market_index: 0,
                nonce,
                direction: Direction::Long,
                size_base,
                collateral,
                price_limit: i64::MAX,
                order_id: nonce,
                stop_loss_price: stop,
            }
            .data(),
        }
    }

    /// Closing a long is a sell, so its slippage bound is a minimum.
    fn close_ix(&mut self, nonce: u8) -> Instruction {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedClosePosition {
                trader_profile: profile_pda(&self.trader.pubkey()),
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position: Env::position_pda(&user_account, 0, nonce),
                collateral_vault: self.env.collateral_vault,
                lp_pool: self.env.lp_pool,
                lp_vault: self.env.lp_vault,
                insurance_fund: self.env.insurance_fund,
                insurance_vault: self.env.insurance_vault,
                fee_vault: self.env.fee_vault,
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedClosePosition {
                market_index: 0,
                nonce,
                price_limit: 1,
            }
            .data(),
        }
    }

    /// The crank carries one `(position, market, price)` triple per open position in
    /// `remaining_accounts`, so its account count is a function of concurrency. That is the
    /// figure `max_concurrent_positions` has to be chosen against.
    fn observe_ix(&mut self, nonces: &[u8]) -> (Instruction, Keypair) {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let observer = Keypair::new();
        self.env
            .svm
            .airdrop(&observer.pubkey(), 1_000_000_000)
            .unwrap();
        let user_account = Env::user_pda(&self.signer);

        let mut metas = noxfunds::accounts::ObserveMandateEquity {
            trader_profile: profile_pda(&self.trader.pubkey()),
            observer: observer.pubkey(),
            config: config_pda(),
            mandate: self.mandate,
            mandate_signer: self.signer,
            user_account,
        }
        .to_account_metas(None);
        for n in nonces {
            metas.push(AccountMeta::new_readonly(
                Env::position_pda(&user_account, 0, *n),
                false,
            ));
            metas.push(AccountMeta::new_readonly(Env::market_pda(0), false));
            metas.push(AccountMeta::new_readonly(p, false));
        }

        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: metas,
            data: noxfunds::instruction::ObserveMandateEquity {}.data(),
        };
        (ix, observer)
    }

    fn payees(&mut self) -> Payees {
        let mint = self.env.usdc_mint;
        let (investor, trader, treasury) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let inv = self.investor.pubkey();
        let tr = self.trader.pubkey();
        let admin = self.env.admin.pubkey();
        self.env.write_token_account(investor, mint, inv, 0);
        self.env.write_token_account(trader, mint, tr, 0);
        self.env.write_token_account(treasury, mint, admin, 0);
        Payees {
            investor,
            trader,
            treasury,
        }
    }

    fn request_settlement(&mut self) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::RequestSettlement {
                investor: self.investor.pubkey(),
                mandate: self.mandate,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::RequestSettlement {}.data(),
        };
        let k = self.investor.insecure_clone();
        self.env.send(ix, &[&k])
    }

    /// A stranger settles — the investor must never have to wait on anyone.
    fn claim_ix(&mut self, p: &Payees) -> (Instruction, Keypair) {
        let settler = Keypair::new();
        self.env
            .svm
            .airdrop(&settler.pubkey(), 1_000_000_000)
            .unwrap();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::ClaimSettlement {
                trader_profile: profile_pda(&self.trader.pubkey()),
                settler: settler.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: Env::user_pda(&self.signer),
                usdc_mint: self.env.usdc_mint,
                collateral_vault: self.env.collateral_vault,
                mandate_vault: self.vault,
                investor_token: p.investor,
                trader_token: p.trader,
                treasury_token: p.treasury,
                token_program: spl_token::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::ClaimSettlement {}.data(),
        };
        (ix, settler)
    }
}

// --- the measurements ---------------------------------------------------------------------

/// Assert the packet and lock limits, and hand back the wire size for the table.
///
/// Measured the way a client actually builds it — with the two compute-budget instructions in
/// front, because those are not optional in production and they cost bytes too. Same helper
/// `solfx-core`'s packet test uses, so the two suites' figures are comparable rather than
/// merely similar.
fn wire(env: &Env, name: &str, ix: &Instruction, payer: &Keypair) -> usize {
    let accounts = ix.accounts.len();
    assert!(
        accounts <= ACCOUNT_LOCK_LIMIT,
        "{name} locks {accounts} accounts, over the {ACCOUNT_LOCK_LIMIT} limit. \
         `increase_tx_account_lock_limit` is not an activated feature."
    );
    let size = env.measure_keeper_tx(ix.clone(), payer);
    assert!(
        size <= PACKET_LIMIT,
        "{name} serialises to {size} bytes, over the {PACKET_LIMIT}-byte packet limit. \
         It cannot be sent as a single transaction."
    );
    size
}

/// **The whole NOXFUNDS budget on one screen.**
///
/// One full mandate lifecycle — open a funded position, crank its equity, close it, settle —
/// with compute, wire size and account locks recorded at every step.
#[test]
fn every_noxfunds_instruction_stays_within_its_budget() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    let stop = SPOT - (SPOT * 30) / 10_000;
    let trader = nox.trader.insecure_clone();

    println!("\n=== NOXFUNDS budgets (EUR/USD, one open position) ===");
    println!("{:<26} {:>8} {:>10} {:>12}", "", "accounts", "bytes", "CU");

    // --- funded_open_position -------------------------------------------------------------
    // Prices the market to check the rules, then CPIs twice — each CPI prices it again.
    let ix = nox.open_ix(0, ONE_LOT / 100, stop, 500 * ONE_USDC);
    let accounts = ix.accounts.len();
    let bytes = wire(&nox.env, "funded_open_position", &ix, &trader);
    let cu = nox.env.send_metered(ix, &[&trader]);
    println!(
        "{:<26} {accounts:>8} {bytes:>10} {cu:>12}",
        "funded_open_position"
    );
    assert!(
        cu <= OPEN_CEILING,
        "funded_open_position consumed {cu} CU, above its {OPEN_CEILING} ceiling"
    );
    nox.env
        .track_position(Env::position_pda(&Env::user_pda(&nox.signer), 0, 0));

    // The account count is a design constraint, not an observation: it is what the two-CPI
    // shape costs, and it is the input to the stack-frame analysis.
    //
    // The plan derived **22**, counting a `trader_profile`. The profile now exists, but it is
    // not on this path: nothing about *opening* a position changes a trader's record. It is
    // written by the close (realised PnL, hold time), by the equity crank (observed drawdown)
    // and by settlement (the mandate's outcome). Keeping it off the open leaves the most
    // expensive instruction at 21 accounts instead of 22.
    assert_eq!(
        accounts, 21,
        "the two-CPI shape is 21 accounts; a change here changes the stack-frame analysis"
    );

    // --- observe_mandate_equity -----------------------------------------------------------
    let (ix, observer) = nox.observe_ix(&[0]);
    let accounts = ix.accounts.len();
    let bytes = wire(&nox.env, "observe_mandate_equity", &ix, &observer);
    let cu = nox.env.send_metered(ix, &[&observer]);
    println!(
        "{:<26} {accounts:>8} {bytes:>10} {cu:>12}",
        "observe_mandate_equity"
    );
    assert!(
        cu <= OBSERVE_CEILING,
        "observe_mandate_equity consumed {cu} CU, above its {OBSERVE_CEILING} ceiling"
    );

    // --- funded_close_position --------------------------------------------------------------
    let ix = nox.close_ix(0);
    let accounts = ix.accounts.len();
    let bytes = wire(&nox.env, "funded_close_position", &ix, &trader);
    let cu = nox.env.send_metered(ix, &[&trader]);
    println!(
        "{:<26} {accounts:>8} {bytes:>10} {cu:>12}",
        "funded_close_position"
    );
    assert!(
        cu <= CLOSE_CEILING,
        "funded_close_position consumed {cu} CU, above its {CLOSE_CEILING} ceiling"
    );

    // --- claim_settlement -------------------------------------------------------------------
    // Withdraws everything from SolFX, then pays three parties out of the mandate vault.
    nox.request_settlement().expect("the investor requests");
    let (ix, settler) = nox.claim_ix(&payees);
    let accounts = ix.accounts.len();
    let bytes = wire(&nox.env, "claim_settlement", &ix, &settler);
    let cu = nox.env.send_metered(ix, &[&settler]);
    println!(
        "{:<26} {accounts:>8} {bytes:>10} {cu:>12}",
        "claim_settlement"
    );
    assert!(
        cu <= SETTLE_CEILING,
        "claim_settlement consumed {cu} CU, above its {SETTLE_CEILING} ceiling"
    );

    println!(
        "{:<26} {ACCOUNT_LOCK_LIMIT:>8} {PACKET_LIMIT:>10} {:>12}",
        "limits", 200_000
    );
    println!("=====================================================\n");

    nox.env.assert_invariants();
}

/// **A mandate carrying its maximum number of positions still cranks in one transaction.**
///
/// `observe_mandate_equity` takes one `(position, market, price)` triple per open position in
/// `remaining_accounts`, so its cost scales with concurrency. That makes the crank — not the
/// trade — the instruction that decides how high `MAX_SLOTS` can go, and it is the reason this
/// is measured at the maximum rather than at one. A crank that does not fit is a mandate whose
/// equity cannot be checked, which is a mandate that cannot be breached or wound down.
///
/// Two figures here: the five-position case is **sent**, so its compute is real; the
/// eight-position case is **built and measured but not sent**, because `MAX_SLOTS` is 8 and
/// wire size does not require the accounts to exist. Both are measurements, neither is an
/// extrapolation.
#[test]
fn a_fully_loaded_crank_fits_in_one_transaction() {
    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    let trader = nox.trader.insecure_clone();

    // Five concurrent positions, which is what this mandate's rules permit. $180 of margin
    // each keeps the total inside the $1,000 deposit — the earlier $500 was sized for a test
    // that only ever opened one.
    for nonce in 0..5u8 {
        let ix = nox.open_ix(nonce, ONE_LOT / 100, stop, 180 * ONE_USDC);
        nox.env.send(ix, &[&trader]).expect("open");
        nox.env
            .track_position(Env::position_pda(&Env::user_pda(&nox.signer), 0, nonce));
    }

    let (ix, observer) = nox.observe_ix(&[0, 1, 2, 3, 4]);
    let accounts = ix.accounts.len();
    let bytes = wire(&nox.env, "observe_mandate_equity ×5", &ix, &observer);
    let cu = nox.env.send_metered(ix, &[&observer]);
    println!("\n  crank over 5 positions: {accounts} accounts, {bytes} bytes, {cu} CU");
    assert!(
        cu <= CRANK_5_CEILING,
        "a five-position crank consumed {cu} CU, above its {CRANK_5_CEILING} ceiling"
    );
    // 6 fixed + 3 per position. The market and the price repeat, so only the position address
    // is new each time — which is why the wire cost per extra position is far below 3 × 32.
    assert_eq!(accounts, 21);

    // The worst case the state can produce. Built at full width and measured; not sent,
    // because nonces 5–7 hold no position and the packet does not care.
    let (widest, keeper) = nox.observe_ix(&[0, 1, 2, 3, 4, 5, 6, 7]);
    let at_max = wire(
        &nox.env,
        "observe_mandate_equity ×MAX_SLOTS",
        &widest,
        &keeper,
    );
    println!(
        "  crank over MAX_SLOTS=8: {} accounts, {at_max} bytes / {PACKET_LIMIT}\n",
        widest.accounts.len()
    );

    nox.env.assert_invariants();
}

/// **Throughput, derived from the measured cost rather than asserted in prose.**
///
/// Every funded trade write-locks the same SolFX accounts — `lp_pool`, `lp_vault`,
/// `collateral_vault`, `insurance_vault`, `fee_vault`, `protocol` and the `Market`. Solana caps
/// a single **writable account** at 12,000,000 CU per block, and that is the cap that actually
/// bounds a venue whose every trade touches one LP pool. SIMD-0286's 100M block limit does not
/// help: the hot account tops out at 12% of it. SIMD-0306 would raise the per-account cap to
/// 24M, and is not activated.
///
/// This asserts a floor rather than printing a number, so a CU regression surfaces as a
/// throughput claim that stopped being true.
#[test]
fn the_write_lock_cap_supports_a_useful_trade_rate() {
    /// Per-writable-account compute per block. Unchanged by SIMD-0286; SIMD-0306 (24M) and
    /// SIMD-0525 are not activated. Confirmed via the Solana MCP.
    const PER_ACCOUNT_CU_PER_BLOCK: u64 = 12_000_000;
    /// 400 ms slots — 2.5 blocks a second, carried ×10 to stay in integers.
    const BLOCKS_PER_SECOND_X10: u64 = 25;

    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    let trader = nox.trader.insecure_clone();
    let ix = nox.open_ix(0, ONE_LOT / 100, stop, 500 * ONE_USDC);
    let cu = nox.env.send_metered(ix, &[&trader]);
    nox.env
        .track_position(Env::position_pda(&Env::user_pda(&nox.signer), 0, 0));

    let per_block = PER_ACCOUNT_CU_PER_BLOCK / cu;
    let per_second = per_block * BLOCKS_PER_SECOND_X10 / 10;
    println!("\n  funded_open_position: {cu} CU");
    println!("  → {per_block} per block, ≈{per_second} funded opens/second");
    println!("  (bound by the 12M per-writable-account cap on the shared LP pool,");
    println!("   not by the 100M block limit — every trade locks the same pool)\n");

    assert!(
        per_second >= 100,
        "at {cu} CU a funded open only reaches {per_second}/s against the write-lock cap"
    );

    nox.env.assert_invariants();
}
