//! Stage 6 — a mandate reaches settlement with nobody's cooperation.
//!
//! Part 10's exit criterion: *a breached mandate winds down with **neither** the trader nor the
//! operator cooperating.* Stage 2 got a mandate to `Breached` on a stranger's crank; Stage 5
//! paid it out once flat. This is the middle: closing the positions, cancelling the stops, and
//! reconciling a position that closed where NOXFUNDS could not see it.

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
#[allow(dead_code)]
mod support;

use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use common::*;
use noxfunds::instructions::investor::MandateRules;
use noxfunds::state::MandateState;
use solana_signer::Signer;
use solfx_core::state::Direction;

const MARKET_0: u128 = 1;
const SPOT: i64 = 1_085_430_000;

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
    investor: solana_keypair::Keypair,
    trader: solana_keypair::Keypair,
    mandate: Pubkey,
    signer: Pubkey,
    /// The mandate's USDC account. Kept because settlement pays out of it.
    vault: Pubkey,
}

/// A mandate whose principal and deposit are whatever the test needs.
///
/// Size matters for the drawdown tests: a $20,000 mandate would need a $600 loss to breach a
/// 3% limit, which no position inside a $2,000 per-trade ceiling can produce. A small mandate
/// makes an ordinary adverse move a real breach rather than a contrived one.
fn setup_funded(rules: MandateRules, principal: u64, deposit: u64) -> Nox {
    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
    // Loaded the way a real deploy leaves it: the deployer holds the upgrade authority, which
    // is what `initialize_config` checks.
    let upgrade_authority = env.admin.pubkey();
    support::set_upgrade_authority(&mut env, Some(upgrade_authority));

    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let investor = solana_keypair::Keypair::new();
    let trader = solana_keypair::Keypair::new();
    for k in [&investor, &trader] {
        env.svm.airdrop(&k.pubkey(), 100 * 1_000_000_000).unwrap();
    }

    let admin = env.admin.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeConfig {
            program: noxfunds::ID,
            program_data: support::program_data_address(),
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
    // The investor holds USDC, and `fund_mandate` moves the principal out of it into the
    // mandate's own vault. Before the funding fix these tests wrote the vault balance directly,
    // which is exactly why a mandate with a principal nobody deposited went unnoticed.
    let investor_token = Pubkey::new_unique();
    env.write_token_account(
        investor_token,
        env.usdc_mint,
        investor.pubkey(),
        support::INVESTOR_START,
    );
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundMandate {
            trader_profile: profile_pda(&trader.pubkey()),
            usdc_mint: env.usdc_mint,
            investor_token,
            mandate_vault: support::vault_pda(&mandate),
            token_program: spl_token::ID,
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
            principal,
            rules,
        }
        .data(),
    };
    // The trader co-signs: a mandate spends their capacity, so it needs their consent (R-8).
    let inv = investor.insecure_clone();
    env.send(ix, &[&inv, &trader]).unwrap();

    let mut nox = Nox {
        env,
        investor,
        trader,
        mandate,
        signer,
        vault: Pubkey::default(),
    };
    nox.fund_for_trading(deposit);
    nox
}

/// A mandate small enough that an ordinary adverse move breaches it.
fn small_mandate() -> Nox {
    setup_funded(
        MandateRules {
            max_drawdown_bps: 100,
            max_daily_loss_bps: 100,
            max_risk_per_trade_bps: 100,
            min_hold_slots: 0,
            ..base_rules()
        },
        1_000 * ONE_USDC,
        1_000 * ONE_USDC,
    )
}

/// 250 bps below spot — a real loss.
fn adverse() -> PriceSpec {
    PriceSpec::at(105_830_000).conf(13_893)
}

fn base_rules() -> MandateRules {
    MandateRules {
        max_trade_notional: 2_000 * ONE_USDC,
        max_total_notional: 6_000 * ONE_USDC,
        max_drawdown_bps: 300,
        max_daily_loss_bps: 300,
        max_risk_per_trade_bps: 100,
        max_stop_distance_bps: 200,
        max_concurrent_positions: 5,
        allowed_markets: MARKET_0,
        min_hold_slots: 0,
    }
}

impl Nox {
    fn fund_for_trading(&mut self, usdc: u64) {
        self.env.svm.airdrop(&self.signer, 1_000_000_000).unwrap();
        // Filled by `fund_mandate`; this only moves what is already there into SolFX.
        let vault = support::vault_pda(&self.mandate);
        self.vault = vault;
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

    #[allow(clippy::too_many_arguments)]
    fn open(&mut self, nonce: u8, size_base: u64, stop: i64) -> TestResult {
        self.open_dir(nonce, size_base, stop, Direction::Long)
    }

    fn open_dir(
        &mut self,
        nonce: u8,
        size_base: u64,
        stop: i64,
        direction: Direction,
    ) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        let position = Env::position_pda(&user_account, 0, nonce);
        let ix = Instruction {
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
                direction,
                size_base,
                collateral: 500 * ONE_USDC,
                // A long opens by buying, so its bound is a maximum; a short opens by
                // selling, so its bound is a minimum. There is no "disabled" slippage.
                price_limit: match direction {
                    Direction::Long => i64::MAX,
                    Direction::Short => 1,
                },
                order_id: nonce,
                stop_loss_price: stop,
            }
            .data(),
        };
        let trader = self.trader.insecure_clone();
        let r = self.env.send(ix, &[&trader]);
        if r.is_ok() {
            self.env.track_position(position);
        }
        r
    }

    /// Closing a long is a sell (bound is a minimum); closing a short is a buy (a maximum).
    /// `wind_down_position` derives this from the position rather than trusting a caller,
    /// which is why the wind-down tests never hit it.
    fn close_dir(&mut self, nonce: u8, direction: Direction) -> TestResult {
        let limit = match direction {
            Direction::Long => 1,
            Direction::Short => i64::MAX,
        };
        self.close_with(PriceSpec::default(), nonce, limit)
    }

    fn close_with(&mut self, spec: PriceSpec, nonce: u8, price_limit: i64) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, spec);
        let user_account = Env::user_pda(&self.signer);
        let ix = Instruction {
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
                price_limit,
            }
            .data(),
        };
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    /// Run the permissionless crank at the current market price.
    fn observe(&mut self, nonces: &[u8]) -> TestResult {
        self.observe_at(PriceSpec::default(), nonces)
    }

    /// Run the crank at a chosen price.
    ///
    /// The price is a parameter rather than always the default because an earlier version
    /// posted `PriceSpec::default()` inside `observe`, which silently overwrote the adverse
    /// price a drawdown test had just set — the mandate then observed as healthy and the test
    /// failed for a reason that had nothing to do with the rule.
    fn observe_at(&mut self, spec: PriceSpec, nonces: &[u8]) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, spec);
        let observer = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&observer.pubkey(), 1_000_000_000)
            .unwrap();
        let user_account = Env::user_pda(&self.signer);

        let mut metas = noxfunds::accounts::ObserveMandateEquity {
            trader_profile: profile_pda(&self.trader.pubkey()),
            observer: observer.pubkey(),
            mandate: self.mandate,
            mandate_vault: support::vault_pda(&self.mandate),
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
        self.env.send(ix, &[&observer])
    }

    fn mandate(&self) -> noxfunds::state::Mandate {
        self.env.read(&self.mandate)
    }
}

// --- settlement ------------------------------------------------------------------------

/// Destination accounts for the three payees. Zero balances, so every unit they end with is
/// a unit settlement put there.
struct Payees {
    investor: Pubkey,
    trader: Pubkey,
    treasury: Pubkey,
}

impl Nox {
    fn payees(&mut self) -> Payees {
        let mint = self.env.usdc_mint;
        let investor = Pubkey::new_unique();
        let trader = Pubkey::new_unique();
        let treasury = Pubkey::new_unique();
        let inv = self.investor.pubkey();
        let tr = self.trader.pubkey();
        // `initialize_config` in the fixture sets the treasury to the admin.
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

    fn request_settlement(&mut self, who: &solana_keypair::Keypair) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::RequestSettlement {
                investor: who.pubkey(),
                mandate: self.mandate,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::RequestSettlement {}.data(),
        };
        let k = who.insecure_clone();
        self.env.send(ix, &[&k])
    }

    fn claim(&mut self, p: &Payees) -> TestResult {
        // A stranger settles. The investor must never have to wait on anyone.
        let settler = solana_keypair::Keypair::new();
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
        self.env.send(ix, &[&settler])
    }
}

/// The small mandate from Stage 2 is too tight to trade a profit through; settlement tests
/// want room. $1,000 principal, $1,000 deposited, generous drawdown.
fn roomy_mandate() -> Nox {
    setup_funded(
        MandateRules {
            max_drawdown_bps: 1_000,
            max_daily_loss_bps: 1_000,
            max_risk_per_trade_bps: 100,
            ..base_rules()
        },
        1_000 * ONE_USDC,
        1_000 * ONE_USDC,
    )
}

impl Nox {
    /// Fire a resting stop the way a stranger's keeper would — straight into `solfx-core`,
    /// never through NOXFUNDS. That path is the whole reason `reconcile_position` exists.
    fn fire_stop(&mut self, nonce: u8, order_id: u8) {
        let keeper = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&keeper.pubkey(), 100 * 1_000_000_000)
            .unwrap();
        let user_account = Env::user_pda(&self.signer);
        let position = Env::position_pda(&user_account, 0, nonce);
        let hit = self
            .env
            .post_price_now(FEED_EUR_USD, PriceSpec::at(108_100_000).conf(13_893));
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::ExecuteTriggerOrder {
                keeper: keeper.pubkey(),
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position,
                trigger_order: Env::trigger_pda(&position, order_id),
                collateral_vault: self.env.collateral_vault,
                lp_pool: self.env.lp_pool,
                lp_vault: self.env.lp_vault,
                insurance_fund: self.env.insurance_fund,
                insurance_vault: self.env.insurance_vault,
                fee_vault: self.env.fee_vault,
                price_update: hit,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::ExecuteTriggerOrder {}.data(),
        };
        self.env.send(ix, &[&keeper]).expect("the stop fires");
    }

    /// Reconcile, supplying every open slot's position in slot order — what the instruction
    /// requires, so nothing can be omitted and nothing passed in another's place.
    fn reconcile(&mut self) -> TestResult {
        let caller = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&caller.pubkey(), 1_000_000_000)
            .unwrap();
        let user_account = Env::user_pda(&self.signer);
        let mut metas = noxfunds::accounts::ReconcilePosition {
            caller: caller.pubkey(),
            mandate: self.mandate,
            user_account,
            trader_profile: profile_pda(&self.trader.pubkey()),
        }
        .to_account_metas(None);
        for slot in self.mandate().slots.iter().filter(|s| s.open) {
            metas.push(AccountMeta::new_readonly(
                Env::position_pda(&user_account, slot.market_index, slot.nonce),
                false,
            ));
        }
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: metas,
            data: noxfunds::instruction::ReconcilePosition {}.data(),
        };
        self.env.send(ix, &[&caller])
    }

    /// A stranger closes a position. Nobody related to the mandate signs.
    fn wind_down(&mut self, nonce: u8) -> TestResult {
        let closer = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&closer.pubkey(), 1_000_000_000)
            .unwrap();
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::WindDownPosition {
                closer: closer.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position: Env::position_pda(&user_account, 0, nonce),
                trader_profile: profile_pda(&self.trader.pubkey()),
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
            data: noxfunds::instruction::WindDownPosition {}.data(),
        };
        self.env.send(ix, &[&closer])
    }

    /// A stranger cancels a resting stop, returning its rent to the mandate.
    fn wind_down_cancel(&mut self, nonce: u8, order_id: u8) -> TestResult {
        let closer = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&closer.pubkey(), 1_000_000_000)
            .unwrap();
        let position = Env::position_pda(&Env::user_pda(&self.signer), 0, nonce);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::WindDownCancelStop {
                closer: closer.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                trigger_order: Env::trigger_pda(&position, order_id),
                position,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::WindDownCancelStop {}.data(),
        };
        self.env.send(ix, &[&closer])
    }
}

// --- the exit criterion ---------------------------------------------------------------------

/// **A breached mandate reaches the investor's wallet with neither the trader nor the operator
/// doing anything.**
///
/// The trader signs once, to open. After that every step — detecting the breach, closing the
/// position, cancelling the stop, paying out — is a different stranger's transaction.
#[test]
fn a_breached_mandate_pays_out_without_the_trader_or_the_operator() {
    let mut nox = small_mandate();
    let payees = nox.payees();
    let stop = SPOT - (SPOT * 30) / 10_000;

    nox.open(0, ONE_LOT / 100, stop).expect("the trader opens");
    // …and is never heard from again.

    nox.observe(&[0]).expect("a stranger sets the peak");
    nox.observe_at(adverse(), &[0])
        .expect("a stranger observes the fall");
    assert_eq!(nox.mandate().state, MandateState::Breached);

    nox.wind_down(0).expect("a stranger closes the position");
    nox.wind_down_cancel(0, 0)
        .expect("a stranger reclaims the stop's rent");
    assert_eq!(nox.mandate().open_positions, 0);
    assert_eq!(nox.mandate().open_notional, 0);

    nox.claim(&payees).expect("a stranger settles");
    assert!(
        nox.env.token_balance(&payees.investor) > 0,
        "the investor is paid without anyone's cooperation"
    );
    assert_eq!(nox.mandate().state, MandateState::Settled);
    nox.env.assert_invariants();
}

/// An `Active` mandate is nobody else's business. The block half.
#[test]
fn a_stranger_cannot_wind_down_a_healthy_mandate() {
    let mut nox = small_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    let err = nox
        .wind_down(0)
        .expect_err("an active mandate may not be closed by a stranger");
    assert!(
        format!("{err:?}").contains("MandateNotWindingDown"),
        "{err:?}"
    );
    let err = nox
        .wind_down_cancel(0, 0)
        .expect_err("nor may its stops be cancelled");
    assert!(
        format!("{err:?}").contains("MandateNotWindingDown"),
        "{err:?}"
    );

    // The permit half: once it breaches, both are allowed.
    nox.observe(&[0]).expect("peak");
    nox.observe_at(adverse(), &[0]).expect("breach");
    nox.wind_down(0).expect("now a stranger may close it");
    nox.env.assert_invariants();
}

// --- the frozen-mandate bug -------------------------------------------------------------

/// **A stop-out used to freeze a mandate forever.**
///
/// The stop fires inside `solfx-core` and never enters NOXFUNDS, so the mandate went on counting
/// a position that no longer existed. Settlement requires that count at zero, and the equity
/// crank requires one triple per counted position — which cannot be supplied for an account that
/// is gone. Both were permanently impossible until `reconcile_position`.
#[test]
fn a_stopped_out_mandate_can_be_reconciled_and_then_settles() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    nox.fire_stop(0, 0);
    let user_account = Env::user_pda(&nox.signer);
    assert!(
        nox.env
            .svm
            .get_account(&Env::position_pda(&user_account, 0, 0))
            .is_none_or(|a| a.data.is_empty()),
        "solfx-core closed the position"
    );

    // The mandate has not noticed, and both ways out are blocked.
    assert_eq!(nox.mandate().open_positions, 1, "the stale count");
    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("request");
    let err = nox.claim(&payees).expect_err("settlement is blocked");
    assert!(format!("{err:?}").contains("PositionsStillOpen"), "{err:?}");
    assert!(
        nox.observe(&[0]).is_err(),
        "and the crank cannot supply an account that no longer exists"
    );

    // One permissionless call unsticks it.
    nox.reconcile().expect("anyone may reconcile");
    assert_eq!(nox.mandate().open_positions, 0);
    assert_eq!(nox.mandate().open_notional, 0, "and the book is clean");

    nox.observe(&[]).expect("the crank works again");
    nox.claim(&payees).expect("and it settles");
    assert!(nox.env.token_balance(&payees.investor) > 0);
    nox.env.assert_invariants();
}

/// Reconciliation cannot be used to hide a live position from the rules.
#[test]
fn a_live_position_cannot_be_reconciled_away() {
    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    let err = nox.reconcile().expect_err("the position is still there");
    assert!(format!("{err:?}").contains("PositionStillOpen"), "{err:?}");
    assert_eq!(nox.mandate().open_positions, 1);
    nox.env.assert_invariants();
}

/// Reusing a nonce whose slot was never reconciled would strand the stale one forever, so the
/// open is refused with a message that says what to do.
#[test]
fn reopening_an_unreconciled_nonce_is_refused() {
    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.fire_stop(0, 0);

    let err = nox
        .open(0, ONE_LOT / 100, stop)
        .expect_err("nonce 0's slot is still held by the stopped-out position");
    assert!(format!("{err:?}").contains("SlotNotReconciled"), "{err:?}");

    nox.reconcile().expect("reconcile");
    nox.open(0, ONE_LOT / 100, stop)
        .expect("and the nonce is usable again");
    nox.env.assert_invariants();
}

// --- the two bookkeeping bugs -------------------------------------------------------------

/// **The book returns to exactly zero**, on both sides of the market.
///
/// Opens book the oracle-priced notional the rules were judged against; an earlier version
/// unbooked core's `entry_notional`, which is priced at the fill after spread. A long unbooked
/// too much and clamped at zero, hiding it; a short unbooked too little and left a residue that
/// would eventually refuse trades the mandate had room for.
#[test]
fn a_round_trip_leaves_the_book_exactly_where_it_started() {
    let mut nox = roomy_mandate();
    let size = ONE_LOT / 100;

    for (nonce, direction) in [(0u8, Direction::Long), (1, Direction::Short)] {
        let stop = match direction {
            Direction::Long => SPOT - (SPOT * 30) / 10_000,
            Direction::Short => SPOT + (SPOT * 30) / 10_000,
        };
        nox.open_dir(nonce, size, stop, direction)
            .unwrap_or_else(|e| panic!("open {direction:?}: {e:?}"));
        assert!(nox.mandate().open_notional > 0);
        nox.close_dir(nonce, direction).expect("close");
        assert_eq!(
            nox.mandate().open_notional,
            0,
            "{direction:?} left a residue in the book"
        );
        assert_eq!(nox.mandate().open_positions, 0);
    }
    nox.env.assert_invariants();
}

/// **The crank cannot be fooled by the same position twice.**
///
/// It used to check only the number of triples, so a profitable position supplied twice
/// inflated equity and hid a breach from the one rule that exists to catch it.
#[test]
fn the_crank_refuses_a_duplicated_position() {
    // Two positions at $500 margin each, so the mandate needs more than roomy_mandate's $1,000.
    let mut nox = setup_funded(
        MandateRules {
            max_drawdown_bps: 1_000,
            max_daily_loss_bps: 1_000,
            max_risk_per_trade_bps: 100,
            ..base_rules()
        },
        5_000 * ONE_USDC,
        5_000 * ONE_USDC,
    );
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.open(1, ONE_LOT / 100, stop).expect("open");

    let err = nox
        .observe(&[0, 0])
        .expect_err("two triples, but the same position twice");
    assert!(
        format!("{err:?}").contains("IncompleteObservation"),
        "{err:?}"
    );

    let err = nox
        .observe(&[0])
        .expect_err("and omitting one is refused too");
    assert!(
        format!("{err:?}").contains("IncompleteObservation"),
        "{err:?}"
    );

    nox.observe(&[0, 1])
        .expect("each open position exactly once");
    nox.env.assert_invariants();
}

// --- internal review, 2026-09-23: what must be true, and now is ------------------------
//
// Each test below states a guarantee the design claims and that the program did not keep until
// this review. They were written first, failed on the code as it stood — that failure was the
// finding — and are now the regression tests for the fixes. None presupposes *how* the fix works.

impl Nox {
    fn cancel_order(&mut self, nonce: u8, order_id: u8) -> TestResult {
        let position = Env::position_pda(&Env::user_pda(&self.signer), 0, nonce);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedCancelStop {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                trigger_order: Env::trigger_pda(&position, order_id),
                position,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedCancelStop {
                market_index: 0,
                nonce,
                order_id,
            }
            .data(),
        };
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    /// Close the position at `position_nonce`, but tell NOXFUNDS it was `claimed_nonce`.
    fn close_claiming(&mut self, position_nonce: u8, claimed_nonce: u8) -> TestResult {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        let ix = Instruction {
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
                position: Env::position_pda(&user_account, 0, position_nonce),
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
                nonce: claimed_nonce,
                price_limit: 1,
            }
            .data(),
        };
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    fn profile(&self) -> noxfunds::state::TraderProfile {
        self.env.read(&profile_pda(&self.trader.pubkey()))
    }
}

/// R-4. **Every funded trade carries a stop** — not only at the moment it opens.
///
/// `funded_cancel_stop` checks nothing about the position, and `solfx-core` lets an authority
/// cancel its own order at any time. So the trader opens with a compliant stop, the risk rule
/// is judged against it, and then the stop is removed.
#[test]
fn the_mandatory_stop_cannot_be_cancelled_while_its_position_is_open() {
    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop)
        .expect("open with a compliant stop");
    let position = Env::position_pda(&Env::user_pda(&nox.signer), 0, 0);
    assert!(nox.env.trigger_exists(&position, 0));

    let r = nox.cancel_order(0, 0);
    assert!(
        r.is_err(),
        "the stop on a live position was cancelled — the position is now unprotected"
    );
    assert!(nox.env.trigger_exists(&position, 0), "and the stop is gone");
}

/// R-5. **A close releases the slot of the position it closed.**
///
/// The CPI closes whatever position account is passed; NOXFUNDS then unbooks whichever
/// `(market_index, nonce)` the trader *named*. Close A while naming B, reconcile A, and B is
/// open on SolFX with no slot — invisible to the drawdown crank, the book limits and wind-down.
#[test]
fn a_close_cannot_release_the_slot_of_a_different_position() {
    // Two positions at $500 margin each need more than the roomy mandate's $1,000 deposit.
    let mut nox = setup_funded(
        MandateRules {
            max_drawdown_bps: 1_000,
            max_daily_loss_bps: 1_000,
            ..base_rules()
        },
        2_000 * ONE_USDC,
        2_000 * ONE_USDC,
    );
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open A");
    nox.open(1, ONE_LOT / 100, stop).expect("open B");

    let r = nox.close_claiming(0, 1);
    if r.is_ok() {
        // Finish the attack so the damage is measured, not described.
        nox.reconcile()
            .expect("A's account is gone, so its slot reconciles");
        let user_account = Env::user_pda(&nox.signer);
        let b_live = nox
            .env
            .svm
            .get_account(&Env::position_pda(&user_account, 0, 1))
            .is_some_and(|a| !a.data.is_empty());
        panic!(
            "closed A while naming B: NOXFUNDS now counts {} open, while B still exists on \
             SolFX = {b_live}",
            nox.mandate().open_positions
        );
    }
}

/// R-3. **Losses reach the track record.**
///
/// A stop fires through `solfx-core` directly, so the only NOXFUNDS instruction that ever sees
/// the trade end is `reconcile_position` — which releases the slot and records nothing. The
/// same is true of `wind_down_position`. Since every funded trade carries a stop, the ordinary
/// way a losing trade ends is the one way it never reaches the record.
#[test]
fn a_stop_out_lands_on_the_record_as_a_loss() {
    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.fire_stop(0, 0);
    nox.reconcile().expect("reconcile the stopped-out slot");

    let p = nox.profile();
    assert_eq!(
        (p.trades, p.losses),
        (1, 1),
        "a trade was stopped out at a loss and the record shows trades={} losses={} \
         gross_loss={}",
        p.trades,
        p.losses,
        p.gross_loss
    );
    assert!(p.gross_loss > 0);
}

/// R-2. **Principal that has not been moved into SolFX is still equity.**
///
/// The crank counts SolFX free collateral plus open positions and never looks at the mandate's
/// own vault. Before the principal is deposited — or with only part of it deposited, which
/// anyone may do — equity reads near zero, drawdown near 100%, and the mandate is breached by a
/// stranger. `Breached` is terminal, and the 100% lands on the trader's permanent record.
#[test]
fn principal_still_in_the_vault_counts_toward_equity() {
    // $1,000 principal, one base unit moved into SolFX — the rest is sitting in the vault.
    let mut nox = setup_funded(base_rules(), 1_000 * ONE_USDC, 1);
    nox.observe(&[]).expect("a stranger runs the crank");

    let m = nox.mandate();
    let p = nox.profile();
    assert_eq!(
        m.state,
        MandateState::Active,
        "a mandate holding all of its principal was breached: equity read {} against a peak of \
         {}, and the trader's record now shows a {} bps drawdown",
        m.last_equity,
        m.peak_equity,
        p.max_drawdown_bps
    );
}

/// R-6. **The daily loss limit an investor sets is enforced.**
///
/// `max_daily_loss_bps` is validated when a mandate is created and never read again.
#[test]
fn a_mandate_past_its_daily_loss_limit_is_stopped() {
    let mut nox = setup_funded(
        MandateRules {
            max_drawdown_bps: 1_000, // 10%: far from binding
            max_daily_loss_bps: 100, // 1%: the rule under test
            max_risk_per_trade_bps: 100,
            ..base_rules()
        },
        1_000 * ONE_USDC,
        1_000 * ONE_USDC,
    );
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.observe(&[0]).expect("mark at the open");
    nox.observe_at(adverse(), &[0])
        .expect("mark after the fall");

    let m = nox.mandate();
    let lost_bps = (m.peak_equity - m.last_equity) * 10_000 / m.peak_equity;
    assert_ne!(
        m.state,
        MandateState::Active,
        "down {lost_bps} bps in one session against a 100 bps daily limit, and still Active"
    );
}

// --- the record, measured against the money ------------------------------------------------
//
// R-3's fix recovers what a stop-out made after the position account is gone. These pin the one
// property that makes that trustworthy: what the record says a trade made is what the mandate's
// SolFX balance actually moved by — to the unit, whichever way the trade ended.

impl Nox {
    fn free_collateral(&self) -> u64 {
        let ua: solfx_core::state::UserAccount = self.env.read(&Env::user_pda(&self.signer));
        ua.free_collateral
    }

    /// Fire any resting order straight into `solfx-core`, as a stranger's keeper would.
    fn fire_order(&mut self, nonce: u8, order_id: u8, at: PriceSpec) {
        let keeper = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&keeper.pubkey(), 100 * 1_000_000_000)
            .unwrap();
        let user_account = Env::user_pda(&self.signer);
        let position = Env::position_pda(&user_account, 0, nonce);
        let hit = self.env.post_price_now(FEED_EUR_USD, at);
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::ExecuteTriggerOrder {
                keeper: keeper.pubkey(),
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position,
                trigger_order: Env::trigger_pda(&position, order_id),
                collateral_vault: self.env.collateral_vault,
                lp_pool: self.env.lp_pool,
                lp_vault: self.env.lp_vault,
                insurance_fund: self.env.insurance_fund,
                insurance_vault: self.env.insurance_vault,
                fee_vault: self.env.fee_vault,
                price_update: hit,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: solfx_core::instruction::ExecuteTriggerOrder {}.data(),
        };
        self.env.send(ix, &[&keeper]).expect("the order fires");
    }

    fn take_profit(&mut self, nonce: u8, order_id: u8, target: i64, size_base: u64) {
        let p = self.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let user_account = Env::user_pda(&self.signer);
        let position = Env::position_pda(&user_account, 0, nonce);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedPlaceTakeProfit {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account,
                market: Env::market_pda(0),
                position,
                trigger_order: Env::trigger_pda(&position, order_id),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedPlaceTakeProfit {
                order_id,
                trigger_price: target,
                size_base,
            }
            .data(),
        };
        let trader = self.trader.insecure_clone();
        self.env
            .send(ix, &[&trader])
            .expect("place the take-profit");
    }
}

/// A $2,000 mandate with room for two positions at once.
fn two_position_mandate() -> Nox {
    setup_funded(
        MandateRules {
            max_drawdown_bps: 1_000,
            max_daily_loss_bps: 1_000,
            ..base_rules()
        },
        2_000 * ONE_USDC,
        2_000 * ONE_USDC,
    )
}

/// **A stop-out is recorded at exactly what it cost.** Not an estimate from the stop price: the
/// change in the mandate's SolFX balance from before the open to after the reconcile, which is
/// margin returned plus PnL less every fee.
#[test]
fn a_stop_out_is_recorded_at_exactly_what_the_balance_moved() {
    let mut nox = roomy_mandate();
    let before = nox.free_collateral();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.fire_stop(0, 0);
    nox.reconcile().expect("reconcile");

    let moved = i128::from(nox.free_collateral()) - i128::from(before);
    let p = nox.profile();
    assert!(moved < 0, "a stop-out loses money");
    assert_eq!(
        i128::from(p.gross_loss),
        -moved,
        "the record says {} was lost; the balance moved by {moved}",
        p.gross_loss
    );
    assert_eq!((p.trades, p.wins, p.losses), (1, 0, 1));
    assert_eq!(p.untimed_trades, 1, "no opening slot left to time it from");
    assert_eq!(p.ambiguous_trades, 0, "one close, so nothing to divide");
    assert_eq!(nox.mandate().realized_pnl, i64::try_from(moved).unwrap());
    assert_eq!(nox.mandate().booked_margin_fees, 0, "nothing left booked");
    nox.env.assert_invariants();
}

/// A forced close lands on the record at its measured result, like a voluntary one.
#[test]
fn a_wind_down_is_recorded_at_exactly_what_the_balance_moved() {
    let mut nox = small_mandate();
    let before = nox.free_collateral();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.observe(&[0]).expect("peak");
    nox.observe_at(adverse(), &[0]).expect("breach");
    assert_eq!(nox.mandate().state, MandateState::Breached);
    nox.wind_down(0).expect("a stranger closes it");

    let moved = i128::from(nox.free_collateral()) - i128::from(before);
    let p = nox.profile();
    let recorded = i128::from(p.gross_profit) - i128::from(p.gross_loss);
    assert_eq!(recorded, moved, "record {recorded} vs balance {moved}");
    assert_eq!(p.trades, 1);
    assert_eq!(
        p.untimed_trades, 0,
        "a wind-down still has its opening slot"
    );
    nox.env.assert_invariants();
}

/// **Two positions closing together cannot net a loss away.** A long's take-profit and a short's
/// stop firing on the same move is the hedged pair that would turn a loss into a smaller win if
/// the combined result were split. It is not split: no win is credited from an ambiguous batch,
/// a combined loss is recorded in full, and both trades are counted as ambiguous.
#[test]
fn a_hedged_pair_closing_together_is_never_credited_as_a_win() {
    let mut nox = two_position_mandate();
    let before = nox.free_collateral();

    // Long with a take-profit 50 bps up; short with its stop 30 bps up. One rise fires both.
    let long_stop = SPOT - (SPOT * 30) / 10_000;
    let short_stop = SPOT + (SPOT * 30) / 10_000;
    nox.open_dir(0, ONE_LOT / 100, long_stop, Direction::Long)
        .expect("long");
    nox.open_dir(1, ONE_LOT / 100, short_stop, Direction::Short)
        .expect("short");
    nox.take_profit(0, 1, SPOT + (SPOT * 50) / 10_000, ONE_LOT / 100);

    let rally = PriceSpec::at(109_200_000).conf(13_893); // 1.09200, through both
    nox.fire_order(0, 1, rally); // the long's take-profit: a win
    nox.fire_order(1, 1, rally); // the short's stop: a loss
    nox.reconcile().expect("reconcile both at once");

    let moved = i128::from(nox.free_collateral()) - i128::from(before);
    let p = nox.profile();
    assert_eq!(p.wins, 0, "an ambiguous batch never credits a win");
    assert_eq!((p.trades, p.losses), (2, 2));
    assert_eq!(p.ambiguous_trades, 2, "and it is published as ambiguous");
    if moved <= 0 {
        assert_eq!(
            i128::from(p.gross_loss),
            -moved,
            "a combined loss is recorded in full"
        );
    } else {
        assert_eq!(p.gross_profit, 0, "a combined profit is not credited");
    }
    // The mandate's money is real whatever the attribution.
    assert_eq!(i128::from(nox.mandate().realized_pnl), moved);
    nox.env.assert_invariants();
}

/// Reconciling one close while another position stays open attributes exactly the one that
/// closed — the open one's margin is read from its live account, not guessed.
#[test]
fn reconciling_one_close_leaves_the_open_position_booked() {
    let mut nox = two_position_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    let wide = SPOT - (SPOT * 150) / 10_000; // below the price that fires the first stop
    nox.open(0, ONE_LOT / 100, stop).expect("open A");
    let after_a = nox.free_collateral();
    nox.open(1, ONE_LOT / 100, wide).expect("open B");
    let b_cost = after_a - nox.free_collateral();

    nox.fire_stop(0, 0);
    nox.reconcile().expect("reconcile A only");

    let m = nox.mandate();
    assert_eq!(m.open_positions, 1, "B is still tracked");
    assert_eq!(
        m.booked_margin_fees, b_cost,
        "exactly B's margin and fee remain booked"
    );
    assert_eq!(nox.profile().ambiguous_trades, 0, "one close is exact");
    nox.observe(&[1])
        .expect("and the crank can mark what is left");
    nox.env.assert_invariants();
}

/// Review L-3. **A pause stops new risk; it does not trap a trader in a losing position.**
#[test]
fn a_trader_can_still_close_while_the_protocol_is_paused() {
    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    let admin = nox.env.admin.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::SetPaused {
            authority: admin.pubkey(),
            config: config_pda(),
        }
        .to_account_metas(None),
        data: noxfunds::instruction::SetPaused { paused: true }.data(),
    };
    nox.env.send(ix, &[&admin]).expect("pause");

    let err = nox
        .open(1, ONE_LOT / 100, stop)
        .expect_err("no new positions while paused");
    assert!(err.contains("ProtocolPaused"), "{err}");
    nox.close_dir(0, Direction::Long)
        .expect("but the open one can still be closed");
    assert_eq!(nox.mandate().open_positions, 0);
}
