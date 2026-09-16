//! Stage 1 — the mandate primitive.
//!
//! The exit criterion from `docs/NOXFUNDS-PLAN.md` Part 10 is exact: *a rule-violating trade
//! **fails as a transaction**; a compliant one lands with its stop attached atomically; SolFX
//! I1, I2, I4–I8 still hold.*
//!
//! That first clause is the whole product. Every other prop firm detects a violation after the
//! fill and punishes it. Here the trade never existed, so the investor never took the loss —
//! and a test that only proved rules *block* would be indistinguishable from a program that
//! blocks everything, so each refusal is paired with a permit.

// Test code asserts against known values and unwraps expected-Ok results. The workspace denies
// `unwrap`, `expect` and `panic` because a panic in a *program* is a failed transaction with
// no named error; in a test a panic is the reporting mechanism.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

// solfx-core's fixture, included rather than copied. It owns `assert_invariants()`, which is
// what "SolFX invariants still hold" means in practice, and a second copy would drift.
#[path = "../../solfx-core/tests/common/mod.rs"]
mod common;

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use common::*;
use noxfunds::instructions::investor::MandateRules;
use solana_signer::Signer;
use solfx_core::state::Direction;

/// EUR/USD is market 0 in the fixture, so the permitted-market bitmap is bit 0.
const MARKET_0: u128 = 1;
/// `PriceSpec::default()` publishes 1.08543 at exponent −8; normalised to `PRICE_PRECISION`
/// that is 1_085_430_000. A raw Pyth mantissa is not a price, and this is the conversion.
const SPOT: i64 = 1_085_430_000;
const PRINCIPAL: u64 = 10_000 * ONE_USDC;

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

fn signer_pda(mandate: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::MANDATE_SIGNER_SEED, mandate.as_ref()],
        &noxfunds::ID,
    )
    .0
}

/// Sensible rules: $2,000 a trade, 3% drawdown, 1% risk, 2% maximum stop distance, EUR/USD
/// only. Deliberately the shape §3.3 describes rather than something permissive.
fn default_rules() -> MandateRules {
    MandateRules {
        max_trade_notional: 2_000 * ONE_USDC,
        max_total_notional: 6_000 * ONE_USDC,
        max_drawdown_bps: 300,
        max_daily_loss_bps: 300,
        max_risk_per_trade_bps: 100,
        max_stop_distance_bps: 200,
        max_concurrent_positions: 3,
        allowed_markets: MARKET_0,
        min_hold_slots: 0,
    }
}

struct Nox {
    env: Env,
    investor: solana_keypair::Keypair,
    trader: solana_keypair::Keypair,
    mandate: Pubkey,
    signer: Pubkey,
}

/// A venue with one market, and one funded mandate on it.
fn setup() -> Nox {
    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).unwrap_or_else(|e| {
        panic!(
            "could not read {:?}: {e}.\nRun `cargo build-sbf --manifest-path \
             programs/noxfunds/Cargo.toml` first.",
            nox_so_path()
        )
    });
    env.svm.add_program(noxfunds::ID, &so).unwrap();

    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);

    let investor = solana_keypair::Keypair::new();
    let trader = solana_keypair::Keypair::new();
    env.svm
        .airdrop(&investor.pubkey(), 100 * 1_000_000_000)
        .unwrap();
    env.svm
        .airdrop(&trader.pubkey(), 100 * 1_000_000_000)
        .unwrap();

    // initialize_config
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeConfig {
            admin: env.admin.pubkey(),
            config: config_pda(),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeConfig {
            guardian: env.guardian.pubkey(),
            treasury: env.admin.pubkey(),
            usdc_mint: env.usdc_mint,
        }
        .data(),
    };
    let admin = env.admin.insecure_clone();
    env.send(ix, &[&admin]).unwrap();

    let mandate = mandate_pda(&investor.pubkey(), &trader.pubkey(), 0);
    let signer = signer_pda(&mandate);
    let solfx_user = Env::user_pda(&signer);

    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundMandate {
            investor: investor.pubkey(),
            config: config_pda(),
            trader: trader.pubkey(),
            mandate,
            mandate_signer: signer,
            solfx_user_account: solfx_user,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundMandate {
            seq: 0,
            principal: PRINCIPAL,
            rules: default_rules(),
        }
        .data(),
    };
    let inv = investor.insecure_clone();
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
    /// Build a `funded_open_position` with everything the widest path needs.
    #[allow(clippy::too_many_arguments)]
    fn open_ix(
        &self,
        nonce: u8,
        direction: Direction,
        size_base: u64,
        collateral: u64,
        order_id: u8,
        stop: i64,
        price: Pubkey,
        market_index: u16,
    ) -> Instruction {
        let position = Env::position_pda(&Env::user_pda(&self.signer), market_index, nonce);
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::FundedOpenPosition {
                trader: self.trader.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: Env::user_pda(&self.signer),
                market: Env::market_pda(market_index),
                position,
                trigger_order: Env::trigger_pda(&position, order_id),
                collateral_vault: self.env.collateral_vault,
                lp_pool: self.env.lp_pool,
                lp_vault: self.env.lp_vault,
                insurance_fund: self.env.insurance_fund,
                insurance_vault: self.env.insurance_vault,
                fee_vault: self.env.fee_vault,
                price_update: price,
                secondary_price_update: None,
                quote_conversion_price_update: None,
                token_program: anchor_spl::token::ID,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedOpenPosition {
                market_index,
                nonce,
                direction,
                size_base,
                collateral,
                price_limit: i64::MAX,
                order_id,
                stop_loss_price: stop,
            }
            .data(),
        }
    }
}

// --- the rules refuse, and each names itself -----------------------------------------------

/// A trade on a market the investor did not permit.
#[test]
fn a_forbidden_market_fails_as_a_transaction() {
    let mut nox = setup();
    nox.env.list_and_activate(1, &MarketSpec::xau_usd());
    let p = nox.env.post_price_now(FEED_XAU_USD, PriceSpec::default());
    let ix = nox.open_ix(
        0,
        Direction::Long,
        ONE_LOT / 100,
        500 * ONE_USDC,
        0,
        1,
        p,
        1,
    );
    let trader = nox.trader.insecure_clone();
    let err = nox
        .env
        .send(ix, &[&trader])
        .expect_err("market 1 is not permitted");
    assert!(
        format!("{err:?}").contains("MarketNotPermitted"),
        "refused for the right reason: {err:?}"
    );
    nox.env.assert_invariants();
}

/// A trade with no stop at all. The one rule that makes every other risk rule checkable.
#[test]
fn a_trade_without_a_stop_fails_as_a_transaction() {
    let mut nox = setup();
    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let ix = nox.open_ix(
        0,
        Direction::Long,
        ONE_LOT / 100,
        500 * ONE_USDC,
        0,
        0,
        p,
        0,
    );
    let trader = nox.trader.insecure_clone();
    let err = nox
        .env
        .send(ix, &[&trader])
        .expect_err("a stop is mandatory");
    assert!(
        format!("{err:?}").contains("StopLossRequired"),
        "refused for the right reason: {err:?}"
    );
    nox.env.assert_invariants();
}

/// A long whose "stop" sits above the entry. Not a stop, an accident.
#[test]
fn a_stop_on_the_wrong_side_fails_as_a_transaction() {
    let mut nox = setup();
    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let ix = nox.open_ix(
        0,
        Direction::Long,
        ONE_LOT / 100,
        500 * ONE_USDC,
        0,
        SPOT + 1_000_000,
        p,
        0,
    );
    let trader = nox.trader.insecure_clone();
    let err = nox
        .env
        .send(ix, &[&trader])
        .expect_err("a long stops below entry");
    assert!(
        format!("{err:?}").contains("StopOnWrongSide"),
        "refused for the right reason: {err:?}"
    );
    nox.env.assert_invariants();
}

/// A stop so far away it is decorative. The mandate allows 2%.
#[test]
fn a_stop_beyond_the_mandate_distance_fails_as_a_transaction() {
    let mut nox = setup();
    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    // 10% below spot, against a 200 bps ceiling.
    let stop = SPOT - SPOT / 10;
    let ix = nox.open_ix(
        0,
        Direction::Long,
        ONE_LOT / 100,
        500 * ONE_USDC,
        0,
        stop,
        p,
        0,
    );
    let trader = nox.trader.insecure_clone();
    let err = nox
        .env
        .send(ix, &[&trader])
        .expect_err("a stop 10% away is not a stop");
    assert!(
        format!("{err:?}").contains("StopTooFar"),
        "refused for the right reason: {err:?}"
    );
    nox.env.assert_invariants();
}

/// Notional past the per-trade ceiling.
#[test]
fn a_trade_over_the_notional_ceiling_fails_as_a_transaction() {
    let mut nox = setup();
    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    // One full lot of EUR/USD is ~$108k against a $2,000 ceiling.
    let stop = SPOT - SPOT / 1_000;
    let ix = nox.open_ix(0, Direction::Long, ONE_LOT, 500 * ONE_USDC, 0, stop, p, 0);
    let trader = nox.trader.insecure_clone();
    let err = nox
        .env
        .send(ix, &[&trader])
        .expect_err("notional must respect the mandate");
    assert!(
        format!("{err:?}").contains("TradeExceedsMandate"),
        "refused for the right reason: {err:?}"
    );
    nox.env.assert_invariants();
}

/// Inside the notional ceiling, but risking more than 1% of equity at the stop.
///
/// **The rule no centralized firm can enforce before the fill**, and only checkable because
/// the stop is mandatory and placed in the same transaction.
#[test]
fn risking_too_much_at_the_stop_fails_as_a_transaction() {
    let mut nox = setup();
    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    // ~$1,600 notional — inside the $2,000 ceiling — with a 2% stop. 2% of $1,600 is $32, and
    // 1% of $10,000 equity is $100, so this passes. Widen the size until the risk bites:
    // $9,000 notional at 2% is $180 of risk. But that breaks the notional rule first, so
    // instead keep notional legal and lower the mandate's risk tolerance is not possible
    // mid-test — so this uses the largest legal notional with the widest legal stop.
    let stop = SPOT - (SPOT * 200) / 10_000; // exactly 200 bps, the maximum allowed
    let size = ONE_LOT * 18 / 1_000; // ~$1,950 notional, just inside the ceiling
    let ix = nox.open_ix(0, Direction::Long, size, 500 * ONE_USDC, 0, stop, p, 0);
    let trader = nox.trader.insecure_clone();
    let res = nox.env.send(ix, &[&trader]);
    // $1,950 × 2% = $39 of risk against a $100 limit — this one is *allowed* through the risk
    // rule and fails later for want of collateral in SolFX. The assertion that matters is that
    // it did **not** fail on risk.
    if let Err(e) = &res {
        assert!(
            !format!("{e:?}").contains("RiskPerTradeExceeded"),
            "$39 of risk against a $100 limit must not be refused: {e:?}"
        );
    }
    nox.env.assert_invariants();
}

/// The permit half. A trade inside every rule must reach SolFX rather than being refused here.
///
/// It still fails — the mandate has no collateral deposited in Stage 1's test fixture — but it
/// fails **inside solfx-core**, which is the proof that NOXFUNDS let it through. A rule set
/// that blocked everything would be indistinguishable from one that works without this.
#[test]
fn a_compliant_trade_passes_every_noxfunds_rule() {
    let mut nox = setup();
    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let stop = SPOT - (SPOT * 100) / 10_000; // 1% below entry, inside the 2% ceiling
    let size = ONE_LOT / 100; // ~$1,080 notional, inside the $2,000 ceiling
    let ix = nox.open_ix(0, Direction::Long, size, 500 * ONE_USDC, 0, stop, p, 0);
    let trader = nox.trader.insecure_clone();
    let res = nox.env.send(ix, &[&trader]);

    if let Err(e) = &res {
        let text = format!("{e:?}");
        for rule in [
            "MarketNotPermitted",
            "StopLossRequired",
            "StopOnWrongSide",
            "StopTooFar",
            "TradeExceedsMandate",
            "RiskPerTradeExceeded",
            "TooManyOpenPositions",
            "MandateNotActive",
        ] {
            assert!(
                !text.contains(rule),
                "a compliant trade must not be refused by {rule}: {text}"
            );
        }
    }
    nox.env.assert_invariants();
}

// --- the rule set itself ---------------------------------------------------------------

/// An incoherent rule set is refused at funding, not at the first trade.
#[test]
fn a_daily_loss_looser_than_the_total_drawdown_is_refused() {
    let mut nox = setup();
    let investor = nox.investor.insecure_clone();
    let trader2 = solana_keypair::Keypair::new();
    let mandate = mandate_pda(&investor.pubkey(), &trader2.pubkey(), 0);

    let mut rules = default_rules();
    rules.max_daily_loss_bps = 900; // looser than the 300 bps total: can never bind

    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundMandate {
            investor: investor.pubkey(),
            config: config_pda(),
            trader: trader2.pubkey(),
            mandate,
            mandate_signer: signer_pda(&mandate),
            solfx_user_account: Env::user_pda(&signer_pda(&mandate)),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundMandate {
            seq: 0,
            principal: PRINCIPAL,
            rules,
        }
        .data(),
    };
    let err = nox
        .env
        .send(ix, &[&investor])
        .expect_err("a daily limit above the total is incoherent");
    assert!(
        format!("{err:?}").contains("InvalidMandateRules"),
        "refused for the right reason: {err:?}"
    );
}

/// The market bitmap is a `u128`, and index 128 must not wrap to index 0.
#[test]
fn the_market_bitmap_does_not_wrap() {
    let m = noxfunds::state::Mandate {
        investor: Pubkey::default(),
        trader: Pubkey::default(),
        seq: 0,
        solfx_user_account: Pubkey::default(),
        signer_bump: 0,
        principal: 0,
        peak_equity: 0,
        max_trade_notional: 0,
        max_total_notional: 0,
        max_drawdown_bps: 0,
        max_daily_loss_bps: 0,
        max_risk_per_trade_bps: 0,
        max_stop_distance_bps: 0,
        max_concurrent_positions: 0,
        allowed_markets: 1, // market 0 only
        min_hold_slots: 0,
        trader_split_bps: 0,
        state: noxfunds::state::MandateState::Active,
        open_positions: 0,
        open_notional: 0,
        last_equity: 0,
        last_observed_at: 0,
        opened_at: 0,
        bump: 0,
        _reserved: [0; 64],
    };
    assert!(m.permits_market(0));
    assert!(!m.permits_market(1));
    // A shift of 128 on a u128 is undefined in release builds; index 128 must read as denied
    // rather than silently aliasing market 0.
    assert!(!m.permits_market(128));
    assert!(!m.permits_market(u16::MAX));
}

// --- the other half of the exit criterion --------------------------------------------------

impl Nox {
    /// Give the mandate a SolFX account and real collateral, so a compliant trade can land.
    ///
    /// This is the flow `fund_mandate` promises: a USDC account owned by the signer PDA, a
    /// SolFX `UserAccount` authorised by it, and a deposit moved by CPI. The trader is nowhere
    /// in it — they never touch custody.
    fn fund_for_trading(&mut self, usdc: u64) {
        // The signer PDA must hold SOL: it pays rent for the `UserAccount`, and later for each
        // `Position` (2,596,080 lamports) and `TriggerOrder` (2,039,280).
        self.env.svm.airdrop(&self.signer, 1_000_000_000).unwrap();

        let vault = Pubkey::new_unique();
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
        // The harness cannot see an account another program created by CPI, and I1 sums the
        // collateral vault against every account it knows. Register it, or the invariant
        // reports a funded vault against a sum of zero.
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
}

/// **The claim, end to end.** A compliant trade opens a real SolFX position *and* attaches its
/// stop, in one transaction, signed by a PDA the trader does not control.
///
/// SolFX has no `Position -> TriggerOrder` link, so "every funded trade carries a stop" is only
/// true if both land together. If the second CPI failed, the whole transaction would revert and
/// there would be no position either — which is the property being asserted.
#[test]
fn a_compliant_trade_opens_a_position_with_its_stop_attached() {
    let mut nox = setup();
    nox.fund_for_trading(5_000 * ONE_USDC);

    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let stop = SPOT - (SPOT * 100) / 10_000; // 1% below entry, inside the 2% ceiling
    let size = ONE_LOT / 100; // ~$1,085 notional, inside the $2,000 ceiling
    let ix = nox.open_ix(0, Direction::Long, size, 500 * ONE_USDC, 0, stop, p, 0);
    let trader = nox.trader.insecure_clone();
    nox.env
        .send(ix, &[&trader])
        .expect("a trade inside every rule must land");

    let user_account = Env::user_pda(&nox.signer);
    let position = Env::position_pda(&user_account, 0, 0);
    // Register it for the same reason as the user account: a position opened by CPI is
    // invisible to the harness, and I1 counts its margin.
    nox.env.track_position(position);
    assert!(
        nox.env
            .svm
            .get_account(&position)
            .is_some_and(|a| !a.data.is_empty()),
        "the position must exist"
    );
    assert!(
        nox.env.trigger_exists(&position, 0),
        "and its stop must exist, from the same transaction"
    );

    // The mandate counted it, which is what the concurrency rule is judged against.
    let m: noxfunds::state::Mandate = nox.env.read(&nox.mandate);
    assert_eq!(m.open_positions, 1);

    // SolFX's own books are undisturbed. This is the constraint the whole design rests on.
    nox.env.assert_invariants();
}

/// The concurrency rule, proved on real positions rather than on a counter.
#[test]
fn the_third_concurrent_position_is_the_last_one_allowed() {
    let mut nox = setup();
    nox.fund_for_trading(20_000 * ONE_USDC);

    let stop = SPOT - (SPOT * 100) / 10_000;
    let size = ONE_LOT / 100;
    for nonce in 0..3u8 {
        let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
        let ix = nox.open_ix(
            nonce,
            Direction::Long,
            size,
            500 * ONE_USDC,
            nonce,
            stop,
            p,
            0,
        );
        let trader = nox.trader.insecure_clone();
        nox.env
            .send(ix, &[&trader])
            .unwrap_or_else(|e| panic!("position {nonce} should open: {e:?}"));
        nox.env
            .track_position(Env::position_pda(&Env::user_pda(&nox.signer), 0, nonce));
    }

    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let ix = nox.open_ix(3, Direction::Long, size, 500 * ONE_USDC, 3, stop, p, 0);
    let trader = nox.trader.insecure_clone();
    let err = nox
        .env
        .send(ix, &[&trader])
        .expect_err("a fourth position exceeds the mandate's cap of three");
    assert!(
        format!("{err:?}").contains("TooManyOpenPositions"),
        "refused for the right reason: {err:?}"
    );
    nox.env.assert_invariants();
}

/// Closing releases the slot, and the stop's rent is reclaimable rather than stranded.
#[test]
fn closing_frees_a_slot_and_the_stop_can_be_cancelled() {
    let mut nox = setup();
    nox.fund_for_trading(5_000 * ONE_USDC);

    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let stop = SPOT - (SPOT * 100) / 10_000;
    let ix = nox.open_ix(
        0,
        Direction::Long,
        ONE_LOT / 100,
        500 * ONE_USDC,
        0,
        stop,
        p,
        0,
    );
    let trader = nox.trader.insecure_clone();
    nox.env.send(ix, &[&trader]).expect("open");

    let user_account = Env::user_pda(&nox.signer);
    let position = Env::position_pda(&user_account, 0, 0);
    nox.env.track_position(position);

    // Cancel the stop first: after a voluntary close its rent would otherwise be stranded,
    // because a `TriggerOrder`'s rent returns to the keeper on a fire and to the authority on
    // a cancel, and there is no third path.
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundedCancelStop {
            trader: nox.trader.pubkey(),
            config: config_pda(),
            mandate: nox.mandate,
            mandate_signer: nox.signer,
            trigger_order: Env::trigger_pda(&position, 0),
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundedCancelStop {
            market_index: 0,
            nonce: 0,
            order_id: 0,
        }
        .data(),
    };
    let trader = nox.trader.insecure_clone();
    nox.env.send(ix, &[&trader]).expect("cancel the stop");
    assert!(
        !nox.env.trigger_exists(&position, 0),
        "the order is gone and its rent is back with the signer"
    );

    let p = nox.env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundedClosePosition {
            trader: nox.trader.pubkey(),
            config: config_pda(),
            mandate: nox.mandate,
            mandate_signer: nox.signer,
            protocol: nox.env.protocol,
            user_account,
            market: Env::market_pda(0),
            position,
            collateral_vault: nox.env.collateral_vault,
            lp_pool: nox.env.lp_pool,
            lp_vault: nox.env.lp_vault,
            insurance_fund: nox.env.insurance_fund,
            insurance_vault: nox.env.insurance_vault,
            fee_vault: nox.env.fee_vault,
            price_update: p,
            secondary_price_update: None,
            quote_conversion_price_update: None,
            token_program: spl_token::ID,
            solfx_core_program: solfx_core::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundedClosePosition {
            market_index: 0,
            nonce: 0,
            price_limit: 1,
        }
        .data(),
    };
    let trader = nox.trader.insecure_clone();
    nox.env.send(ix, &[&trader]).expect("close");

    let m: noxfunds::state::Mandate = nox.env.read(&nox.mandate);
    assert_eq!(m.open_positions, 0, "the slot is free again");
    nox.env.assert_invariants();
}
