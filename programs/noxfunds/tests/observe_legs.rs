//! The equity crank on markets that need more than one oracle leg.
//!
//! `observe_mandate_equity` once took a fixed `(position, market, price)` triple per open
//! position and called `load_validated_price` with no extra legs. That is correct for a direct,
//! USD-quoted market and wrong for every other kind: a synthetic pair needs its second leg and a
//! non-USD-quoted pair needs its conversion leg, and `load_validated_price` refuses to price
//! either without them. So any mandate holding EUR/JPY or USD/INR — both listed on devnet — could
//! not be marked at all, and its drawdown rule could not be judged for as long as that position
//! stayed open. A rule that cannot be checked is not a rule.
//!
//! These tests are the first in this crate to open a funded position on anything but EUR/USD.
//! Every existing test used a direct USD market, which is precisely why the triple went
//! unnoticed: the one shape that worked was the only shape anything exercised.
//!
//! The legs are validated by `load_validated_price` itself, against the market's own configured
//! feed ids, so a crank that supplies the wrong feed is refused rather than trusted — asserted
//! below, because that is what makes a variable-length group safe to accept from a stranger.

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
use solana_signer::Signer;
use solfx_core::state::Direction;

const MARKET_0: u128 = 1;

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

/// The three shapes a market can take, beyond the one every other test uses.
#[derive(Clone, Copy)]
enum Shape {
    /// EUR/GBP composed from EUR/USD ÷ GBP/USD: a second leg, no conversion.
    Synthetic,
    /// USD/INR: one leg, but PnL lands in Rupees and needs converting.
    NonUsdQuote,
    /// Both at once — the widest group the crank can be handed, five accounts per position.
    Widest,
}

impl Shape {
    fn spec(self) -> MarketSpec {
        match self {
            Shape::Synthetic => MarketSpec::eur_gbp_synthetic(),
            Shape::NonUsdQuote => MarketSpec::usd_inr(),
            Shape::Widest => MarketSpec::widest(),
        }
    }

    /// The composed spot at `PriceSpec::default()`, in `PRICE_PRECISION`.
    ///
    /// Every leg is posted from the same spec, so a synthetic with `invert_quote` is base ÷ quote
    /// of two equal numbers — exactly 1.0. A direct market is simply the posted price. Computed
    /// from the composition rule rather than read back from a trade, so a wrong stop here would
    /// surface as a refused open instead of a quietly different test.
    fn spot(self) -> i64 {
        match self {
            Shape::Synthetic | Shape::Widest => 1_000_000_000,
            Shape::NonUsdQuote => 1_085_430_000,
        }
    }
}

/// Every leg a position on this market needs, freshly posted.
struct Legs {
    primary: Pubkey,
    secondary: Option<Pubkey>,
    conversion: Option<Pubkey>,
}

struct Nox {
    env: Env,
    shape: Shape,
    trader: solana_keypair::Keypair,
    mandate: Pubkey,
    signer: Pubkey,
}

fn rules() -> MandateRules {
    MandateRules {
        max_trade_notional: 5_000 * ONE_USDC,
        max_total_notional: 10_000 * ONE_USDC,
        max_drawdown_bps: 1_000,
        max_daily_loss_bps: 500,
        max_risk_per_trade_bps: 500,
        max_stop_distance_bps: 500,
        max_concurrent_positions: 3,
        allowed_markets: MARKET_0,
        min_hold_slots: 0,
    }
}

fn setup(shape: Shape) -> Nox {
    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
    let upgrade_authority = env.admin.pubkey();
    support::set_upgrade_authority(&mut env, Some(upgrade_authority));

    env.init_protocol();
    env.list_and_activate(0, &shape.spec());
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
            principal: 10_000 * ONE_USDC,
            rules: rules(),
        }
        .data(),
    };
    env.send(ix, &[&investor, &trader]).unwrap();

    // Into SolFX: create the mandate's account there and move collateral across.
    env.svm.airdrop(&signer, 1_000_000_000).unwrap();
    let user_account = Env::user_pda(&signer);
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::CreateSolfxAccount {
            payer: investor.pubkey(),
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
    env.send(ix, &[&investor]).expect("create solfx account");
    env.track_user(user_account);

    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundSolfxCollateral {
            payer: investor.pubkey(),
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
            amount: 2_000 * ONE_USDC,
        }
        .data(),
    };
    env.send(ix, &[&investor]).expect("fund collateral");

    Nox {
        env,
        shape,
        trader,
        mandate,
        signer,
    }
}

impl Nox {
    /// Post every leg this market needs, at the default price, from the market's own feeds.
    ///
    /// Per shape rather than one `post_widest_now` for all: USD/INR's primary is the INR feed,
    /// not EUR/USD. An earlier draft posted EUR/USD as every market's primary, and the program
    /// rejected the USD/INR open with `WrongOracleFeed` — correctly. The client was wrong.
    fn legs(&mut self) -> Legs {
        let spec = PriceSpec::default();
        match self.shape {
            Shape::Synthetic => Legs {
                primary: self.env.post_price_now(FEED_EUR_USD, spec),
                secondary: Some(self.env.post_price_now(FEED_GBP_USD, spec)),
                conversion: None,
            },
            // The conversion leg is the same feed as the primary: a Rupee-quoted pair converts
            // back to USD by its own rate. One posted account serves both.
            Shape::NonUsdQuote => {
                let inr = self.env.post_price_now(FEED_USD_INR, spec);
                Legs {
                    primary: inr,
                    secondary: None,
                    conversion: Some(inr),
                }
            }
            Shape::Widest => {
                let (primary, secondary, conversion) = self.env.post_widest_now(spec);
                Legs {
                    primary,
                    secondary: Some(secondary),
                    conversion: Some(conversion),
                }
            }
        }
    }

    /// Open a long with a stop 30 bps below the composed spot.
    fn open(&mut self, nonce: u8) -> TestResult {
        let legs = self.legs();
        let spot = self.shape.spot();
        let stop = spot - (spot * 30) / 10_000;
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
                price_update: legs.primary,
                secondary_price_update: legs.secondary,
                quote_conversion_price_update: legs.conversion,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::FundedOpenPosition {
                market_index: 0,
                nonce,
                direction: Direction::Long,
                size_base: ONE_LOT / 100,
                collateral: 300 * ONE_USDC,
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

    /// The crank, handed exactly the groups given. Each group is appended as-is, so a test can
    /// build a correct group, a short one, or one with a leg from the wrong feed.
    fn observe_groups(&mut self, groups: &[Vec<Pubkey>]) -> TestResult {
        let observer = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&observer.pubkey(), 1_000_000_000)
            .unwrap();
        let mut metas = noxfunds::accounts::ObserveMandateEquity {
            trader_profile: profile_pda(&self.trader.pubkey()),
            observer: observer.pubkey(),
            mandate: self.mandate,
            mandate_vault: support::vault_pda(&self.mandate),
            user_account: Env::user_pda(&self.signer),
        }
        .to_account_metas(None);
        for group in groups {
            for key in group {
                metas.push(AccountMeta::new_readonly(*key, false));
            }
        }
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: metas,
            data: noxfunds::instruction::ObserveMandateEquity {}.data(),
        };
        self.env.send(ix, &[&observer])
    }

    /// The group the program expects for one position: position, market, primary, then the
    /// secondary leg if the market is synthetic, then the conversion leg if it is not USD-quoted.
    fn group(&mut self, nonce: u8) -> Vec<Pubkey> {
        let legs = self.legs();
        self.group_with(nonce, &legs)
    }

    /// The same group, built from legs posted once and shared.
    ///
    /// This is how a real keeper builds a crank: the devnet poster keeps one fixed account per
    /// feed (`price-accounts.json`), so every position on a market names the same price
    /// accounts. `group` posts fresh ones each call, which is right for a single-position test
    /// and wrong for measuring a many-position transaction — an earlier draft did exactly that
    /// and measured 1,491 bytes for a case whose keys were never actually shared.
    fn group_with(&self, nonce: u8, legs: &Legs) -> Vec<Pubkey> {
        let user_account = Env::user_pda(&self.signer);
        let mut g = vec![
            Env::position_pda(&user_account, 0, nonce),
            Env::market_pda(0),
            legs.primary,
        ];
        g.extend(legs.secondary);
        g.extend(legs.conversion);
        g
    }

    fn mandate(&self) -> noxfunds::state::Mandate {
        self.env.read(&self.mandate)
    }
}

/// Assert a refusal names one of the expected codes. `expect_err` alone would pass on any
/// failure, including a mis-built transaction that never reached the rule under test.
fn refused(r: TestResult, any_of: &[&str]) {
    let err = r.expect_err("expected a refusal");
    assert!(
        any_of.iter().any(|c| err.contains(c)),
        "refused, but not for {any_of:?}:\n{err}"
    );
}

/// Open one position on `shape`, crank it with the full group, and check the crank recorded.
fn marks(shape: Shape) {
    let mut nox = setup(shape);
    nox.open(0).expect("a funded open on this market shape");

    let before = nox.mandate();
    assert_eq!(before.last_observed_at, 0, "nothing has observed it yet");

    let group = nox.group(0);
    nox.observe_groups(&[group])
        .expect("the crank marks a position whose market needs the extra legs");

    let after = nox.mandate();
    assert!(after.last_observed_at > 0, "the observation was recorded");
    assert!(
        after.last_equity > 0,
        "equity was measured, not left at zero"
    );
    assert_eq!(after.open_positions, 1);
    nox.env.assert_invariants();
}

#[test]
fn a_synthetic_position_can_be_marked() {
    marks(Shape::Synthetic);
}

#[test]
fn a_non_usd_quoted_position_can_be_marked() {
    marks(Shape::NonUsdQuote);
}

#[test]
fn the_widest_position_can_be_marked() {
    marks(Shape::Widest);
}

#[test]
fn several_widest_positions_are_marked_in_one_crank() {
    let mut nox = setup(Shape::Widest);
    for nonce in 0..3u8 {
        nox.open(nonce).expect("open");
    }
    let groups: Vec<Vec<Pubkey>> = (0..3u8).map(|n| nox.group(n)).collect();
    nox.observe_groups(&groups)
        .expect("three five-account groups in one crank");
    assert_eq!(nox.mandate().open_positions, 3);
    nox.env.assert_invariants();
}

/// The old shape. Before the fix this was the *only* shape the crank accepted, and for a
/// synthetic market it could never succeed; now it is refused with a named reason instead.
#[test]
fn the_old_triple_is_refused_for_a_synthetic_market() {
    let mut nox = setup(Shape::Synthetic);
    nox.open(0).unwrap();
    let mut group = nox.group(0);
    group.truncate(3);
    refused(nox.observe_groups(&[group]), &["IncompleteObservation"]);
}

#[test]
fn a_missing_conversion_leg_is_refused() {
    let mut nox = setup(Shape::NonUsdQuote);
    nox.open(0).unwrap();
    let mut group = nox.group(0);
    group.truncate(3);
    refused(nox.observe_groups(&[group]), &["IncompleteObservation"]);
}

/// The safety argument for accepting variable-length groups from a stranger. A crank that hands
/// in a real, guardian-signed update for the **wrong** feed as the second leg is refused by
/// `load_validated_price`, which checks every leg against the market's own configured feed id.
#[test]
fn a_leg_from_the_wrong_feed_is_refused() {
    let mut nox = setup(Shape::Synthetic);
    nox.open(0).unwrap();
    let mut group = nox.group(0);
    // Position, market, primary, secondary: swap the GBP/USD leg for the EUR/USD one.
    group[3] = group[2];
    refused(nox.observe_groups(&[group]), &["WrongOracleFeed"]);
}

/// And the other direction: an extra leg on a market that needs none is not silently ignored.
/// With the group length decided by the market, the surplus account becomes the start of a
/// phantom next group, which must not be allowed to pass as a position.
#[test]
fn a_surplus_leg_is_not_silently_accepted() {
    let mut nox = setup(Shape::NonUsdQuote);
    nox.open(0).unwrap();
    let mut group = nox.group(0);
    let extra = group[2];
    group.push(extra);
    nox.observe_groups(&[group])
        .expect_err("a trailing account the market does not use must be refused");
}

// --- how many positions one crank can carry -----------------------------------------------

const PACKET_LIMIT: usize = 1_232;

/// Wire size of a crank over `groups`, built the way a keeper sends it — compute-budget
/// instructions included, because a crank without a priority fee does not land in the
/// congestion that makes marking urgent.
fn crank_bytes(nox: &Nox, groups: &[Vec<Pubkey>], keeper: &solana_keypair::Keypair) -> usize {
    let mut metas = noxfunds::accounts::ObserveMandateEquity {
        trader_profile: profile_pda(&nox.trader.pubkey()),
        observer: keeper.pubkey(),
        mandate: nox.mandate,
        mandate_vault: support::vault_pda(&nox.mandate),
        user_account: Env::user_pda(&nox.signer),
    }
    .to_account_metas(None);
    for group in groups {
        for key in group {
            metas.push(AccountMeta::new_readonly(*key, false));
        }
    }
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: metas,
        data: noxfunds::instruction::ObserveMandateEquity {}.data(),
    };
    nox.env.measure_keeper_tx(ix, keeper)
}

/// **The crank's real ceiling, measured in both of the shapes a mandate can take.**
///
/// A legacy transaction lists each account key once, however many times the instruction names
/// it. So the size of a crank depends on how many *distinct* markets its positions span, not on
/// how many positions there are:
///
/// - **One market, many positions** — the market and its three price legs repeat, so each extra
///   position costs one new key. This is the common case, and it fits at `MAX_SLOTS`.
/// - **Every position on a different three-leg market** — five new keys per position. This is
///   the worst the state can produce, and it does **not** fit at `MAX_SLOTS`.
///
/// The second case is recorded, not hidden. No three-leg market is listed on devnet — `widest()`
/// is a test shape — so a live mandate cannot reach it today. If one is ever listed, a keeper
/// marking such a mandate must send a v0 transaction with a lookup table, where each repeated
/// key costs one byte instead of thirty-two. The program is indifferent to the transaction
/// format; this is a limit on the client, and it is stated here so it is not discovered live.
#[test]
fn the_crank_ceiling_is_measured_not_assumed() {
    let mut nox = setup(Shape::Widest);
    let keeper = solana_keypair::Keypair::new();
    let max_slots = noxfunds::state::MAX_SLOTS;

    // Shared: one market, its legs posted once and named by every position — as a keeper
    // reading the poster's fixed per-feed accounts would build it. Positions need not exist for
    // the wire to be measured.
    let legs = nox.legs();
    let shared: Vec<Vec<Pubkey>> = (0..u8::try_from(max_slots).unwrap())
        .map(|n| nox.group_with(n, &legs))
        .collect();
    let shared_bytes = crank_bytes(&nox, &shared, &keeper);

    // Distinct: every key in every group is new — the worst case the state can produce.
    let distinct_at = |n: usize| -> usize {
        let groups: Vec<Vec<Pubkey>> = (0..n)
            .map(|_| (0..5).map(|_| Pubkey::new_unique()).collect())
            .collect();
        crank_bytes(&nox, &groups, &keeper)
    };
    let fits_distinct = (1..=max_slots)
        .take_while(|&n| distinct_at(n) <= PACKET_LIMIT)
        .last()
        .unwrap_or(0);

    println!("\n  observe_mandate_equity, three-leg market (5 accounts a position):");
    println!("    {max_slots} positions, one market:        {shared_bytes} bytes / {PACKET_LIMIT}");
    for n in 1..=max_slots {
        println!(
            "    {n} positions, distinct markets:  {} bytes",
            distinct_at(n)
        );
    }
    println!("    → legacy fits {fits_distinct} distinct three-leg positions of {max_slots}\n");

    assert!(
        shared_bytes <= PACKET_LIMIT,
        "a full mandate on one three-leg market no longer fits a legacy crank: {shared_bytes} bytes"
    );
    // Pinned, so a change in the crank's fixed accounts that moves this ceiling fails here
    // instead of surfacing as a keeper that cannot mark a mandate.
    assert_eq!(
        fits_distinct, 5,
        "the distinct-market ceiling moved to {fits_distinct}; update the documented limit"
    );
}
