//! Stage 3 — the evaluation.
//!
//! Part 10's exit criterion, verbatim: *a simulated fill matches a real SolFX fill to the unit
//! on the same oracle input; no token moves.* The first test does exactly that — it opens a
//! virtual position and a real SolFX position against the same price update and the same market
//! state, then closes both, and compares every figure.
//!
//! The rest prove each rule both ways, pinned to the error it must produce, and walk two full
//! phases to a refunded stake.

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
use noxfunds::state::{EvalRule, Evaluation, EvaluationState, VirtualPosition};
use solana_signer::Signer;
use solfx_core::state::Direction;

/// EUR/USD mid, as a Pyth mantissa at exponent −8: 1.08543.
const MID: i64 = 108_543_000;
/// One pip (0.0001) at that exponent — 10,000, not 1,000, which would be a pipette.
const PIP: i64 = 10_000;

/// The same mid, and the same pip, at `PRICE_PRECISION` (1e9) — the units the program works in
/// once `load_validated_price` has normalised the feed.
///
/// **Two unit systems, on purpose.** A price is *posted* as Pyth publishes it (a mantissa at
/// exponent −8); a stop is *passed* in the program's units. Mixing them is the "a raw Pyth
/// mantissa is not a price" trap: the first version of this file passed stops as mantissas, and
/// every one read as ten times the price away — which the risk rule correctly refused.
const SPOT: i64 = 1_085_430_000;
const PPIP: i64 = 100_000;
const TEN_K: u64 = 10_000 * ONE_USDC;

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
fn vpos_pda(evaluation: &Pubkey, market_index: u16, nonce: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::VPOS_SEED,
            evaluation.as_ref(),
            &market_index.to_le_bytes(),
            &[nonce],
        ],
        &noxfunds::ID,
    )
    .0
}

/// Assert a refusal, and that it is refused for the right reason.
fn refused(r: TestResult, code: &str) {
    let err = r.expect_err("expected a refusal");
    assert!(err.contains(code), "refused, but not for {code}:\n{err}");
}

struct Eval {
    env: Env,
    trader: solana_keypair::Keypair,
    trader_token: Pubkey,
    treasury_token: Pubkey,
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
    let treasury_token = Pubkey::new_unique();
    env.write_token_account(treasury_token, env.usdc_mint, admin.pubkey(), 0);

    Eval {
        env,
        trader,
        trader_token,
        treasury_token,
        seq: 0,
    }
}

impl Eval {
    fn evaluation(&self) -> Pubkey {
        eval_pda(&self.trader.pubkey(), self.seq)
    }
    fn state(&self) -> Evaluation {
        self.env.read(&self.evaluation())
    }
    fn price(&mut self, mantissa: i64) -> Pubkey {
        self.env
            .post_price_now(FEED_EUR_USD, PriceSpec::at(mantissa))
    }

    fn start(&mut self, account_size: u64) -> TestResult {
        let ix = self.start_ix(account_size);
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    fn start_ix(&self, account_size: u64) -> Instruction {
        let trader = self.trader.insecure_clone();
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::StartEvaluation {
                trader: trader.pubkey(),
                config: config_pda(),
                trader_profile: profile_pda(&trader.pubkey()),
                // Sequential: the attempt before this one, which must be over (review M-2).
                previous_evaluation: self
                    .seq
                    .checked_sub(1)
                    .map(|prev| eval_pda(&trader.pubkey(), prev)),
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
                account_size,
            }
            .data(),
        }
    }

    fn open_ix(
        &self,
        nonce: u8,
        direction: Direction,
        size: u64,
        stop: i64,
        price: Pubkey,
    ) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalOpenPosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, 0, nonce),
                market: Env::market_pda(0),
                price_update: price,
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
        }
    }

    fn open(
        &mut self,
        nonce: u8,
        direction: Direction,
        size: u64,
        stop: i64,
        price: Pubkey,
    ) -> TestResult {
        let ix = self.open_ix(nonce, direction, size, stop, price);
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    fn close_ix(&self, nonce: u8, price: Pubkey) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalClosePosition {
                trader: self.trader.pubkey(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, 0, nonce),
                market: Env::market_pda(0),
                price_update: price,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalClosePosition {}.data(),
        }
    }

    fn close(&mut self, nonce: u8, price: Pubkey) -> TestResult {
        let ix = self.close_ix(nonce, price);
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    fn trigger_ix(&self, keeper: &Pubkey, nonce: u8, price: Pubkey) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalTriggerStop {
                keeper: *keeper,
                trader: self.trader.pubkey(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, 0, nonce),
                market: Env::market_pda(0),
                price_update: price,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalTriggerStop {}.data(),
        }
    }

    fn trigger(&mut self, nonce: u8, price: Pubkey) -> TestResult {
        let keeper = self.env.admin.insecure_clone();
        let ix = self.trigger_ix(&keeper.pubkey(), nonce, price);
        self.env.send(ix, &[&keeper])
    }

    fn observe_ix(&self, observer: &Pubkey, triples: &[(u8, Pubkey)]) -> Instruction {
        let evaluation = self.evaluation();
        let mut accounts = noxfunds::accounts::EvalObserveEquity {
            observer: *observer,
            evaluation,
        }
        .to_account_metas(None);
        for (nonce, price) in triples {
            accounts.push(AccountMeta::new_readonly(
                vpos_pda(&evaluation, 0, *nonce),
                false,
            ));
            accounts.push(AccountMeta::new_readonly(Env::market_pda(0), false));
            accounts.push(AccountMeta::new_readonly(*price, false));
        }
        Instruction {
            program_id: noxfunds::ID,
            accounts,
            data: noxfunds::instruction::EvalObserveEquity {}.data(),
        }
    }

    fn observe(&mut self, triples: &[(u8, Pubkey)]) -> TestResult {
        let observer = self.env.admin.insecure_clone();
        let ix = self.observe_ix(&observer.pubkey(), triples);
        self.env.send(ix, &[&observer])
    }

    fn claim_ix(&self) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::ClaimStagePass {
                trader: self.trader.pubkey(),
                config: config_pda(),
                evaluation,
                usdc_mint: self.env.usdc_mint,
                trader_token: self.trader_token,
                stake_vault: eval_vault_pda(&evaluation),
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::ClaimStagePass {}.data(),
        }
    }

    fn claim(&mut self) -> TestResult {
        let ix = self.claim_ix();
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    fn forfeit_ix(&self, caller: &Pubkey) -> Instruction {
        let evaluation = self.evaluation();
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::ForfeitStake {
                caller: *caller,
                config: config_pda(),
                evaluation,
                usdc_mint: self.env.usdc_mint,
                stake_vault: eval_vault_pda(&evaluation),
                treasury_token: self.treasury_token,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::ForfeitStake {}.data(),
        }
    }

    fn forfeit(&mut self) -> TestResult {
        let caller = self.env.admin.insecure_clone();
        let ix = self.forfeit_ix(&caller.pubkey());
        self.env.send(ix, &[&caller])
    }

    fn abandon_ix(&self) -> Instruction {
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::AbandonEvaluation {
                trader: self.trader.pubkey(),
                evaluation: self.evaluation(),
            }
            .to_account_metas(None),
            data: noxfunds::instruction::AbandonEvaluation {}.data(),
        }
    }

    fn abandon(&mut self) -> TestResult {
        let ix = self.abandon_ix();
        let trader = self.trader.insecure_clone();
        self.env.send(ix, &[&trader])
    }

    /// One whole trade: open long at `MID`, hold an hour, close `pips` higher.
    fn round_trip(&mut self, nonce: u8, pips: i64) {
        let p = self.price(MID);
        self.open(nonce, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
            .unwrap();
        self.env.advance_clock(3_600);
        let p = self.price(MID + pips * PIP);
        self.close(nonce, p).unwrap();
    }

    /// Two trades a day for five days: ten trades over five distinct UTC days, each held an hour.
    fn five_days(&mut self, pips_per_trade: &[i64; 10]) {
        for day in 0..5 {
            for k in 0..2 {
                let i = day * 2 + k;
                self.round_trip(u8::try_from(i).unwrap(), pips_per_trade[i]);
            }
            self.env.advance_clock(86_400);
        }
    }
}

// --- the exit criterion ------------------------------------------------------------------------

#[test]
fn a_virtual_fill_equals_a_real_solfx_fill_to_the_unit() {
    let mut e = setup();
    e.start(100_000 * ONE_USDC).unwrap();
    let price = e.price(MID);

    // Virtual first: it does not move open interest, so the real fill after it sees exactly the
    // market state the virtual one was priced in.
    e.open(0, Direction::Long, ONE_LOT, SPOT - 20 * PPIP, price)
        .unwrap();

    let user = e.env.new_user(20_000 * ONE_USDC, Pubkey::default());
    e.env.deposit(&user, 20_000 * ONE_USDC).unwrap();
    e.env
        .open(
            &user,
            0,
            0,
            Direction::Long,
            ONE_LOT,
            5_000 * ONE_USDC,
            i64::MAX,
            price,
        )
        .unwrap();

    let virt: VirtualPosition = e.env.read(&vpos_pda(&e.evaluation(), 0, 0));
    let real = e.env.position_state(&user, 0, 0);
    assert_eq!(virt.entry_price, real.entry_price, "execution price");
    assert_eq!(virt.entry_notional, real.entry_notional, "notional in USDC");
    assert_eq!(virt.open_fee, real.open_fee_paid, "open fee");

    // And the close. The virtual one goes first, while the real position is still open, so both
    // are priced against the same open interest.
    e.env.advance_clock(900);
    let later = e.price(MID + 15 * PIP);
    let balance_before = e.state().balance;
    e.close(0, later).unwrap();
    let virtual_net = e.state().balance - balance_before;

    let free_before = e.env.user_state(&user).free_collateral;
    e.env.close(&user, 0, 0, 0, later).unwrap();
    let released = e.env.user_state(&user).free_collateral - free_before;
    // Free collateral grows by the released margin plus the net result; the margin was 5,000.
    let real_net = i64::try_from(released).unwrap() - i64::try_from(5_000 * ONE_USDC).unwrap();

    assert_eq!(
        virtual_net, real_net,
        "net result of the close — PnL less close fee and carry"
    );
    assert!(virtual_net > 0, "the price rose on a long");

    // The simulation moved no token: SolFX's books balance exactly as if it never ran.
    e.env.assert_invariants();
}

#[test]
fn no_trading_instruction_touches_a_token_account() {
    // N3, structurally: the simulated trade path takes no token program and no token account,
    // so it cannot move one. Only start, claim and forfeit touch the stake escrow.
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    let keeper = e.env.admin.pubkey();
    for ix in [
        e.open_ix(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p),
        e.close_ix(0, p),
        e.trigger_ix(&keeper, 0, p),
        e.observe_ix(&keeper, &[(0, p)]),
        e.abandon_ix(),
    ] {
        assert!(
            ix.accounts.iter().all(|m| m.pubkey != spl_token::ID),
            "an evaluation trading instruction must not take the token program"
        );
    }
}

// --- starting ------------------------------------------------------------------------------

#[test]
fn starting_moves_the_stake_into_escrow() {
    let mut e = setup();
    let before = e.env.token_balance(&e.trader_token);
    e.start(TEN_K).unwrap();
    assert_eq!(e.env.token_balance(&e.trader_token), before - 50 * ONE_USDC);
    assert_eq!(
        e.env.token_balance(&eval_vault_pda(&e.evaluation())),
        50 * ONE_USDC
    );
    let s = e.state();
    assert_eq!(s.stage, 1);
    assert_eq!(s.state, EvaluationState::Active);
    assert_eq!(s.balance, i64::try_from(TEN_K).unwrap());
}

#[test]
fn an_account_size_outside_the_band_is_refused() {
    let mut e = setup();
    refused(e.start(5_000 * ONE_USDC), "InvalidAccountSize");
    refused(e.start(300_000 * ONE_USDC), "InvalidAccountSize");
}

#[test]
fn starting_without_the_stake_is_refused() {
    let mut e = setup();
    let (token, mint, owner) = (e.trader_token, e.env.usdc_mint, e.trader.pubkey());
    e.env.write_token_account(token, mint, owner, 10 * ONE_USDC);
    refused(e.start(TEN_K), "InsufficientPrincipal");
}

// --- the rules at open ---------------------------------------------------------------------------

#[test]
fn the_stop_is_mandatory() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    refused(
        e.open(0, Direction::Long, ONE_LOT, 0, p),
        "StopLossRequired",
    );
}

#[test]
fn a_stop_on_the_winning_side_is_refused() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    refused(
        e.open(0, Direction::Long, ONE_LOT, SPOT + 9 * PPIP, p),
        "StopOnWrongSide",
    );
    refused(
        e.open(0, Direction::Short, ONE_LOT, SPOT - 9 * PPIP, p),
        "StopOnWrongSide",
    );
}

#[test]
fn risk_at_the_stop_above_one_percent_is_refused_and_below_is_permitted() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    // One lot, 20 pips: $200 at risk on $10,000 — 2%.
    refused(
        e.open(0, Direction::Long, ONE_LOT, SPOT - 20 * PPIP, p),
        "RiskPerTradeExceeded",
    );
    // One lot, 9 pips: $90 — under 1%.
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
}

#[test]
fn leverage_beyond_the_market_cap_is_refused() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    // Ten lots on $10,000 is ~108× against a 50× market. The 1-pip stop keeps the risk rule
    // satisfied, so the refusal can only be the leverage cap.
    refused(
        e.open(0, Direction::Long, 10 * ONE_LOT, SPOT - PPIP, p),
        "EvaluationLeverageTooHigh",
    );
}

// --- closing and stops ------------------------------------------------------------------------

#[test]
fn a_voluntary_close_inside_ten_minutes_is_refused_but_a_stop_out_is_not() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();

    e.env.advance_clock(60);
    let p = e.price(MID);
    refused(e.close(0, p), "MinimumHoldNotMet");

    // The stop fires a minute in. The rulebook demands the stop, so it must not punish it.
    let through = e.price(MID - 10 * PIP);
    e.trigger(0, through)
        .expect("a stop-out is exempt from the minimum hold");
    let s = e.state();
    assert_eq!(s.trades, 1);
    assert_eq!(
        s.voluntary_closes, 0,
        "stop-outs are excluded from the average hold"
    );
    assert_eq!(s.open_positions, 0);
}

#[test]
fn a_stop_fires_only_when_the_oracle_reaches_it() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    let above = e.price(MID - 8 * PIP);
    refused(e.trigger(0, above), "StopNotTriggered");
    // Inclusive, as SolFX's own trigger is: a price exactly at the stop fires it.
    let at = e.price(MID - 9 * PIP);
    e.trigger(0, at).unwrap();
}

// --- the loss limits ------------------------------------------------------------------------

#[test]
fn the_crank_fails_an_evaluation_past_six_percent_drawdown() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    // A 100-pip gap straight through the stop: ~$1,000 on $10,000.
    let gap = e.price(MID - 100 * PIP);
    e.observe(&[(0, gap)]).unwrap();
    let s = e.state();
    assert_eq!(s.state, EvaluationState::Failed);
    assert_eq!(s.failed_rule, EvalRule::Drawdown);
}

#[test]
fn the_crank_fails_an_evaluation_past_three_percent_in_a_day() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    // ~40 pips: past the 3% daily limit, inside the 6% total one.
    let down = e.price(MID - 40 * PIP);
    e.observe(&[(0, down)]).unwrap();
    let s = e.state();
    assert_eq!(s.state, EvaluationState::Failed);
    assert_eq!(s.failed_rule, EvalRule::DailyLoss);
}

#[test]
fn the_crank_must_see_every_open_position_exactly_once() {
    let mut e = setup();
    e.start(100_000 * ONE_USDC).unwrap();
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    e.open(1, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    refused(e.observe(&[(0, p)]), "IncompleteObservation");
    refused(e.observe(&[(0, p), (0, p)]), "IncompleteObservation");
    e.observe(&[(0, p), (1, p)]).unwrap();
}

#[test]
fn a_failed_evaluation_takes_no_new_trades() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    e.abandon().unwrap();
    let p = e.price(MID);
    refused(
        e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p),
        "EvaluationNotActive",
    );
}

// --- the stake --------------------------------------------------------------------------------

#[test]
fn the_stake_is_forfeit_only_once_the_evaluation_has_failed_and_only_once() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    refused(e.forfeit(), "EvaluationNotFailed");

    e.abandon().unwrap();
    assert_eq!(e.state().failed_rule, EvalRule::Abandoned);
    e.forfeit().unwrap();
    assert_eq!(e.env.token_balance(&e.treasury_token), 50 * ONE_USDC);
    assert_eq!(e.env.token_balance(&eval_vault_pda(&e.evaluation())), 0);

    refused(e.forfeit(), "ZeroAmount");
}

// --- passing --------------------------------------------------------------------------------

#[test]
fn a_stage_cannot_be_claimed_early_or_with_a_position_open() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    refused(e.claim(), "EvaluationIncomplete");
    let p = e.price(MID);
    e.open(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    refused(e.claim(), "PositionsStillOpen");
}

#[test]
fn one_lucky_day_fails_the_consistency_rule() {
    let mut e = setup();
    e.start(TEN_K).unwrap();
    // Four ordinary days and one big one. The target is met — measured round trips cost ~$87 a
    // lot, so 18 pips nets ~$93 — but one day carries far more than half of it.
    e.five_days(&[18, 18, 18, 18, 18, 18, 18, 18, 60, 60]);
    refused(e.claim(), "ConsistencyRuleViolated");
}

#[test]
fn passing_both_phases_refunds_the_stake_in_full() {
    let mut e = setup();
    let start_balance = e.env.token_balance(&e.trader_token);
    e.start(TEN_K).unwrap();

    // Phase 1: ten trades over five days, each held an hour. 18 pips nets ~$93 a lot after the
    // ~$87 a round trip measured in fees and spread: $930 against an $800 target, best day
    // ~$186 against a $400 cap.
    e.five_days(&[18; 10]);
    e.claim().expect("phase 1 passes");
    let s = e.state();
    assert_eq!(s.stage, 2, "a pass starts phase 2 afresh");
    assert_eq!(s.balance, i64::try_from(TEN_K).unwrap());
    assert_eq!(s.trades, 0);

    // Phase 2, the same way.
    e.five_days(&[18; 10]);
    e.claim().expect("phase 2 passes");
    assert_eq!(e.state().state, EvaluationState::Passed);

    // The stake came back whole. The protocol earns from traders succeeding, not failing.
    assert_eq!(e.env.token_balance(&e.trader_token), start_balance);
    assert_eq!(e.env.token_balance(&eval_vault_pda(&e.evaluation())), 0);
}

// --- the budget ------------------------------------------------------------------------------

/// Meter one instruction: compute charged, wire size (with the two compute-budget instructions
/// a client prepends, via the same helper `budgets.rs` uses) and account locks.
fn meter(
    env: &mut Env,
    name: &str,
    ix: Instruction,
    payer: &solana_keypair::Keypair,
) -> (u64, usize) {
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
    println!("  {name:<22} {cu:>8} CU {wire:>6} bytes");
    (cu, wire)
}

/// **Every evaluation instruction, metered.** Run with `-- --nocapture` for the table.
///
/// Ceilings follow the method in `stage7.rs`: instructions that `init` an account without a
/// stored bump pay ~1,500 CU per `find_program_address` miss, so they get the lowest measured
/// figure plus 12 misses per search (about 2⁻¹² per search of a spurious failure); the rest were
/// identical run to run and get their figure plus about 30%.
#[test]
fn every_evaluation_instruction_stays_within_its_budget() {
    let mut e = setup();
    let trader = e.trader.insecure_clone();
    let keeper = e.env.admin.insecure_clone();

    let ix = e.start_ix(TEN_K);
    let (start, _) = meter(&mut e.env, "start_evaluation", ix, &trader);

    let p = e.price(MID);
    let ix = e.open_ix(0, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p);
    let (open, _) = meter(&mut e.env, "eval_open_position", ix, &trader);

    let ix = e.observe_ix(&keeper.pubkey(), &[(0, p)]);
    let (observe, _) = meter(&mut e.env, "eval_observe_equity", ix, &keeper);

    e.env.advance_clock(3_600);
    let p = e.price(MID + 18 * PIP);
    let ix = e.close_ix(0, p);
    let (close, _) = meter(&mut e.env, "eval_close_position", ix, &trader);

    let p = e.price(MID);
    e.open(1, Direction::Long, ONE_LOT, SPOT - 9 * PPIP, p)
        .unwrap();
    let through = e.price(MID - 10 * PIP);
    let ix = e.trigger_ix(&keeper.pubkey(), 1, through);
    let (trigger, _) = meter(&mut e.env, "eval_trigger_stop", ix, &keeper);

    // A passing claim: the heaviest branch of phase 1 is the full rule check.
    e.env.advance_clock(86_400);
    e.five_days(&[18; 10]);
    let ix = e.claim_ix();
    let (claim, _) = meter(&mut e.env, "claim_stage_pass", ix, &trader);

    // Phase 2's claim also refunds the stake through a token transfer, so it is the heavier of
    // the two and is metered rather than estimated.
    e.five_days(&[18; 10]);
    let ix = e.claim_ix();
    let (refund, _) = meter(&mut e.env, "claim_stage_pass_refund", ix, &trader);
    assert_eq!(e.state().state, EvaluationState::Passed);

    // A second evaluation, to meter the failure path.
    e.seq = 1;
    e.start(TEN_K).unwrap();

    let ix = e.abandon_ix();
    let (abandon, _) = meter(&mut e.env, "abandon_evaluation", ix, &trader);
    let ix = e.forfeit_ix(&keeper.pubkey());
    let (forfeit, _) = meter(&mut e.env, "forfeit_stake", ix, &keeper);

    for (name, cu, ceiling) in [
        ("start_evaluation", start, START_CEILING),
        ("eval_open_position", open, OPEN_CEILING),
        ("eval_observe_equity", observe, OBSERVE_CEILING),
        ("eval_close_position", close, CLOSE_CEILING),
        ("eval_trigger_stop", trigger, TRIGGER_CEILING),
        ("claim_stage_pass", claim, CLAIM_CEILING),
        ("claim_stage_pass_refund", refund, REFUND_CEILING),
        ("abandon_evaluation", abandon, ABANDON_CEILING),
        ("forfeit_stake", forfeit, FORFEIT_CEILING),
    ] {
        assert!(
            cu <= ceiling,
            "{name} consumed {cu} CU, above its {ceiling} ceiling"
        );
    }
}

// Measured on 2026-09-19 over six runs. Deterministic ones: measured + ~30%. Searched ones:
// lowest measured + 12 × 1,500 per `find_program_address` search (start: evaluation and stake
// vault, two; open: the virtual position, one).
const ABANDON_CEILING: u64 = 7_000; //      5,399
const FORFEIT_CEILING: u64 = 16_500; //    12,730
const OBSERVE_CEILING: u64 = 18_000; //    13,678 (one position)
const CLAIM_CEILING: u64 = 18_500; //      13,990 (phase 1, no transfer)
const CLOSE_CEILING: u64 = 20_000; //      15,100
const TRIGGER_CEILING: u64 = 20_000; //    15,415
const REFUND_CEILING: u64 = 21_000; //     16,105 (phase 2, stake refunded)
const OPEN_CEILING: u64 = 39_000; //       20,949 + 18,000
const START_CEILING: u64 = 61_000; //      24,705 + 36,000

/// Review M-2. **Evaluations run one at a time, in order.** A trader who could stake two at
/// once — long in one, short in the other — would keep whichever passed, buying a `StagePassed`
/// for one forfeited stake. The next attempt needs the previous one finished, and must follow it.
#[test]
fn evaluations_run_one_at_a_time_and_in_order() {
    let mut e = setup();
    e.start(20_000 * ONE_USDC).expect("the first attempt");

    // A second while the first is live.
    e.seq = 1;
    let err = e
        .start(20_000 * ONE_USDC)
        .expect_err("two evaluations at once");
    assert!(err.contains("PreviousEvaluationActive"), "{err}");

    // Skipping a number, so there is no predecessor to check.
    e.seq = 2;
    let err = e.start(20_000 * ONE_USDC).expect_err("seq cannot skip");
    assert!(
        err.contains("PreviousEvaluationActive") || err.contains("AccountNotInitialized"),
        "{err}"
    );

    // Once the first is over, the next may begin.
    e.seq = 0;
    e.abandon().expect("walk away from the first");
    e.seq = 1;
    e.start(20_000 * ONE_USDC)
        .expect("the second attempt, after the first ended");
}
