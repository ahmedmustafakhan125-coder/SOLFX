//! Stage 5 — settlement: the 5% fee, the 70/30 split, and the investor's principal.
//!
//! Part 10's exit criterion: *N1–N4 hold under adversarial sequences; the investor always
//! recovers principal from a wound-down mandate.*
//!
//! N4 — every unit of the final balance goes to exactly one party — is proved across a grid in
//! `src/settlement.rs`. These tests prove the same thing with real tokens: a real trade, a real
//! withdrawal from SolFX, and three real transfers whose sum is checked against what came out.

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
            principal,
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

/// 250 bps above spot — a real profit for a long.
fn favourable() -> PriceSpec {
    PriceSpec::at(111_250_000).conf(13_893)
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

    #[allow(clippy::too_many_arguments)]
    fn open(&mut self, nonce: u8, size_base: u64, stop: i64) -> TestResult {
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
                direction: Direction::Long,
                size_base,
                collateral: 500 * ONE_USDC,
                price_limit: i64::MAX,
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

    fn close(&mut self, nonce: u8) -> TestResult {
        self.close_at(PriceSpec::default(), nonce)
    }

    fn close_at(&mut self, spec: PriceSpec, nonce: u8) -> TestResult {
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
                price_limit: 1,
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

/// **A profitable mandate, settled by a stranger.** Principal comes back first, the protocol
/// takes 5% of gross, and the rest divides 70/30 — with every unit accounted for.
#[test]
fn a_profitable_mandate_settles_by_the_plan() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    let stop = SPOT - (SPOT * 30) / 10_000;

    // ~$1,950 long, closed 250 bps higher.
    nox.open(0, ONE_LOT * 18 / 1_000, stop).expect("open");
    nox.close_at(favourable(), 0).expect("close in profit");

    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("the investor ends it");
    nox.claim(&payees).expect("anyone settles a flat mandate");

    let investor = nox.env.token_balance(&payees.investor);
    let trader = nox.env.token_balance(&payees.trader);
    let treasury = nox.env.token_balance(&payees.treasury);
    let principal = 1_000 * ONE_USDC;
    let total = investor + trader + treasury;

    assert!(total > principal, "the trade must have made money: {total}");
    assert_eq!(
        nox.env.token_balance(&nox.vault),
        0,
        "the vault is emptied — nothing is left behind"
    );

    // Recompute the split from what actually came out, and demand the chain agrees exactly.
    let expected = noxfunds::settlement::split(principal, total, 500, 7_000).unwrap();
    assert_eq!(treasury, expected.protocol, "5% of gross, rounded up");
    assert_eq!(trader, expected.trader, "70% of net, rounded down");
    assert_eq!(investor, expected.investor, "principal plus the remainder");
    assert!(
        investor >= principal,
        "the investor recovers their principal"
    );
    assert!(trader > 0 && treasury > 0);

    let m: noxfunds::state::Mandate = nox.env.read(&nox.mandate);
    assert_eq!(m.state, MandateState::Settled);
    nox.env.assert_invariants();
}

/// **A losing mandate.** No profit, so no fee and no trader share — the investor receives
/// everything that is left, and the protocol earns nothing from a loss.
#[test]
fn a_losing_mandate_returns_everything_to_the_investor() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    let stop = SPOT - (SPOT * 30) / 10_000;

    nox.open(0, ONE_LOT * 18 / 1_000, stop).expect("open");
    nox.close_at(adverse(), 0).expect("close at a loss");

    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("request");
    nox.claim(&payees).expect("settle");

    let investor = nox.env.token_balance(&payees.investor);
    assert!(
        investor > 0 && investor < 1_000 * ONE_USDC,
        "a real loss: {investor}"
    );
    assert_eq!(
        nox.env.token_balance(&payees.trader),
        0,
        "no profit, no trader share"
    );
    assert_eq!(
        nox.env.token_balance(&payees.treasury),
        0,
        "the protocol never earns from a loss"
    );
    assert_eq!(nox.env.token_balance(&nox.vault), 0);
    nox.env.assert_invariants();
}

/// A breached mandate settles without the investor asking — the breach already stopped it.
#[test]
fn a_breached_mandate_settles_without_a_request() {
    let mut nox = small_mandate();
    let payees = nox.payees();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");
    nox.observe(&[0]).expect("peak");
    nox.observe_at(adverse(), &[0]).expect("breach");
    assert_eq!(nox.mandate().state, MandateState::Breached);

    nox.close_at(adverse(), 0)
        .expect("the position still closes");
    nox.claim(&payees)
        .expect("a breached, flat mandate settles");

    assert!(nox.env.token_balance(&payees.investor) > 0);
    assert_eq!(nox.mandate().state, MandateState::Settled);
    nox.env.assert_invariants();
}

// --- the refusals ------------------------------------------------------------------------

/// Only the investor can end a mandate. A trader cannot walk off with a wind-down.
#[test]
fn only_the_investor_may_request_settlement() {
    let mut nox = roomy_mandate();
    let trader = nox.trader.insecure_clone();
    let err = nox
        .request_settlement(&trader)
        .expect_err("the trader is not the investor");
    assert!(format!("{err:?}").contains("NotTheInvestor"), "{err:?}");

    // The permit half.
    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("the investor may");
    assert_eq!(nox.mandate().state, MandateState::WindingDown);
}

/// A winding-down mandate takes no new trades, but may still close the ones it has.
#[test]
fn winding_down_stops_new_trades_but_not_closes() {
    let mut nox = roomy_mandate();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("request");

    let err = nox
        .open(1, ONE_LOT / 100, stop)
        .expect_err("no new trades after a wind-down request");
    assert!(format!("{err:?}").contains("MandateNotActive"), "{err:?}");
    nox.close(0).expect("but the open one can still be closed");
    nox.env.assert_invariants();
}

/// Settling with a position open would pay out free collateral and leave the position's margin
/// and PnL behind. Refused.
#[test]
fn a_mandate_with_open_positions_cannot_settle() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    let stop = SPOT - (SPOT * 30) / 10_000;
    nox.open(0, ONE_LOT / 100, stop).expect("open");

    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("request");
    let err = nox.claim(&payees).expect_err("a position is still open");
    assert!(format!("{err:?}").contains("PositionsStillOpen"), "{err:?}");

    nox.close(0).expect("close");
    nox.claim(&payees).expect("now it settles");
    nox.env.assert_invariants();
}

/// An `Active` mandate cannot be settled out from under a trader.
#[test]
fn an_active_mandate_cannot_be_settled() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    let err = nox.claim(&payees).expect_err("nobody asked for this");
    assert!(
        format!("{err:?}").contains("MandateNotSettleable"),
        "{err:?}"
    );
}

/// Settlement happens once. A second claim must not pay anyone again.
#[test]
fn a_settled_mandate_cannot_be_settled_twice() {
    let mut nox = roomy_mandate();
    let payees = nox.payees();
    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("request");
    nox.claim(&payees).expect("first settlement");
    let paid = nox.env.token_balance(&payees.investor);

    let err = nox.claim(&payees).expect_err("already settled");
    assert!(
        format!("{err:?}").contains("MandateNotSettleable"),
        "{err:?}"
    );
    assert_eq!(
        nox.env.token_balance(&payees.investor),
        paid,
        "nobody is paid twice"
    );
}

/// A permissionless settler must not be able to choose where the principal goes.
#[test]
fn the_settler_cannot_redirect_the_investors_money() {
    let mut nox = roomy_mandate();
    let mut payees = nox.payees();
    let inv = nox.investor.insecure_clone();
    nox.request_settlement(&inv).expect("request");

    // Swap in an account the attacker owns.
    let thief = Pubkey::new_unique();
    let thief_account = Pubkey::new_unique();
    let mint = nox.env.usdc_mint;
    nox.env.write_token_account(thief_account, mint, thief, 0);
    payees.investor = thief_account;

    let err = nox
        .claim(&payees)
        .expect_err("the investor account is constrained");
    assert!(
        format!("{err:?}").contains("ConstraintTokenOwner") || format!("{err:?}").contains("2015"),
        "refused by the token-authority constraint: {err:?}"
    );
    assert_eq!(nox.env.token_balance(&thief_account), 0);
}
