//! Orders on an evaluation: shorts, take-profits, a stop that tightens, resting entries, and
//! the verified mark.
//!
//! Every rule is proven both ways, each refusal pinned to the error it must produce. The ones
//! worth reading first are the ones that close loopholes: a target and a moved stop both wait
//! for the minimum hold, a limit never fills worse than its price, and an order that triggers
//! is judged by every rule of an open at the moment it fills.

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

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use common::*;
use noxfunds::instructions::EntryOrderParams;
use noxfunds::state::{
    EntryKind, EvalEntryOrder, Evaluation, EvaluationState, TraderProfile, VirtualPosition,
};
use solana_signer::Signer;
use solfx_core::state::Direction;

/// EUR/USD mid as a Pyth mantissa at exponent −8 (1.08543), one pip at that exponent, and the
/// same two at `PRICE_PRECISION` — see `stage3.rs` on why both unit systems appear.
const MID: i64 = 108_543_000;
const PIP: i64 = 10_000;
const SPOT: i64 = 1_085_430_000;
const PPIP: i64 = 100_000;
const TEN_K: u64 = 10_000 * ONE_USDC;
const QUARTER: u64 = ONE_LOT / 4;

fn nox_so_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/noxfunds.so")
}
fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[noxfunds::constants::CONFIG_SEED], &noxfunds::ID).0
}
fn profile_pda(trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::TRADER_SEED, trader.as_ref()],
        &noxfunds::ID,
    )
    .0
}
fn eval_pda(trader: &Pubkey, seq: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::EVAL_SEED, trader.as_ref(), &[seq]],
        &noxfunds::ID,
    )
    .0
}
fn eval_vault_pda(evaluation: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::EVAL_VAULT_SEED, evaluation.as_ref()],
        &noxfunds::ID,
    )
    .0
}
fn vpos_pda(evaluation: &Pubkey, nonce: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::VPOS_SEED,
            evaluation.as_ref(),
            &0u16.to_le_bytes(),
            &[nonce],
        ],
        &noxfunds::ID,
    )
    .0
}
fn order_pda(evaluation: &Pubkey, order_id: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::EVAL_ORDER_SEED,
            evaluation.as_ref(),
            &[order_id],
        ],
        &noxfunds::ID,
    )
    .0
}

/// Read a NOXFUNDS account that may have been closed. `Env::try_read` only accepts accounts
/// owned by `solfx-core`, so it reads every NOXFUNDS account as absent.
fn nox_read<T: anchor_lang::AccountDeserialize>(env: &Env, key: &Pubkey) -> Option<T> {
    let acct = env.svm.get_account(key)?;
    if acct.owner != noxfunds::ID || acct.data.len() < 8 {
        return None;
    }
    T::try_deserialize(&mut acct.data.as_slice()).ok()
}

fn refused(r: TestResult, code: &str) {
    let err = r.expect_err("expected a refusal");
    assert!(err.contains(code), "refused, but not for {code}:\n{err}");
}

struct Eval {
    env: Env,
    trader: solana_keypair::Keypair,
    trader_token: Pubkey,
    seq: u8,
}

fn setup() -> Eval {
    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
    let upgrade_authority = env.admin.pubkey();
    support::set_upgrade_authority(&mut env, Some(upgrade_authority));

    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

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

    let trader = solana_keypair::Keypair::new();
    env.svm
        .airdrop(&trader.pubkey(), 100 * 1_000_000_000)
        .unwrap();
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

    let trader_token = Pubkey::new_unique();
    env.write_token_account(
        trader_token,
        env.usdc_mint,
        trader.pubkey(),
        1_000 * ONE_USDC,
    );

    let mut e = Eval {
        env,
        trader,
        trader_token,
        seq: 0,
    };
    e.start().unwrap();
    e
}

impl Eval {
    fn evaluation(&self) -> Pubkey {
        eval_pda(&self.trader.pubkey(), self.seq)
    }
    fn state(&self) -> Evaluation {
        self.env.read(&self.evaluation())
    }
    fn position(&self, nonce: u8) -> Option<VirtualPosition> {
        nox_read(&self.env, &vpos_pda(&self.evaluation(), nonce))
    }
    fn profile(&self) -> TraderProfile {
        self.env.read(&profile_pda(&self.trader.pubkey()))
    }
    fn price(&mut self, mantissa: i64) -> Pubkey {
        self.env
            .post_price_now(FEED_EUR_USD, PriceSpec::at(mantissa))
    }
    fn as_trader(&mut self, ix: Instruction) -> TestResult {
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }
    fn as_keeper(&mut self, ix: Instruction) -> TestResult {
        let keeper = self.env.admin.insecure_clone();
        self.env.send(ix, &[&keeper])
    }
    fn stranger(&mut self) -> solana_keypair::Keypair {
        let k = solana_keypair::Keypair::new();
        self.env.svm.airdrop(&k.pubkey(), 1_000_000_000).unwrap();
        k
    }

    fn start(&mut self) -> TestResult {
        let trader = self.trader.pubkey();
        let evaluation = self.evaluation();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::StartEvaluation {
                trader,
                config: config_pda(),
                trader_profile: profile_pda(&trader),
                previous_evaluation: self.seq.checked_sub(1).map(|p| eval_pda(&trader, p)),
                evaluation,
                usdc_mint: self.env.usdc_mint,
                trader_token: self.trader_token,
                stake_vault: eval_vault_pda(&evaluation),
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::StartEvaluation {
                seq: self.seq,
                account_size: TEN_K,
            }
            .data(),
        };
        self.as_trader(ix)
    }

    fn open(
        &mut self,
        nonce: u8,
        direction: Direction,
        size: u64,
        stop: i64,
        p: Pubkey,
    ) -> TestResult {
        let evaluation = self.evaluation();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalOpenPosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, nonce),
                market: Env::market_pda(0),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalOpenPosition {
                market_index: 0,
                nonce,
                direction,
                size_base: size,
                stop_loss_price: stop,
            }
            .data(),
        };
        self.as_trader(ix)
    }

    fn close(&mut self, nonce: u8, p: Pubkey) -> TestResult {
        let evaluation = self.evaluation();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalClosePosition {
                trader: self.trader.pubkey(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, nonce),
                market: Env::market_pda(0),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalClosePosition {}.data(),
        };
        self.as_trader(ix)
    }

    fn trigger_stop(&mut self, nonce: u8, p: Pubkey) -> TestResult {
        let evaluation = self.evaluation();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalTriggerStop {
                keeper: self.env.admin.pubkey(),
                trader: self.trader.pubkey(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, nonce),
                market: Env::market_pda(0),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalTriggerStop {}.data(),
        };
        self.as_keeper(ix)
    }

    fn set_tp_ix(&self, signer: &Pubkey, nonce: u8, price: i64, p: Pubkey) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalSetTakeProfit {
                trader: *signer,
                evaluation,
                virtual_position: vpos_pda(&evaluation, nonce),
                market: Env::market_pda(0),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalSetTakeProfit {
                trigger_price: price,
            }
            .data(),
        }
    }

    fn set_tp(&mut self, nonce: u8, price: i64, p: Pubkey) -> TestResult {
        let ix = self.set_tp_ix(&self.trader.pubkey(), nonce, price, p);
        self.as_trader(ix)
    }

    fn fire_tp_ix(&self, keeper: &Pubkey, nonce: u8, p: Pubkey) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalTriggerTakeProfit {
                keeper: *keeper,
                trader: self.trader.pubkey(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, nonce),
                market: Env::market_pda(0),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalTriggerTakeProfit {}.data(),
        }
    }

    fn fire_tp(&mut self, nonce: u8, p: Pubkey) -> TestResult {
        let ix = self.fire_tp_ix(&self.env.admin.pubkey(), nonce, p);
        self.as_keeper(ix)
    }

    fn move_stop_ix(&self, nonce: u8, stop: i64, p: Pubkey) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalMoveStop {
                trader: self.trader.pubkey(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, nonce),
                market: Env::market_pda(0),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalMoveStop { new_stop: stop }.data(),
        }
    }

    fn move_stop(&mut self, nonce: u8, stop: i64, p: Pubkey) -> TestResult {
        let ix = self.move_stop_ix(nonce, stop, p);
        self.as_trader(ix)
    }

    fn place_ix(&self, order_id: u8, params: EntryOrderParams) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalPlaceEntryOrder {
                trader: self.trader.pubkey(),
                config: config_pda(),
                evaluation,
                entry_order: order_pda(&evaluation, order_id),
                market: Env::market_pda(0),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalPlaceEntryOrder { order_id, params }.data(),
        }
    }

    fn place(&mut self, order_id: u8, params: EntryOrderParams) -> TestResult {
        let ix = self.place_ix(order_id, params);
        self.as_trader(ix)
    }

    fn fill_ix(&self, keeper: &Pubkey, order_id: u8, nonce: u8, p: Pubkey) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalFillEntryOrder {
                keeper: *keeper,
                config: config_pda(),
                evaluation,
                entry_order: order_pda(&evaluation, order_id),
                virtual_position: vpos_pda(&evaluation, nonce),
                market: Env::market_pda(0),
                price_update: p,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalFillEntryOrder {}.data(),
        }
    }

    fn fill(&mut self, order_id: u8, nonce: u8, p: Pubkey) -> TestResult {
        let ix = self.fill_ix(&self.env.admin.pubkey(), order_id, nonce, p);
        self.as_keeper(ix)
    }

    fn cancel_ix(&self, caller: &Pubkey, order_id: u8) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalCancelEntryOrder {
                caller: *caller,
                trader: self.trader.pubkey(),
                evaluation,
                entry_order: order_pda(&evaluation, order_id),
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalCancelEntryOrder {}.data(),
        }
    }

    fn claim(&mut self) -> TestResult {
        let evaluation = self.evaluation();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::ClaimStagePass {
                trader: self.trader.pubkey(),
                config: config_pda(),
                evaluation,
                usdc_mint: self.env.usdc_mint,
                trader_token: self.trader_token,
                stake_vault: eval_vault_pda(&evaluation),
                trader_profile: profile_pda(&self.trader.pubkey()),
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::ClaimStagePass {}.data(),
        };
        self.as_trader(ix)
    }

    /// One whole long trade: open at `MID`, hold an hour, close `pips` higher.
    fn round_trip(&mut self, nonce: u8, pips: i64) {
        let p = self.price(MID);
        self.open(nonce, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
            .unwrap();
        self.env.advance_clock(3_600);
        let p = self.price(MID + pips * PIP);
        self.close(nonce, p).unwrap();
    }

    /// Ten trades over five UTC days, each held an hour — one stage's activity requirement.
    fn five_days(&mut self, pips: i64) {
        for day in 0..5u8 {
            for k in 0..2u8 {
                self.round_trip(day * 2 + k, pips);
            }
            self.env.advance_clock(86_400);
        }
    }
}

/// A buy limit `pips` below the mid, a quarter lot, stop 30 pips under the limit, no target.
fn buy_limit(nonce: u8, pips_below: i64) -> EntryOrderParams {
    let trigger = SPOT - pips_below * PPIP;
    EntryOrderParams {
        market_index: 0,
        nonce,
        direction: Direction::Long,
        kind: EntryKind::Limit,
        trigger_price: trigger,
        size_base: QUARTER,
        stop_loss_price: trigger - 30 * PPIP,
        take_profit_price: 0,
        expires_at: 0,
    }
}

// --- shorts ------------------------------------------------------------------------------------

/// The program always took shorts; the browser offered only longs. A short is the mirror image:
/// the stop goes above, and a fall is the profit.
#[test]
fn a_short_round_trip_books_a_win_when_the_price_falls() {
    let mut e = setup();
    let p = e.price(MID);
    refused(
        e.open(0, Direction::Short, ONE_LOT, SPOT - 9 * PPIP, p),
        "StopOnWrongSide",
    );
    e.open(0, Direction::Short, ONE_LOT, SPOT + 9 * PPIP, p)
        .expect("a short with its stop above");

    e.env.advance_clock(3_600);
    let p = e.price(MID - 30 * PIP);
    e.close(0, p).unwrap();
    let s = e.state();
    assert_eq!((s.trades, s.wins, s.losses), (1, 1, 0));
    assert!(
        s.balance > i64::try_from(TEN_K).unwrap(),
        "30 pips down is a profit on a short"
    );
}

// --- the take-profit ---------------------------------------------------------------------------

#[test]
fn a_take_profit_goes_only_where_it_has_not_happened() {
    let mut e = setup();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    refused(e.set_tp(0, SPOT - PPIP, p), "TakeProfitOnWrongSide");
    refused(e.set_tp(0, SPOT, p), "TakeProfitOnWrongSide");
    refused(e.set_tp(0, -5, p), "TakeProfitOnWrongSide");
    e.set_tp(0, SPOT + 20 * PPIP, p).expect("above a long");
    assert_eq!(e.position(0).unwrap().take_profit_price, SPOT + 20 * PPIP);

    // Clearing it reads no price, so it works on a feed that has gone stale.
    e.env.advance_clock(600);
    e.set_tp(0, 0, p).expect("clearing needs no fresh price");
    assert_eq!(e.position(0).unwrap().take_profit_price, 0);
}

#[test]
fn only_the_trader_sets_a_take_profit() {
    let mut e = setup();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    let stranger = e.stranger();
    let ix = e.set_tp_ix(&stranger.pubkey(), 0, SPOT + 20 * PPIP, p);
    let err = e
        .env
        .send(ix, &[&stranger])
        .expect_err("a stranger's target");
    assert!(
        err.contains("ConstraintSeeds") || err.contains("NotTheTrader"),
        "{err}"
    );
}

/// **A target is a chosen exit, so it waits for the hold.** Otherwise a target one tick away is
/// a way out in minute one that the stop-out exemption was never meant to give.
#[test]
fn a_take_profit_fires_on_the_oracle_only_after_the_hold_and_counts_as_voluntary() {
    let mut e = setup();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    e.set_tp(0, SPOT + 20 * PPIP, p).unwrap();

    // Reached, but inside the ten minutes.
    e.env.advance_clock(300);
    let p = e.price(MID + 25 * PIP);
    refused(e.fire_tp(0, p), "MinimumHoldNotMet");

    // Past the hold, but not reached.
    e.env.advance_clock(400);
    let p = e.price(MID + 10 * PIP);
    refused(e.fire_tp(0, p), "TakeProfitNotTriggered");

    // Reached, exactly at the trigger: inclusive, as SolFX's own triggers are.
    let p = e.price(MID + 20 * PIP);
    e.fire_tp(0, p)
        .expect("fires at the trigger once the hold has passed");
    assert!(e.position(0).is_none(), "the position is closed");
    let s = e.state();
    assert_eq!((s.trades, s.wins), (1, 1));
    assert_eq!(s.voluntary_closes, 1, "a target is a voluntary close");
    assert_eq!(s.voluntary_hold_secs, 700);
    assert_eq!(s.open_positions, 0);
    // No token moved: SolFX's books balance exactly as if the simulation never ran.
    e.env.assert_invariants();
}

#[test]
fn a_position_without_a_target_has_nothing_to_fire() {
    let mut e = setup();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    e.env.advance_clock(3_600);
    let p = e.price(MID + 100 * PIP);
    refused(e.fire_tp(0, p), "TakeProfitNotTriggered");
}

// --- moving the stop ---------------------------------------------------------------------------

/// Tighten only, never to where it is already met, and only after the hold — the three rules
/// that make a movable stop safe under a stop-out exemption.
#[test]
fn a_stop_tightens_after_the_hold_and_never_loosens() {
    let mut e = setup();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();

    refused(e.move_stop(0, SPOT - 5 * PPIP, p), "MinimumHoldNotMet");

    e.env.advance_clock(600);
    let p = e.price(MID + 20 * PIP);
    refused(e.move_stop(0, SPOT - 12 * PPIP, p), "StopNotTighter");
    refused(e.move_stop(0, SPOT - 9 * PPIP, p), "StopNotTighter");
    // At or past the price is not a stop, it is a close.
    refused(e.move_stop(0, SPOT + 20 * PPIP, p), "TriggerAlreadyMet");

    // To breakeven and beyond is allowed: the risk only falls.
    e.move_stop(0, SPOT + 5 * PPIP, p).expect("into profit");
    assert_eq!(e.position(0).unwrap().stop_price, SPOT + 5 * PPIP);

    // And it is the moved stop that fires.
    let p = e.price(MID + 5 * PIP);
    e.trigger_stop(0, p).expect("the moved stop fires");
    let s = e.state();
    assert_eq!(s.trades, 1);
    assert_eq!(s.voluntary_closes, 0, "still a stop-out");
    e.env.assert_invariants();
}

#[test]
fn a_short_stop_tightens_downward() {
    let mut e = setup();
    let p = e.price(MID);
    e.open(0, Direction::Short, ONE_LOT, SPOT + 9 * PPIP, p)
        .unwrap();
    e.env.advance_clock(600);
    let p = e.price(MID);
    refused(e.move_stop(0, SPOT + 10 * PPIP, p), "StopNotTighter");
    e.move_stop(0, SPOT + 3 * PPIP, p)
        .expect("down, toward the price");
}

// --- resting entry orders ----------------------------------------------------------------------

#[test]
fn a_limit_rests_until_the_oracle_reaches_it_then_fills_under_every_rule() {
    let mut e = setup();
    let keeper = e.env.admin.pubkey();
    let mut params = buy_limit(0, 5);
    params.take_profit_price = params.trigger_price + 40 * PPIP;
    e.place(0, params).expect("a buy limit under the market");
    assert_eq!(e.state().pending_orders, 1);

    // Not reached.
    let p = e.price(MID);
    refused(e.fill(0, 0, p), "EntryNotTriggered");

    // Reached on the oracle, but a buy executes above the oracle: the spread would make this a
    // fill above the limit, which a limit never does.
    let p = e.price(MID - 5 * PIP);
    refused(e.fill(0, 0, p), "EntryNotTriggered");

    // Far enough through that the execution price, spread included, is at or under the limit.
    let p = e.price(MID - 10 * PIP);
    let order_rent = e.env.sol_balance(&order_pda(&e.evaluation(), 0));
    let before = e.env.sol_balance(&keeper);
    e.fill(0, 0, p).expect("fills");
    let pos = e.position(0).expect("a position");
    assert!(
        pos.entry_price <= params.trigger_price,
        "never worse than the limit"
    );
    assert_eq!(pos.stop_price, params.stop_loss_price);
    assert_eq!(pos.take_profit_price, params.take_profit_price);
    assert_eq!(pos.size_base, QUARTER);

    let s = e.state();
    assert_eq!(s.pending_orders, 0);
    assert_eq!(s.open_positions, 1);
    assert_eq!(
        e.env.sol_balance(&order_pda(&e.evaluation(), 0)),
        0,
        "order closed"
    );

    // The filler is never out of pocket: it was paid the order's rent and paid the position's.
    let position_rent = e.env.sol_balance(&vpos_pda(&e.evaluation(), 0));
    assert!(
        order_rent >= position_rent,
        "{order_rent} < {position_rent}"
    );
    let fee = 5_000;
    assert!(e.env.sol_balance(&keeper) + fee >= before);
    e.env.assert_invariants();
}

#[test]
fn a_sell_stop_entry_fills_when_the_price_falls_through_it() {
    let mut e = setup();
    let trigger = SPOT - 10 * PPIP;
    let params = EntryOrderParams {
        market_index: 0,
        nonce: 0,
        direction: Direction::Short,
        kind: EntryKind::Stop,
        trigger_price: trigger,
        size_base: QUARTER,
        stop_loss_price: trigger + 30 * PPIP,
        take_profit_price: trigger - 30 * PPIP,
        expires_at: 0,
    };
    e.place(0, params).unwrap();
    let p = e.price(MID - 9 * PIP);
    refused(e.fill(0, 0, p), "EntryNotTriggered");
    let p = e.price(MID - 10 * PIP);
    e.fill(0, 0, p).expect("a sell stop at its trigger");
    assert_eq!(e.position(0).unwrap().direction, Direction::Short);
}

/// What can be judged without a price is judged at placement, so an order that could never
/// fill does not rest.
#[test]
fn placement_refuses_a_bracket_that_could_never_work() {
    let mut e = setup();
    let base = buy_limit(0, 5);

    let mut wrong_stop = base;
    wrong_stop.stop_loss_price = base.trigger_price + PPIP;
    refused(e.place(0, wrong_stop), "StopOnWrongSide");

    let mut wrong_tp = base;
    wrong_tp.take_profit_price = base.trigger_price - PPIP;
    refused(e.place(0, wrong_tp), "TakeProfitOnWrongSide");

    // 1 lot with a 30-pip stop is $300 at risk: 3% of $10,000.
    let mut too_big = base;
    too_big.size_base = ONE_LOT;
    refused(e.place(0, too_big), "RiskPerTradeExceeded");

    let mut past = base;
    past.expires_at = e.env.now - 1;
    refused(e.place(0, past), "InvalidExpiry");

    let mut no_stop = base;
    no_stop.stop_loss_price = 0;
    refused(e.place(0, no_stop), "StopLossRequired");

    assert_eq!(e.state().pending_orders, 0);
}

/// **A gap through the stop refuses the fill.** The limit was reached, but by a price already
/// below the stop — opening there would be opening a position its own stop has already closed.
#[test]
fn a_gap_through_the_stop_refuses_the_fill() {
    let mut e = setup();
    let params = buy_limit(0, 5);
    e.place(0, params).unwrap();
    let p = e.price(MID - 40 * PIP);
    refused(e.fill(0, 0, p), "StopOnWrongSide");
    assert_eq!(e.state().pending_orders, 1, "it waits");
}

/// Rules are judged at the fill, not frozen at placement: a book that filled up in between
/// refuses the order exactly as it would refuse a market order.
#[test]
fn the_rules_are_judged_again_at_the_fill() {
    let mut e = setup();
    e.place(0, buy_limit(9, 5)).unwrap();
    let p = e.price(MID);
    for n in 0..5 {
        e.open(n, Direction::Long, QUARTER, SPOT - 9 * PPIP, p)
            .unwrap();
    }
    let p = e.price(MID - 10 * PIP);
    refused(e.fill(0, 9, p), "TooManyOpenPositions");
}

#[test]
fn an_expired_order_cannot_fill_and_anyone_may_clear_it() {
    let mut e = setup();
    let mut params = buy_limit(0, 5);
    params.expires_at = e.env.now + 100;
    e.place(0, params).unwrap();
    let stranger = e.stranger();

    // Live and unexpired: only the trader may cancel.
    let ix = e.cancel_ix(&stranger.pubkey(), 0);
    refused(e.env.send(ix, &[&stranger]), "NotYourOrderToCancel");

    e.env.advance_clock(200);
    let p = e.price(MID - 10 * PIP);
    refused(e.fill(0, 0, p), "EntryOrderExpired");

    let rent_before = e.env.sol_balance(&e.trader.pubkey());
    let ix = e.cancel_ix(&stranger.pubkey(), 0);
    e.env.send(ix, &[&stranger]).expect("anyone, once expired");
    assert!(
        e.env.sol_balance(&e.trader.pubkey()) > rent_before,
        "the rent goes to the trader, not the canceller"
    );
    assert_eq!(e.state().pending_orders, 0);
}

#[test]
fn the_trader_cancels_any_time() {
    let mut e = setup();
    e.place(0, buy_limit(0, 5)).unwrap();
    let ix = e.cancel_ix(&e.trader.pubkey(), 0);
    e.as_trader(ix).unwrap();
    assert_eq!(e.state().pending_orders, 0);
    assert!(nox_read::<EvalEntryOrder>(&e.env, &order_pda(&e.evaluation(), 0)).is_none());
}

#[test]
fn an_ended_evaluation_fills_nothing_and_its_orders_are_anyones_to_clear() {
    let mut e = setup();
    e.place(0, buy_limit(0, 5)).unwrap();
    let evaluation = e.evaluation();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::AbandonEvaluation {
            trader: e.trader.pubkey(),
            evaluation,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::AbandonEvaluation {}.data(),
    };
    e.as_trader(ix).unwrap();

    let p = e.price(MID - 10 * PIP);
    refused(e.fill(0, 0, p), "EvaluationNotActive");
    let stranger = e.stranger();
    let ix = e.cancel_ix(&stranger.pubkey(), 0);
    e.env
        .send(ix, &[&stranger])
        .expect("anyone, once it cannot fill");
}

#[test]
fn at_most_five_orders_rest_at_once() {
    let mut e = setup();
    for id in 0..5 {
        e.place(id, buy_limit(id, 5)).unwrap();
    }
    refused(e.place(5, buy_limit(5, 5)), "TooManyPendingOrders");
}

/// **An order from Phase 1 cannot fill into Phase 2.** The stage is claimed flat and empty.
#[test]
fn a_resting_order_blocks_the_claim() {
    let mut e = setup();
    e.place(0, buy_limit(0, 5)).unwrap();
    refused(e.claim(), "EntryOrdersPending");
}

#[test]
fn a_pause_stops_placing_and_filling_but_not_cancelling() {
    let mut e = setup();
    e.place(0, buy_limit(0, 5)).unwrap();
    let admin = e.env.admin.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::SetPaused {
            authority: admin.pubkey(),
            config: config_pda(),
        }
        .to_account_metas(None),
        data: noxfunds::instruction::SetPaused { paused: true }.data(),
    };
    e.env.send(ix, &[&admin]).unwrap();

    refused(e.place(1, buy_limit(1, 5)), "ProtocolPaused");
    let p = e.price(MID - 10 * PIP);
    refused(e.fill(0, 0, p), "ProtocolPaused");
    let ix = e.cancel_ix(&e.trader.pubkey(), 0);
    e.as_trader(ix).expect("cancelling takes risk off");
}

// --- the verified mark -------------------------------------------------------------------------

/// **Verification is the Phase 2 pass, and only that.** Phase 1 records nothing; the profile is
/// credited once, at the moment the stake is refunded.
#[test]
fn passing_both_phases_marks_the_profile_verified_and_phase_one_alone_does_not() {
    let mut e = setup();
    assert_eq!(e.profile().evaluations_passed, 0);

    e.five_days(18);
    e.claim().expect("phase 1 passes");
    assert_eq!(
        e.profile().evaluations_passed,
        0,
        "phase 1 alone verifies nothing"
    );

    e.five_days(18);
    e.claim().expect("phase 2 passes");
    assert_eq!(e.state().state, EvaluationState::Passed);
    let profile = e.profile();
    assert_eq!(profile.evaluations_passed, 1);
    assert_eq!(profile.last_passed_at, e.env.now);
}

#[test]
fn a_failed_evaluation_verifies_nothing() {
    let mut e = setup();
    let evaluation = e.evaluation();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::AbandonEvaluation {
            trader: e.trader.pubkey(),
            evaluation,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::AbandonEvaluation {}.data(),
    };
    e.as_trader(ix).unwrap();
    refused(e.claim(), "EvaluationNotActive");
    assert_eq!(e.profile().evaluations_passed, 0);
}

// --- the budget ----------------------------------------------------------------------------------

/// Meter one instruction: compute, wire size with the two compute-budget instructions a client
/// prepends, and account locks — the same helper `stage3.rs` and `budgets.rs` use.
fn meter(env: &mut Env, name: &str, ix: Instruction, payer: &solana_keypair::Keypair) -> u64 {
    let wire = env.measure_keeper_tx(ix.clone(), payer);
    assert!(
        ix.accounts.len() <= 64,
        "{name} locks {} accounts",
        ix.accounts.len()
    );
    assert!(
        wire <= 1_232,
        "{name} serialises to {wire} bytes, over 1,232"
    );
    let cu = env.send_metered(ix, &[payer]);
    println!("  {name:<26} {cu:>8} CU {wire:>6} bytes");
    cu
}

/// **Every order instruction, metered.** Run with `-- --nocapture` for the table.
///
/// Ceilings follow `stage3.rs`: instructions that `init` an account without a stored bump pay
/// ~1,500 CU per `find_program_address` miss, so they get the lowest measured figure plus 12
/// misses; the rest were identical run to run and get their figure plus about 30%.
#[test]
fn every_order_instruction_stays_within_its_budget() {
    let mut e = setup();
    let trader = e.trader.insecure_clone();
    let keeper = e.env.admin.insecure_clone();

    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    let ix = e.set_tp_ix(&trader.pubkey(), 0, SPOT + 20 * PPIP, p);
    let set_tp = meter(&mut e.env, "eval_set_take_profit", ix, &trader);

    e.env.advance_clock(600);
    let p = e.price(MID + 5 * PIP);
    let ix = e.move_stop_ix(0, SPOT - 2 * PPIP, p);
    let move_stop = meter(&mut e.env, "eval_move_stop", ix, &trader);

    let p = e.price(MID + 20 * PIP);
    let ix = e.fire_tp_ix(&keeper.pubkey(), 0, p);
    let fire_tp = meter(&mut e.env, "eval_trigger_take_profit", ix, &keeper);

    let ix = e.place_ix(0, buy_limit(1, 5));
    let place = meter(&mut e.env, "eval_place_entry_order", ix, &trader);

    let p = e.price(MID - 10 * PIP);
    let ix = e.fill_ix(&keeper.pubkey(), 0, 1, p);
    let fill = meter(&mut e.env, "eval_fill_entry_order", ix, &keeper);

    e.place(1, buy_limit(2, 5)).unwrap();
    let ix = e.cancel_ix(&trader.pubkey(), 1);
    let cancel = meter(&mut e.env, "eval_cancel_entry_order", ix, &trader);

    for (name, cu, ceiling) in [
        ("eval_set_take_profit", set_tp, SET_TP_CEILING),
        ("eval_move_stop", move_stop, MOVE_STOP_CEILING),
        ("eval_trigger_take_profit", fire_tp, FIRE_TP_CEILING),
        ("eval_place_entry_order", place, PLACE_CEILING),
        ("eval_fill_entry_order", fill, FILL_CEILING),
        ("eval_cancel_entry_order", cancel, CANCEL_CEILING),
    ] {
        assert!(
            cu <= ceiling,
            "{name} consumed {cu} CU, above its {ceiling} ceiling"
        );
    }
}

// Measured 2026-10-03 over three runs. Deterministic: measured + ~30%. Searched (an `init` with
// no stored bump — the order on placement, the position on fill): lowest + 12 × 1,500.
const CANCEL_CEILING: u64 = 11_000; //     8,483
const SET_TP_CEILING: u64 = 13_500; //    10,201
const MOVE_STOP_CEILING: u64 = 13_500; // 10,252
const FIRE_TP_CEILING: u64 = 21_000; //   15,934
const PLACE_CEILING: u64 = 36_000; //     18,032 + 18,000
const FILL_CEILING: u64 = 42_500; //      24,512 + 18,000
