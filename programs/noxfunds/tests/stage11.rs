//! Evaluations on every market shape: a pair not quoted in USD, a synthetic, and a book that
//! holds both beside a direct one.
//!
//! What has to be true: each is priced by `load_validated_price` with exactly the legs its market
//! needs; a missing leg is refused by name rather than guessed at; and the equity crank takes
//! each position's legs in the number its market demands, so a mark can neither skip a leg nor
//! be handed one the market does not use.

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
use noxfunds::state::Evaluation;
use solana_signer::Signer;
use solfx_core::state::Direction;

const TEN_K: u64 = 10_000 * ONE_USDC;
/// One thousand units of base currency at BASE_PRECISION.
const THOUSAND: u64 = 1_000 * 1_000_000_000;

const EURUSD: u16 = 0;
const USDINR: u16 = 1;
const EURGBP: u16 = 2;

/// USD/INR at 88.00 and GBP/USD at 1.27, as Pyth mantissas at exponent −8.
const INR_M: i64 = 8_800_000_000;
const GBP_M: i64 = 127_000_000;
/// The same prices at PRICE_PRECISION 1e9.
const INR: i64 = 88_000_000_000;
const EUR: i64 = 1_085_430_000;
/// EUR/GBP composed as the program composes it: EUR/USD ÷ GBP/USD, floored.
const EURGBP_SPOT: i64 = 854_669_291; // 1_085_430_000 × 1e9 ÷ 1_270_000_000, floored

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
fn eval_pda(trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::EVAL_SEED, trader.as_ref(), &[0]],
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

fn refused(r: TestResult, code: &str) {
    let err = r.expect_err("expected a refusal");
    assert!(err.contains(code), "refused, but not for {code}:\n{err}");
}

/// A market's price legs, as `load_validated_price` takes them.
#[derive(Clone, Copy)]
struct Legs {
    primary: Pubkey,
    secondary: Option<Pubkey>,
    conversion: Option<Pubkey>,
}

struct Eval {
    env: Env,
    trader: solana_keypair::Keypair,
}

fn setup() -> Eval {
    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
    let upgrade_authority = env.admin.pubkey();
    support::set_upgrade_authority(&mut env, Some(upgrade_authority));

    env.init_protocol();
    env.list_and_activate(EURUSD, &MarketSpec::eur_usd());
    env.list_and_activate(USDINR, &MarketSpec::usd_inr());
    env.list_and_activate(EURGBP, &MarketSpec::eur_gbp_synthetic());
    env.seed_pool(1_000_000 * ONE_USDC);

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
    let evaluation = eval_pda(&trader.pubkey());
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::StartEvaluation {
            trader: trader.pubkey(),
            config: config_pda(),
            trader_profile: profile_pda(&trader.pubkey()),
            previous_evaluation: None,
            evaluation,
            usdc_mint: env.usdc_mint,
            trader_token,
            stake_vault: eval_vault_pda(&evaluation),
            token_program: spl_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::StartEvaluation {
            seq: 0,
            account_size: TEN_K,
        }
        .data(),
    };
    let t = trader.insecure_clone();
    env.send(ix, &[&t]).unwrap();
    Eval { env, trader }
}

impl Eval {
    fn evaluation(&self) -> Pubkey {
        eval_pda(&self.trader.pubkey())
    }
    fn state(&self) -> Evaluation {
        self.env.read(&self.evaluation())
    }

    /// Post every feed the three markets read, now, and return each market's legs.
    fn prices(&mut self, eur_m: i64, inr_m: i64, gbp_m: i64) -> [Legs; 3] {
        let eur = self.env.post_price_now(FEED_EUR_USD, PriceSpec::at(eur_m));
        let inr = self.env.post_price_now(FEED_USD_INR, PriceSpec::at(inr_m));
        let gbp = self.env.post_price_now(FEED_GBP_USD, PriceSpec::at(gbp_m));
        [
            Legs {
                primary: eur,
                secondary: None,
                conversion: None,
            },
            // USD/INR converts with its own feed: the rate is the price.
            Legs {
                primary: inr,
                secondary: None,
                conversion: Some(inr),
            },
            Legs {
                primary: eur,
                secondary: Some(gbp),
                conversion: None,
            },
        ]
    }

    fn open(
        &mut self,
        market_index: u16,
        legs: Legs,
        direction: Direction,
        size: u64,
        stop: i64,
    ) -> TestResult {
        let evaluation = self.evaluation();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalOpenPosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, market_index, 0),
                market: Env::market_pda(market_index),
                price_update: legs.primary,
                secondary_price_update: legs.secondary,
                quote_conversion_price_update: legs.conversion,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalOpenPosition {
                market_index,
                nonce: 0,
                direction,
                size_base: size,
                stop_loss_price: stop,
            }
            .data(),
        };
        let t = self.trader.insecure_clone();
        self.env.send(ix, &[&t])
    }

    fn close(&mut self, market_index: u16, legs: Legs) -> TestResult {
        let evaluation = self.evaluation();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::EvalClosePosition {
                trader: self.trader.pubkey(),
                evaluation,
                virtual_position: vpos_pda(&evaluation, market_index, 0),
                market: Env::market_pda(market_index),
                price_update: legs.primary,
                secondary_price_update: legs.secondary,
                quote_conversion_price_update: legs.conversion,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::EvalClosePosition {}.data(),
        };
        let t = self.trader.insecure_clone();
        self.env.send(ix, &[&t])
    }

    /// Mark the book with `groups`: each (market index, legs) for an open position at nonce 0.
    fn observe(&mut self, groups: &[(u16, Legs)]) -> TestResult {
        let evaluation = self.evaluation();
        let observer = self.env.admin.insecure_clone();
        let mut accounts = noxfunds::accounts::EvalObserveEquity {
            observer: observer.pubkey(),
            evaluation,
        }
        .to_account_metas(None);
        for (market_index, legs) in groups {
            accounts.push(AccountMeta::new_readonly(
                vpos_pda(&evaluation, *market_index, 0),
                false,
            ));
            accounts.push(AccountMeta::new_readonly(
                Env::market_pda(*market_index),
                false,
            ));
            accounts.push(AccountMeta::new_readonly(legs.primary, false));
            for leg in [legs.secondary, legs.conversion].into_iter().flatten() {
                accounts.push(AccountMeta::new_readonly(leg, false));
            }
        }
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts,
            data: noxfunds::instruction::EvalObserveEquity {}.data(),
        };
        self.env.send(ix, &[&observer])
    }
}

/// **A rupee-quoted pair, in dollars.** USD/INR's P&L is in rupees; the evaluation's balance is
/// in USDC. A 1% rise on a long of $1,000 must land as roughly $10 less the round-trip costs —
/// not as 880 rupees mistaken for dollars, which is the failure conversion exists to prevent.
#[test]
fn a_rupee_quoted_trade_lands_on_the_balance_in_dollars() {
    let mut e = setup();
    let [_, inr, _] = e.prices(108_543_000, INR_M, GBP_M);
    let no_conversion = Legs {
        conversion: None,
        ..inr
    };
    refused(
        e.open(
            USDINR,
            no_conversion,
            Direction::Long,
            THOUSAND,
            INR - INR / 100,
        ),
        "MissingQuoteConversionPriceUpdate",
    );
    e.open(USDINR, inr, Direction::Long, THOUSAND, INR - INR / 100)
        .expect("opens with its conversion leg");

    e.env.advance_clock(3_600);
    let [_, inr, _] = e.prices(108_543_000, INR_M + INR_M / 100, GBP_M);
    e.close(USDINR, inr).expect("closes");
    let s = e.state();
    let gained = s.balance - i64::try_from(TEN_K).unwrap();
    assert!(
        (5_000_000..10_000_000).contains(&gained),
        "a 1% move on $1,000 nets a few dollars after costs, got {gained}"
    );
    assert_eq!((s.trades, s.wins), (1, 1));
}

/// **A synthetic pair, priced from two feeds.** Without the second leg it is refused by name.
#[test]
fn a_synthetic_pair_needs_and_uses_its_second_leg() {
    let mut e = setup();
    let [_, _, gbp] = e.prices(108_543_000, INR_M, GBP_M);
    let one_leg = Legs {
        secondary: None,
        ..gbp
    };
    let stop = EURGBP_SPOT - EURGBP_SPOT / 100;
    refused(
        e.open(EURGBP, one_leg, Direction::Long, THOUSAND, stop),
        "MissingSecondaryPriceUpdate",
    );
    e.open(EURGBP, gbp, Direction::Long, THOUSAND, stop)
        .expect("opens on both legs");
    let pos: noxfunds::state::VirtualPosition = e.env.read(&vpos_pda(&e.evaluation(), EURGBP, 0));
    // The fill is the composed price plus the buy side's spread: above the mid, and close to it.
    assert!(pos.entry_price > EURGBP_SPOT);
    assert!(pos.entry_price < EURGBP_SPOT + EURGBP_SPOT / 100);
}

/// **The crank takes each position's own legs, in its own number.** A direct, a converted and a
/// synthetic position marked together; a group short of one leg is refused, and so is a mark
/// that hands a direct market a leg it does not use.
#[test]
fn the_crank_marks_a_mixed_book_and_refuses_a_missing_leg() {
    let mut e = setup();
    let [eur, inr, gbp] = e.prices(108_543_000, INR_M, GBP_M);
    e.open(EURUSD, eur, Direction::Long, THOUSAND, EUR - EUR / 100)
        .unwrap();
    e.open(USDINR, inr, Direction::Short, THOUSAND, INR + INR / 100)
        .unwrap();
    e.open(
        EURGBP,
        gbp,
        Direction::Long,
        THOUSAND,
        EURGBP_SPOT - EURGBP_SPOT / 100,
    )
    .unwrap();

    let [eur, inr, gbp] = e.prices(108_543_000, INR_M, GBP_M);
    // The converted group without its conversion leg: the next position's market is read where
    // the leg should be, and the shape no longer adds up.
    let short = Legs {
        conversion: None,
        ..inr
    };
    assert!(e
        .observe(&[(EURUSD, eur), (USDINR, short), (EURGBP, gbp)])
        .is_err());
    // A direct market handed a spare leg: the group shapes drift and the mark is refused.
    let spare = Legs {
        conversion: Some(inr.primary),
        ..eur
    };
    assert!(e
        .observe(&[(EURUSD, spare), (USDINR, inr), (EURGBP, gbp)])
        .is_err());

    e.observe(&[(EURUSD, eur), (USDINR, inr), (EURGBP, gbp)])
        .expect("every group with exactly its market's legs");
    let s = e.state();
    assert_eq!(s.open_positions, 3);
    assert!(
        s.last_equity > 0 && s.last_equity < TEN_K,
        "costs only, nothing moved"
    );
    e.env.assert_invariants();
}
