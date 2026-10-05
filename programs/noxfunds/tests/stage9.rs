//! Orders on a funded mandate: a stop that tightens, a take-profit that waits for the hold, and
//! resting entries filled under every rule of the mandate.
//!
//! The tests worth reading first close loopholes: a stop is replaced, never removed, so the
//! position is protected at every instant; a target and a moved stop both wait for
//! `min_hold_slots`; a limit cannot fill worse than its price, because `solfx-core`'s own
//! slippage check holds the bound; and an order that triggers is judged by `check_rules`.

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
use noxfunds::instructions::MandateEntryParams;
use noxfunds::state::{EntryKind, MandateEntryOrder, MandateState};
use solana_signer::Signer;
use solfx_core::state::{Direction, TriggerKind, TriggerOrder};

const MARKET_0: u128 = 1;
/// EUR/USD mid as a Pyth mantissa (exponent −8), one pip at that exponent, and both at
/// `PRICE_PRECISION` — see `stage3.rs` on why both unit systems appear.
const MID: i64 = 108_543_000;
const PIP: i64 = 10_000;
const SPOT: i64 = 1_085_430_000;
const PPIP: i64 = 100_000;
/// 1,000 units of EUR: about $1,085 notional.
const SIZE: u64 = ONE_LOT / 100;
const HOLD: u64 = 50;

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
fn signer_pda(mandate: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::MANDATE_SIGNER_SEED, mandate.as_ref()],
        &noxfunds::ID,
    )
    .0
}
fn order_pda(mandate: &Pubkey, order_id: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::MANDATE_ORDER_SEED,
            mandate.as_ref(),
            &[order_id],
        ],
        &noxfunds::ID,
    )
    .0
}

fn refused(r: TestResult, code: &str) {
    let err = r.expect_err("expected a refusal");
    assert!(err.contains(code), "refused, but not for {code}:\n{err}");
}

fn rules(min_hold_slots: u64) -> MandateRules {
    MandateRules {
        max_trade_notional: 2_000 * ONE_USDC,
        max_total_notional: 6_000 * ONE_USDC,
        max_drawdown_bps: 1_000,
        max_daily_loss_bps: 1_000,
        max_risk_per_trade_bps: 100,
        max_stop_distance_bps: 200,
        max_concurrent_positions: 5,
        allowed_markets: MARKET_0,
        min_hold_slots,
    }
}

struct Nox {
    env: Env,
    investor: solana_keypair::Keypair,
    trader: solana_keypair::Keypair,
    mandate: Pubkey,
    signer: Pubkey,
}

/// A $1,000 mandate, fully posted to SolFX, with the given minimum hold.
fn setup(min_hold_slots: u64) -> Nox {
    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
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

    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeTraderProfile {
            payer: admin.pubkey(),
            authority: trader.pubkey(),
            profile: profile_pda(&trader.pubkey()),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeTraderProfile {}.data(),
    };
    env.send(ix, &[&admin]).unwrap();

    let mandate = mandate_pda(&investor.pubkey(), &trader.pubkey(), 0);
    let signer = signer_pda(&mandate);
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
            principal: 1_000 * ONE_USDC,
            rules: rules(min_hold_slots),
        }
        .data(),
    };
    let inv = investor.insecure_clone();
    env.send(ix, &[&inv, &trader]).unwrap();

    // Post the principal to SolFX, as `fund_for_trading` does in `stage6.rs`.
    env.svm.airdrop(&signer, 1_000_000_000).unwrap();
    let user_account = Env::user_pda(&signer);
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::CreateSolfxAccount {
            payer: inv.pubkey(),
            config: config_pda(),
            mandate,
            mandate_signer: signer,
            protocol: env.protocol,
            user_account,
            system_program: anchor_lang::system_program::ID,
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::CreateSolfxAccount {}.data(),
    };
    env.send(ix, &[&inv]).unwrap();
    env.track_user(user_account);
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundSolfxCollateral {
            payer: inv.pubkey(),
            config: config_pda(),
            mandate,
            mandate_signer: signer,
            protocol: env.protocol,
            user_account,
            collateral_mint: env.usdc_mint,
            collateral_vault: env.collateral_vault,
            mandate_vault: support::vault_pda(&mandate),
            token_program: spl_token::ID,
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundSolfxCollateral {
            amount: 1_000 * ONE_USDC,
        }
        .data(),
    };
    env.send(ix, &[&inv]).unwrap();

    Nox {
        env,
        investor,
        trader,
        mandate,
        signer,
    }
}

impl Nox {
    fn user_account(&self) -> Pubkey {
        Env::user_pda(&self.signer)
    }
    fn position(&self, nonce: u8) -> Pubkey {
        Env::position_pda(&self.user_account(), 0, nonce)
    }
    fn trigger(&self, nonce: u8, order_id: u8) -> Pubkey {
        Env::trigger_pda(&self.position(nonce), order_id)
    }
    fn mandate(&self) -> noxfunds::state::Mandate {
        self.env.read(&self.mandate)
    }
    fn exists(&self, key: &Pubkey) -> bool {
        self.env.sol_balance(key) > 0
    }
    fn price(&mut self, mantissa: i64) -> Pubkey {
        self.env
            .post_price_now(FEED_EUR_USD, PriceSpec::at(mantissa))
    }
    fn as_trader(&mut self, ix: Instruction) -> TestResult {
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }
    fn stranger(&mut self) -> solana_keypair::Keypair {
        let k = solana_keypair::Keypair::new();
        self.env.svm.airdrop(&k.pubkey(), 1_000_000_000).unwrap();
        k
    }

    /// A long at the mid with its stop at `order_id = nonce`, as `funded_open_position` places it.
    fn open(&mut self, nonce: u8, stop: i64) -> TestResult {
        let p = self.price(MID);
        let position = self.position(nonce);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedOpenPosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: self.user_account(),
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
                size_base: SIZE,
                collateral: 300 * ONE_USDC,
                price_limit: i64::MAX,
                order_id: nonce,
                stop_loss_price: stop,
            }
            .data(),
        };
        let r = self.as_trader(ix);
        if r.is_ok() {
            self.env.track_position(position);
        }
        r
    }

    fn take_profit(&mut self, nonce: u8, order_id: u8, target: i64) -> TestResult {
        let p = self.price(MID);
        let position = self.position(nonce);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedPlaceTakeProfit {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: self.user_account(),
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
                size_base: SIZE,
            }
            .data(),
        };
        self.as_trader(ix)
    }

    fn move_stop_ix(&mut self, nonce: u8, old_id: u8, new_id: u8, stop: i64) -> Instruction {
        let p = self.price(MID);
        let position = self.position(nonce);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedMoveStop {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: self.user_account(),
                market: Env::market_pda(0),
                position,
                old_stop: Env::trigger_pda(&position, old_id),
                new_stop: Env::trigger_pda(&position, new_id),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedMoveStop {
                new_order_id: new_id,
                new_stop: stop,
            }
            .data(),
        }
    }

    fn move_stop(&mut self, nonce: u8, old_id: u8, new_id: u8, stop: i64) -> TestResult {
        let ix = self.move_stop_ix(nonce, old_id, new_id, stop);
        self.as_trader(ix)
    }

    /// Fire a resting order straight into `solfx-core`, as a stranger's keeper would.
    fn fire(&mut self, nonce: u8, order_id: u8, mantissa: i64) -> TestResult {
        let keeper = self.stranger();
        let position = self.position(nonce);
        let p = self.price(mantissa);
        let ix = Instruction {
            program_id: solfx_core::ID,
            accounts: solfx_core::accounts::ExecuteTriggerOrder {
                keeper: keeper.pubkey(),
                protocol: self.env.protocol,
                user_account: self.user_account(),
                market: Env::market_pda(0),
                position,
                trigger_order: Env::trigger_pda(&position, order_id),
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
            }
            .to_account_metas(None),
            data: solfx_core::instruction::ExecuteTriggerOrder {}.data(),
        };
        self.env.send(ix, &[&keeper])
    }

    fn place(&mut self, order_id: u8, params: MandateEntryParams) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedPlaceEntryOrder {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                entry_order: order_pda(&self.mandate, order_id),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedPlaceEntryOrder { order_id, params }.data(),
        };
        self.as_trader(ix)
    }

    fn fill_ix(
        &mut self,
        keeper: &Pubkey,
        order_id: u8,
        o: &MandateEntryParams,
        mantissa: i64,
    ) -> Instruction {
        let p = self.price(mantissa);
        let position = self.position(o.nonce);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedFillEntryOrder {
                keeper: *keeper,
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                entry_order: order_pda(&self.mandate, order_id),
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: self.user_account(),
                market: Env::market_pda(0),
                position,
                trigger_order: Env::trigger_pda(&position, o.stop_order_id),
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
            data: noxfunds::instruction::FundedFillEntryOrder {}.data(),
        }
    }

    fn fill(&mut self, order_id: u8, o: &MandateEntryParams, mantissa: i64) -> TestResult {
        let keeper = self.stranger();
        let ix = self.fill_ix(&keeper.pubkey(), order_id, o, mantissa);
        let r = self.env.send(ix, &[&keeper]);
        if r.is_ok() {
            let position = self.position(o.nonce);
            self.env.track_position(position);
        }
        r
    }

    fn cancel(&mut self, caller: &solana_keypair::Keypair, order_id: u8) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedCancelEntryOrder {
                caller: caller.pubkey(),
                trader: self.trader.pubkey(),
                mandate: self.mandate,
                entry_order: order_pda(&self.mandate, order_id),
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedCancelEntryOrder {}.data(),
        };
        let c = caller.insecure_clone();
        self.env.send(ix, &[&c])
    }
}

/// A buy limit 5 pips under the mid, its stop 20 pips under that, bounded at its own price.
fn buy_limit(nonce: u8) -> MandateEntryParams {
    let trigger = SPOT - 5 * PPIP;
    MandateEntryParams {
        market_index: 0,
        nonce,
        stop_order_id: 0,
        direction: Direction::Long,
        kind: EntryKind::Limit,
        trigger_price: trigger,
        price_limit: trigger,
        size_base: SIZE,
        collateral: 300 * ONE_USDC,
        stop_loss_price: trigger - 20 * PPIP,
        expires_at: 0,
    }
}

// --- moving the stop ---------------------------------------------------------------------------

/// **The new stop lands before the old one goes.** Tighter only, and only after the hold.
#[test]
fn a_funded_stop_tightens_after_the_hold_by_replacement_not_removal() {
    let mut nox = setup(HOLD);
    nox.open(0, SPOT - 30 * PPIP).unwrap();

    refused(
        nox.move_stop(0, 0, 1, SPOT - 10 * PPIP),
        "MinimumHoldNotMet",
    );
    nox.env.advance_slot(HOLD);

    refused(nox.move_stop(0, 0, 1, SPOT - 40 * PPIP), "StopNotTighter");
    refused(nox.move_stop(0, 0, 1, SPOT - 30 * PPIP), "StopNotTighter");
    // Already met at the price: `solfx-core` refuses it, so the old stop is never cancelled.
    refused(nox.move_stop(0, 0, 1, SPOT + PPIP), "TriggerAlreadyMet");
    assert!(
        nox.exists(&nox.trigger(0, 0)),
        "the original stop is untouched"
    );

    let signer_before = nox.env.sol_balance(&nox.signer);
    nox.move_stop(0, 0, 1, SPOT - 10 * PPIP)
        .expect("tighter, after the hold");
    assert!(!nox.exists(&nox.trigger(0, 0)), "the old stop is gone");
    let new: TriggerOrder = nox.env.read(&nox.trigger(0, 1));
    assert_eq!(new.kind, TriggerKind::StopLoss);
    assert_eq!(new.trigger_price, SPOT - 10 * PPIP);
    assert_eq!(new.size_base, SIZE);
    // The signer paid the new stop's rent and was refunded the old one's: the same size.
    assert_eq!(nox.env.sol_balance(&nox.signer), signer_before);

    // The moved stop is the one that fires.
    nox.fire(0, 1, MID - 12 * PIP)
        .expect("the moved stop fires");
    assert!(!nox.exists(&nox.position(0)));
    nox.env.assert_invariants();
}

#[test]
fn only_a_stop_on_this_position_can_be_replaced() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    nox.take_profit(0, 1, SPOT + 30 * PPIP).unwrap();
    refused(nox.move_stop(0, 1, 2, SPOT - 10 * PPIP), "NotTheStopLoss");
}

#[test]
fn only_the_trader_moves_a_stop() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    let stranger = nox.stranger();
    let mut ix = nox.move_stop_ix(0, 0, 1, SPOT - 10 * PPIP);
    ix.accounts[0].pubkey = stranger.pubkey();
    refused(nox.env.send(ix, &[&stranger]), "NotTheTrader");
}

// --- the take-profit waits for the hold ------------------------------------------------------

/// **The loophole this closes.** A target fires inside `solfx-core`, which never reads the
/// mandate's hold rule; placed at open, one tick away, it was a profitable exit in the first
/// slot. Now it cannot be placed until the hold has passed.
#[test]
fn a_funded_take_profit_cannot_be_placed_inside_the_hold() {
    let mut nox = setup(HOLD);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    refused(nox.take_profit(0, 1, SPOT + PPIP), "MinimumHoldNotMet");
    nox.env.advance_slot(HOLD);
    nox.take_profit(0, 1, SPOT + 30 * PPIP)
        .expect("after the hold");
}

#[test]
fn with_no_hold_a_take_profit_is_placed_at_once_as_before() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    nox.take_profit(0, 1, SPOT + 30 * PPIP)
        .expect("min_hold_slots = 0 changes nothing");
}

// --- resting entry orders ----------------------------------------------------------------------

#[test]
fn a_funded_limit_fills_only_at_its_price_or_better_with_its_stop() {
    let mut nox = setup(0);
    let o = buy_limit(0);
    nox.place(0, o).expect("place");

    refused(nox.fill(0, &o, MID), "EntryNotTriggered");
    // The oracle is at the limit, but a buy executes above the oracle: `solfx-core`'s own
    // slippage check holds the bound the order carries.
    refused(nox.fill(0, &o, MID - 5 * PIP), "SlippageExceeded");

    let trader_before = nox.env.sol_balance(&nox.trader.pubkey());
    nox.fill(0, &o, MID - 15 * PIP)
        .expect("fills under the limit");

    let pos: solfx_core::state::Position = nox.env.read(&nox.position(0));
    assert!(
        pos.entry_price <= o.trigger_price,
        "never worse than the limit"
    );
    assert_eq!(pos.size_base, SIZE);
    let stop: TriggerOrder = nox.env.read(&nox.trigger(0, o.stop_order_id));
    assert_eq!(stop.kind, TriggerKind::StopLoss);
    assert_eq!(stop.trigger_price, o.stop_loss_price);

    let m = nox.mandate();
    assert_eq!(m.open_positions, 1, "booked on the mandate like any open");
    assert!(m.booked_margin_fees > 0);
    assert!(
        !nox.exists(&order_pda(&nox.mandate, 0)),
        "the order is closed"
    );
    assert!(
        nox.env.sol_balance(&nox.trader.pubkey()) > trader_before,
        "its rent went back to the trader"
    );
    nox.env.assert_invariants();
}

#[test]
fn a_funded_order_that_breaks_a_rule_at_the_fill_does_not_fill() {
    let mut nox = setup(0);
    // $2,700 notional against a $2,000 per-trade ceiling. Placement cannot price it; the fill can.
    let mut o = buy_limit(0);
    o.size_base = ONE_LOT / 40;
    nox.place(0, o).unwrap();
    refused(nox.fill(0, &o, MID - 15 * PIP), "TradeExceedsMandate");
    assert!(nox.exists(&order_pda(&nox.mandate, 0)), "it waits");
    assert_eq!(nox.mandate().open_positions, 0);
}

#[test]
fn funded_placement_refuses_what_could_never_fill() {
    let mut nox = setup(0);
    let base = buy_limit(0);

    let mut market = base;
    market.market_index = 1;
    refused(nox.place(0, market), "MarketNotPermitted");

    let mut far = base;
    far.stop_loss_price = base.trigger_price - (base.trigger_price * 300) / 10_000;
    refused(nox.place(0, far), "StopTooFar");

    let mut wrong_side = base;
    wrong_side.stop_loss_price = base.trigger_price + PPIP;
    refused(nox.place(0, wrong_side), "StopOnWrongSide");

    // A buy limit bounded above its own price would fill worse than its limit.
    let mut loose = base;
    loose.price_limit = base.trigger_price + PPIP;
    refused(nox.place(0, loose), "InvalidPriceLimit");

    let mut past = base;
    past.expires_at = nox.env.now - 1;
    refused(nox.place(0, past), "InvalidExpiry");
}

#[test]
fn a_funded_buy_stop_fills_on_the_breakout_within_its_bound() {
    let mut nox = setup(0);
    let trigger = SPOT + 5 * PPIP;
    let o = MandateEntryParams {
        kind: EntryKind::Stop,
        trigger_price: trigger,
        price_limit: trigger + 20 * PPIP,
        stop_loss_price: trigger - 20 * PPIP,
        ..buy_limit(0)
    };
    nox.place(0, o).unwrap();
    refused(nox.fill(0, &o, MID + 4 * PIP), "EntryNotTriggered");
    nox.fill(0, &o, MID + 6 * PIP).expect("on the breakout");
    nox.env.assert_invariants();
}

#[test]
fn a_funded_order_dies_with_its_mandate_and_anyone_may_clear_it() {
    let mut nox = setup(0);
    let o = buy_limit(0);
    nox.place(0, o).unwrap();
    let stranger = nox.stranger();
    refused(nox.cancel(&stranger, 0), "NotYourOrderToCancel");

    let investor = nox.investor.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::RequestSettlement {
            investor: investor.pubkey(),
            mandate: nox.mandate,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::RequestSettlement {}.data(),
    };
    nox.env.send(ix, &[&investor]).unwrap();
    assert_eq!(nox.mandate().state, MandateState::WindingDown);

    refused(nox.fill(0, &o, MID - 15 * PIP), "MandateNotActive");
    nox.cancel(&stranger, 0)
        .expect("anyone, once it cannot fill");
}

#[test]
fn an_expired_funded_order_cannot_fill() {
    let mut nox = setup(0);
    let mut o = buy_limit(0);
    o.expires_at = nox.env.now + 100;
    nox.place(0, o).unwrap();
    nox.env.advance_clock(200);
    refused(nox.fill(0, &o, MID - 15 * PIP), "EntryOrderExpired");
    let stranger = nox.stranger();
    nox.cancel(&stranger, 0).expect("expired, so anyone");
    assert!(nox_read_order(&nox, 0).is_none());
}

fn nox_read_order(nox: &Nox, order_id: u8) -> Option<MandateEntryOrder> {
    let acct = nox
        .env
        .svm
        .get_account(&order_pda(&nox.mandate, order_id))?;
    if acct.owner != noxfunds::ID {
        return None;
    }
    anchor_lang::AccountDeserialize::try_deserialize(&mut acct.data.as_slice()).ok()
}

#[test]
fn the_trader_cancels_a_funded_order_any_time() {
    let mut nox = setup(0);
    nox.place(0, buy_limit(0)).unwrap();
    let trader = nox.trader.insecure_clone();
    nox.cancel(&trader, 0).unwrap();
    assert!(nox_read_order(&nox, 0).is_none());
}

#[test]
fn a_pause_stops_funded_orders_being_placed_or_filled() {
    let mut nox = setup(0);
    let o = buy_limit(0);
    nox.place(0, o).unwrap();
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
    nox.env.send(ix, &[&admin]).unwrap();
    refused(nox.place(1, buy_limit(1)), "ProtocolPaused");
    refused(nox.fill(0, &o, MID - 15 * PIP), "ProtocolPaused");
}

// --- partial closes ---------------------------------------------------------------------------

impl Nox {
    fn free(&self) -> u64 {
        let ua: solfx_core::state::UserAccount = self.env.read(&self.user_account());
        ua.free_collateral
    }
    fn profile(&self) -> noxfunds::state::TraderProfile {
        self.env.read(&profile_pda(&self.trader.pubkey()))
    }

    fn reduce_ix(&mut self, nonce: u8, size: u64, mantissa: i64, market_index: u16) -> Instruction {
        let p = self.price(mantissa);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedReducePosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: self.user_account(),
                market: Env::market_pda(0),
                position: self.position(nonce),
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
            data: noxfunds::instruction::FundedReducePosition {
                market_index,
                nonce,
                size_delta: size,
                // Closing a long sells, so its bound is a minimum.
                price_limit: 1,
            }
            .data(),
        }
    }

    fn reduce(&mut self, nonce: u8, size: u64, mantissa: i64) -> TestResult {
        let ix = self.reduce_ix(nonce, size, mantissa, 0);
        self.as_trader(ix)
    }

    fn close(&mut self, nonce: u8, mantissa: i64) -> TestResult {
        let p = self.price(mantissa);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedClosePosition {
                trader_profile: profile_pda(&self.trader.pubkey()),
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: self.user_account(),
                market: Env::market_pda(0),
                position: self.position(nonce),
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
        self.as_trader(ix)
    }

    fn reconcile(&mut self) -> TestResult {
        let caller = self.stranger();
        let user_account = self.user_account();
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
}

/// **The books still balance.** A partial close at a profit, then the rest closed: the trade is
/// counted once, and the mandate's recorded result is the measured change in its SolFX balance,
/// to the unit — the identity every close path in this program is held to.
#[test]
fn a_partial_then_a_close_counts_one_trade_and_balances_to_the_unit() {
    let mut nox = setup(HOLD);
    let start = nox.free();
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    let booked = nox.mandate().open_notional;

    refused(nox.reduce(0, SIZE / 2, MID + 20 * PIP), "MinimumHoldNotMet");
    nox.env.advance_slot(HOLD);
    nox.reduce(0, SIZE / 2, MID + 20 * PIP)
        .expect("half off at a profit");

    let p = nox.profile();
    assert_eq!((p.trades, p.wins, p.losses), (0, 0, 0), "not a trade yet");
    assert!(p.gross_profit > 0, "but the profit is on the record");
    let m = nox.mandate();
    assert_eq!(m.open_positions, 1, "the slot stays open");
    assert_eq!(
        m.open_notional,
        booked - booked / 2,
        "half released, rounded down"
    );
    assert!(m.realized_pnl > 0);
    let pos: solfx_core::state::Position = nox.env.read(&nox.position(0));
    assert_eq!(pos.size_base, SIZE - SIZE / 2);
    assert!(
        nox.exists(&nox.trigger(0, 0)),
        "the stop still guards the rest"
    );

    nox.close(0, MID + 10 * PIP).expect("the rest");
    let p = nox.profile();
    assert_eq!(p.trades, 1, "one trade, however many pieces");
    let m = nox.mandate();
    assert_eq!(m.open_positions, 0);
    assert_eq!(m.open_notional, 0);
    assert_eq!(m.booked_margin_fees, 0, "nothing left booked");
    let moved = i128::from(nox.free()) - i128::from(start);
    assert_eq!(i128::from(m.realized_pnl), moved, "recorded == measured");
    assert_eq!(
        i128::from(p.gross_profit) - i128::from(p.gross_loss),
        moved,
        "the record nets to the same figure"
    );
    nox.env.assert_invariants();
}

/// A partial, then a stop-out of the rest that NOXFUNDS only sees on reconcile. The partial
/// left the open fee booked with the survivor, which is exactly what reconcile reads for a live
/// position — so the stop-out is still recovered to the unit.
#[test]
fn a_partial_then_a_stop_out_still_reconciles_to_the_unit() {
    let mut nox = setup(0);
    let start = nox.free();
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    nox.reduce(0, SIZE / 4, MID - 5 * PIP)
        .expect("a quarter off at a small loss");
    nox.fire(0, 0, MID - 35 * PIP)
        .expect("the stop takes the rest");
    nox.reconcile().expect("reconcile");

    let m = nox.mandate();
    assert_eq!(m.open_positions, 0);
    assert_eq!(m.booked_margin_fees, 0);
    let moved = i128::from(nox.free()) - i128::from(start);
    assert_eq!(i128::from(m.realized_pnl), moved, "recorded == measured");
    let p = nox.profile();
    assert_eq!((p.trades, p.losses), (1, 1));
    assert_eq!(i128::from(p.gross_profit) - i128::from(p.gross_loss), moved);
    nox.env.assert_invariants();
}

#[test]
fn a_partial_must_be_partial_and_name_its_position() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    refused(nox.reduce(0, SIZE, MID), "PositionSizeOutOfBounds");
    refused(nox.reduce(0, 0, MID), "PositionSizeOutOfBounds");
    let ix = nox.reduce_ix(0, SIZE / 2, MID, 3);
    refused(nox.as_trader(ix), "PositionArgsMismatch");
}

// --- margin -------------------------------------------------------------------------------------

impl Nox {
    fn margin_ix(&mut self, nonce: u8, amount: u64, add: bool, market_index: u16) -> Instruction {
        let p = self.price(MID);
        let accounts = noxfunds::accounts::FundedAdjustMargin {
            trader: self.trader.pubkey(),
            config: config_pda(),
            mandate: self.mandate,
            mandate_signer: self.signer,
            protocol: self.env.protocol,
            user_account: self.user_account(),
            market: Env::market_pda(0),
            position: self.position(nonce),
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
        .to_account_metas(None);
        let data = if add {
            noxfunds::instruction::FundedAddMargin {
                market_index,
                nonce,
                amount,
            }
            .data()
        } else {
            noxfunds::instruction::FundedRemoveMargin {
                market_index,
                nonce,
                amount,
            }
            .data()
        };
        Instruction {
            program_id: noxfunds::ID,
            accounts,
            data,
        }
    }

    fn margin(&mut self, nonce: u8, amount: u64, add: bool) -> TestResult {
        let ix = self.margin_ix(nonce, amount, add, 0);
        self.as_trader(ix)
    }
}

/// **Margin moves are bookkeeping, not results.** Add some, take some back, let the stop take
/// the position: the record still equals the measured balance change to the unit, which is only
/// true if every move was booked exactly.
#[test]
fn margin_moves_keep_the_books_exact_through_a_stop_out() {
    let mut nox = setup(0);
    let start = nox.free();
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    let booked = nox.mandate().booked_margin_fees;

    nox.margin(0, 100 * ONE_USDC, true).expect("add margin");
    let pos: solfx_core::state::Position = nox.env.read(&nox.position(0));
    assert_eq!(pos.collateral, 400 * ONE_USDC);
    assert_eq!(nox.mandate().booked_margin_fees, booked + 100 * ONE_USDC);
    nox.margin(0, 50 * ONE_USDC, false).expect("remove some");
    assert_eq!(nox.mandate().booked_margin_fees, booked + 50 * ONE_USDC);

    nox.fire(0, 0, MID - 35 * PIP).expect("the stop fires");
    nox.reconcile().expect("reconcile");
    let m = nox.mandate();
    assert_eq!(m.booked_margin_fees, 0);
    let moved = i128::from(nox.free()) - i128::from(start);
    assert_eq!(i128::from(m.realized_pnl), moved, "recorded == measured");
    nox.env.assert_invariants();
}

#[test]
fn margin_cannot_be_drawn_below_what_the_position_needs() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    // $300 posted against ~$1,085 notional; all but one unit would leave it far over-levered.
    refused(
        nox.margin(0, 300 * ONE_USDC - 1, false),
        "WouldBreachMaintenanceMargin",
    );
    refused(nox.margin(0, 0, true), "ZeroAmount");
    let ix = nox.margin_ix(0, ONE_USDC, true, 3);
    refused(nox.as_trader(ix), "PositionArgsMismatch");
}

/// Adding margin lowers risk, so a stopped mandate may still do it; removing it may not.
#[test]
fn a_winding_down_mandate_may_add_margin_and_not_remove_it() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    let investor = nox.investor.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::RequestSettlement {
            investor: investor.pubkey(),
            mandate: nox.mandate,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::RequestSettlement {}.data(),
    };
    nox.env.send(ix, &[&investor]).unwrap();
    nox.margin(0, 10 * ONE_USDC, true)
        .expect("adding is always allowed");
    refused(nox.margin(0, 5 * ONE_USDC, false), "MandateNotActive");
}

// --- the budget ----------------------------------------------------------------------------------

/// The fill is the widest instruction NOXFUNDS has: the 21 accounts of `funded_open_position`
/// plus the order and the trader it refunds. Metered against all three ceilings.
#[test]
fn the_funded_order_instructions_fit_every_ceiling() {
    let mut nox = setup(0);
    let o = buy_limit(0);
    nox.place(0, o).unwrap();
    let keeper = nox.stranger();
    let ix = nox.fill_ix(&keeper.pubkey(), 0, &o, MID - 15 * PIP);
    let accounts = ix.accounts.len();
    let wire = nox.env.measure_keeper_tx(ix.clone(), &keeper);
    let fill = nox.env.send_metered(ix, &[&keeper]);
    nox.env.track_position(nox.position(0));
    println!("  funded_fill_entry_order {fill:>8} CU {wire:>6} bytes {accounts:>3} accounts");

    nox.env.advance_slot(1);
    let ix = nox.move_stop_ix(0, 0, 1, o.stop_loss_price + 5 * PPIP);
    let trader = nox.trader.insecure_clone();
    let m_accounts = ix.accounts.len();
    let m_wire = nox.env.measure_keeper_tx(ix.clone(), &trader);
    let mv = nox.env.send_metered(ix, &[&trader]);
    println!("  funded_move_stop        {mv:>8} CU {m_wire:>6} bytes {m_accounts:>3} accounts");

    assert!(accounts <= 64 && m_accounts <= 64);
    assert!(wire <= 1_232, "fill serialises to {wire} bytes");
    assert!(m_wire <= 1_232, "move serialises to {m_wire} bytes");
    assert!(
        fill <= FILL_CEILING,
        "fill consumed {fill} CU, above {FILL_CEILING}"
    );
    assert!(
        mv <= MOVE_CEILING,
        "move consumed {mv} CU, above {MOVE_CEILING}"
    );
}

// Measured 2026-10-03 over five runs. Both vary in exact steps of 1,500 CU — `solfx-core`
// creates the position and the trigger with an unstored bump, so each run's random keys search a
// different distance — and so, as in `budgets.rs`, the ceiling is the lowest measurement plus
// room for the search: 22 iterations for the fill (the same allowance `funded_open_position`
// carries, for the same two inits), 12 for the move (one init).
//
//   fill  113,315 / 114,815 / 116,315 / 120,815 / 120,815   892 bytes  23 accounts
//   move   44,697 /  46,197 /  49,197 /  50,697 /  52,197   637 bytes  15 accounts
const FILL_CEILING: u64 = 150_000; // 113,315 + 22 × 1,500 ≈ 146,315
const MOVE_CEILING: u64 = 63_000; //   44,697 + 12 × 1,500 = 62,697

#[test]
fn a_partial_close_fits_every_ceiling() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    let ix = nox.reduce_ix(0, SIZE / 2, MID + 10 * PIP, 0);
    let trader = nox.trader.insecure_clone();
    let accounts = ix.accounts.len();
    let wire = nox.env.measure_keeper_tx(ix.clone(), &trader);
    let cu = nox.env.send_metered(ix, &[&trader]);
    println!("  funded_reduce_position  {cu:>8} CU {wire:>6} bytes {accounts:>3} accounts");
    assert!(accounts <= 64);
    assert!(wire <= 1_232, "reduce serialises to {wire} bytes");
    assert!(
        cu <= REDUCE_CEILING,
        "reduce consumed {cu} CU, above {REDUCE_CEILING}"
    );
}

// Deterministic (no account is created): 85,157 CU, 812 bytes, 20 accounts on every run.
// Measured + ~30%, the method `stage3.rs` uses for deterministic instructions.
const REDUCE_CEILING: u64 = 110_000;

#[test]
fn moving_margin_fits_every_ceiling() {
    let mut nox = setup(0);
    nox.open(0, SPOT - 30 * PPIP).unwrap();
    let trader = nox.trader.insecure_clone();
    for (name, add, ceiling) in [
        ("funded_add_margin", true, ADD_MARGIN_CEILING),
        ("funded_remove_margin", false, REMOVE_MARGIN_CEILING),
    ] {
        let ix = nox.margin_ix(0, 10 * ONE_USDC, add, 0);
        let accounts = ix.accounts.len();
        let wire = nox.env.measure_keeper_tx(ix.clone(), &trader);
        let cu = nox.env.send_metered(ix, &[&trader]);
        println!("  {name:<23} {cu:>8} CU {wire:>6} bytes {accounts:>3} accounts");
        assert!(wire <= 1_232 && accounts <= 64);
        assert!(cu <= ceiling, "{name} consumed {cu} CU, above {ceiling}");
    }
}

// Deterministic (nothing is created): 66,508 and 71,621 CU, 771 bytes, 19 accounts, every run.
// Measured + ~30%.
const ADD_MARGIN_CEILING: u64 = 86_500;
const REMOVE_MARGIN_CEILING: u64 = 93_000;
