//! Stage 7 — the marketplace: how an investor and a trader find each other and agree terms.
//!
//! The binding step here is an escrow, not a conversation. So the tests that matter are not
//! "can a listing be posted" but the ones that prove the escrow's guarantees:
//!
//! - the principal is really in escrow from the moment the offer exists, not a claim about money;
//! - the investor can always get it back while nobody has accepted;
//! - only the addressed trader can accept, and only before expiry;
//! - the rules the trader accepts are **byte for byte** the rules the investor published;
//! - acceptance is atomic — mandate, vault and transfer, or nothing.
//!
//! The last test is a regression guard carried over from the reference escrow in
//! `solana-developers/program-examples`, whose own suite documents the bug: a take that derived
//! its amount from a token balance could be bricked for free by any stranger sending one base
//! unit to the right account first. `accept_offer` moves `offer.principal` and never a balance,
//! and the test proves the grief does not work.

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
use noxfunds::instructions::investor::MandateRules;
use noxfunds::instructions::marketplace::ListingTerms;
use noxfunds::state::{MandateOffer, MandateState, OfferState, TraderListing};
use solana_signer::Signer;

const MARKET_0: u128 = 1;
const PRINCIPAL: u64 = 5_000 * ONE_USDC;
/// Comfortably inside the Bronze ceiling of $10,000, so tier is never the thing under test.
const HOUR: i64 = 3_600;

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
fn listing_pda(trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::LISTING_SEED, trader.as_ref()],
        &noxfunds::ID,
    )
    .0
}
fn offer_pda(investor: &Pubkey, trader: &Pubkey, seq: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::OFFER_SEED,
            investor.as_ref(),
            trader.as_ref(),
            &[seq],
        ],
        &noxfunds::ID,
    )
    .0
}
fn offer_vault_pda(offer: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::OFFER_VAULT_SEED, offer.as_ref()],
        &noxfunds::ID,
    )
    .0
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

/// The rule set every offer in this file carries, so a changed field in an acceptance test is
/// unambiguously the program's doing rather than the fixture's.
fn rules() -> MandateRules {
    MandateRules {
        max_trade_notional: 2_000 * ONE_USDC,
        max_total_notional: 4_000 * ONE_USDC,
        max_drawdown_bps: 600,
        max_daily_loss_bps: 300,
        max_risk_per_trade_bps: 100,
        max_stop_distance_bps: 500,
        max_concurrent_positions: 2,
        allowed_markets: MARKET_0,
        min_hold_slots: 0,
    }
}

fn terms() -> ListingTerms {
    ListingTerms {
        min_principal: 1_000 * ONE_USDC,
        max_principal: 50_000 * ONE_USDC,
        wanted_markets: MARKET_0,
        wanted_split_bps: 7_000,
        note: "EUR/USD London session. Happy to start small.".to_string(),
    }
}

struct Market {
    env: Env,
    investor: solana_keypair::Keypair,
    trader: solana_keypair::Keypair,
    investor_token: Pubkey,
}

fn setup() -> Market {
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

    create_profile(&mut env, &admin, &trader.pubkey());

    let investor_token = Pubkey::new_unique();
    env.write_token_account(
        investor_token,
        env.usdc_mint,
        investor.pubkey(),
        support::INVESTOR_START,
    );

    Market {
        env,
        investor,
        trader,
        investor_token,
    }
}

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

impl Market {
    fn post_listing(&mut self, t: ListingTerms) -> TestResult {
        let trader = self.trader.insecure_clone();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::PostListing {
                trader: trader.pubkey(),
                config: config_pda(),
                trader_profile: profile_pda(&trader.pubkey()),
                listing: listing_pda(&trader.pubkey()),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::PostListing { terms: t }.data(),
        };
        self.env.send(ix, &[&trader])
    }

    fn post_offer(&mut self, seq: u8, principal: u64, expires_at: i64, note: &str) -> TestResult {
        let investor = self.investor.insecure_clone();
        let offer = offer_pda(&investor.pubkey(), &self.trader.pubkey(), seq);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::PostOffer {
                investor: investor.pubkey(),
                config: config_pda(),
                trader: self.trader.pubkey(),
                trader_profile: profile_pda(&self.trader.pubkey()),
                offer,
                usdc_mint: self.env.usdc_mint,
                investor_token: self.investor_token,
                offer_vault: offer_vault_pda(&offer),
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::PostOffer {
                seq,
                principal,
                rules: rules(),
                trader_split_bps: 7_000,
                expires_at,
                note: note.to_string(),
            }
            .data(),
        };
        self.env.send(ix, &[&investor])
    }

    fn revoke(&mut self, seq: u8) -> TestResult {
        let investor = self.investor.insecure_clone();
        let offer = offer_pda(&investor.pubkey(), &self.trader.pubkey(), seq);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::RevokeOffer {
                investor: investor.pubkey(),
                offer,
                usdc_mint: self.env.usdc_mint,
                offer_vault: offer_vault_pda(&offer),
                investor_token: self.investor_token,
                token_program: spl_token::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::RevokeOffer {}.data(),
        };
        self.env.send(ix, &[&investor])
    }

    fn accept_as(&mut self, who: &solana_keypair::Keypair, seq: u8) -> TestResult {
        let offer = offer_pda(&self.investor.pubkey(), &self.trader.pubkey(), seq);
        let mandate = mandate_pda(&self.investor.pubkey(), &who.pubkey(), seq);
        let signer = signer_pda(&mandate);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::AcceptOffer {
                trader: who.pubkey(),
                config: config_pda(),
                offer,
                trader_profile: profile_pda(&who.pubkey()),
                mandate,
                mandate_signer: signer,
                solfx_user_account: Env::user_pda(&signer),
                usdc_mint: self.env.usdc_mint,
                offer_vault: offer_vault_pda(&offer),
                mandate_vault: support::vault_pda(&mandate),
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::AcceptOffer {}.data(),
        };
        self.env.send(ix, &[who])
    }

    fn accept(&mut self, seq: u8) -> TestResult {
        let trader = self.trader.insecure_clone();
        self.accept_as(&trader, seq)
    }

    fn offer(&self, seq: u8) -> MandateOffer {
        self.env.read(&offer_pda(
            &self.investor.pubkey(),
            &self.trader.pubkey(),
            seq,
        ))
    }
}

// --- the trader's side -------------------------------------------------------------------

#[test]
fn a_trader_advertises_and_the_terms_are_readable_by_anyone() {
    let mut m = setup();
    m.post_listing(terms()).expect("post a listing");

    let l: TraderListing = m.env.read(&listing_pda(&m.trader.pubkey()));
    assert_eq!(l.trader, m.trader.pubkey());
    assert_eq!(l.min_principal, 1_000 * ONE_USDC);
    assert_eq!(l.max_principal, 50_000 * ONE_USDC);
    assert_eq!(l.wanted_split_bps, 7_000);
    assert!(l.open);
    // The note round-trips as text rather than as padded bytes.
    let len = usize::from(l.note_len);
    assert_eq!(
        core::str::from_utf8(&l.note[..len]).unwrap(),
        "EUR/USD London session. Happy to start small."
    );
}

#[test]
fn a_listing_cannot_ask_for_the_entire_profit() {
    let mut m = setup();
    let mut t = terms();
    t.wanted_split_bps = 10_000;
    m.post_listing(t)
        .expect_err("a 100% split leaves the investor bearing loss for no gain");
}

#[test]
fn a_listing_cannot_invert_its_own_principal_band() {
    let mut m = setup();
    let mut t = terms();
    t.min_principal = 50_000 * ONE_USDC;
    t.max_principal = 1_000 * ONE_USDC;
    m.post_listing(t).expect_err("max below min is incoherent");
}

#[test]
fn a_note_longer_than_the_field_is_refused_rather_than_truncated() {
    let mut m = setup();
    let mut t = terms();
    t.note = "x".repeat(noxfunds::constants::MAX_NOTE_LEN + 1);
    m.post_listing(t)
        .expect_err("truncating could cut a multi-byte character in half");
}

// --- the escrow --------------------------------------------------------------------------

#[test]
fn posting_an_offer_really_moves_the_principal_into_escrow() {
    let mut m = setup();
    let before = m.env.token_balance(&m.investor_token);
    let expiry = m.env.now + HOUR;

    m.post_offer(0, PRINCIPAL, expiry, "terms attached")
        .unwrap();

    let offer = offer_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0);
    // The money left the investor and is sitting in the escrow — not recorded as a number
    // somewhere while the balance stayed put. That distinction is the whole reason
    // `fund_mandate` was rewritten, and it is worth asserting here too.
    assert_eq!(m.env.token_balance(&m.investor_token), before - PRINCIPAL);
    assert_eq!(m.env.token_balance(&offer_vault_pda(&offer)), PRINCIPAL);

    let o = m.offer(0);
    assert_eq!(o.state, OfferState::Open);
    assert_eq!(o.principal, PRINCIPAL);
    assert_eq!(o.investor, m.investor.pubkey());
    assert_eq!(o.trader, m.trader.pubkey());
    assert_eq!(o.note_str(), "terms attached");
}

#[test]
fn an_investor_can_always_take_back_an_offer_nobody_accepted() {
    let mut m = setup();
    let before = m.env.token_balance(&m.investor_token);
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();

    m.revoke(0)
        .expect("the capital is the investor's until signed for");

    assert_eq!(m.env.token_balance(&m.investor_token), before);
    let offer = offer_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0);
    assert_eq!(m.env.token_balance(&offer_vault_pda(&offer)), 0);
    assert_eq!(m.offer(0).state, OfferState::Revoked);
}

#[test]
fn an_expired_offer_can_still_be_revoked_but_not_accepted() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();

    m.env.advance_clock(HOUR + 1);

    m.accept(0).expect_err("the offer has expired");
    // The other half matters more: expiry must never be a way to strand an investor's money.
    m.revoke(0).expect("expiry does not trap the capital");
    assert_eq!(m.offer(0).state, OfferState::Revoked);
}

#[test]
fn a_revoked_offer_cannot_be_revoked_twice() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    m.revoke(0).unwrap();
    m.revoke(0)
        .expect_err("a second revoke would drain an empty vault");
}

#[test]
fn an_offer_cannot_be_posted_with_an_expiry_already_in_the_past() {
    let mut m = setup();
    let past = m.env.now - 1;
    m.post_offer(0, PRINCIPAL, past, "")
        .expect_err("an offer nobody could ever accept is not an offer");
}

#[test]
fn an_investor_cannot_offer_capital_they_do_not_hold() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    let too_much = support::INVESTOR_START + 1;
    m.post_offer(0, too_much, expiry, "")
        .expect_err("the principal has to actually exist");
}

// --- acceptance --------------------------------------------------------------------------

#[test]
fn accepting_creates_the_mandate_and_moves_the_money_in_one_transaction() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "please").unwrap();

    m.accept(0).expect("the addressed trader accepts");

    let mandate = mandate_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0);

    // The escrow is empty and the mandate's own vault holds exactly the principal. No
    // intermediate state exists in which one is true and the other is not.
    let offer = offer_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0);
    assert_eq!(m.env.token_balance(&offer_vault_pda(&offer)), 0);
    assert_eq!(
        m.env.token_balance(&support::vault_pda(&mandate)),
        PRINCIPAL
    );

    let md: noxfunds::state::Mandate = m.env.read(&mandate);
    assert_eq!(md.state, MandateState::Active);
    assert_eq!(md.principal, PRINCIPAL);
    assert_eq!(md.peak_equity, PRINCIPAL);
    assert_eq!(md.investor, m.investor.pubkey());
    assert_eq!(md.trader, m.trader.pubkey());

    assert_eq!(m.offer(0).state, OfferState::Accepted);

    let p: noxfunds::state::TraderProfile = m.env.read(&profile_pda(&m.trader.pubkey()));
    assert_eq!(p.mandates_funded, 1);
    assert_eq!(p.active_mandates, 1);
}

#[test]
fn the_rules_the_trader_accepts_are_the_rules_the_investor_published() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    m.accept(0).unwrap();

    let r = rules();
    let md: noxfunds::state::Mandate =
        m.env
            .read(&mandate_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0));

    // Field by field. A marketplace whose accepted terms can drift from the published ones is
    // worth less than no marketplace, so this is checked exhaustively rather than by sampling.
    assert_eq!(md.max_trade_notional, r.max_trade_notional);
    assert_eq!(md.max_total_notional, r.max_total_notional);
    assert_eq!(md.max_drawdown_bps, r.max_drawdown_bps);
    assert_eq!(md.max_daily_loss_bps, r.max_daily_loss_bps);
    assert_eq!(md.max_risk_per_trade_bps, r.max_risk_per_trade_bps);
    assert_eq!(md.max_stop_distance_bps, r.max_stop_distance_bps);
    assert_eq!(md.max_concurrent_positions, r.max_concurrent_positions);
    assert_eq!(md.allowed_markets, r.allowed_markets);
    assert_eq!(md.min_hold_slots, r.min_hold_slots);
    assert_eq!(md.trader_split_bps, 7_000);
}

#[test]
fn only_the_addressed_trader_may_accept() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();

    // A second trader, fully set up — profile and all — so the refusal is about *identity*
    // rather than about a missing account.
    let interloper = solana_keypair::Keypair::new();
    m.env
        .svm
        .airdrop(&interloper.pubkey(), 100 * 1_000_000_000)
        .unwrap();
    let admin = m.env.admin.insecure_clone();
    create_profile(&mut m.env, &admin, &interloper.pubkey());

    m.accept_as(&interloper, 0)
        .expect_err("an offer is addressed, not open to whoever gets there first");

    assert_eq!(m.offer(0).state, OfferState::Open);
}

#[test]
fn an_accepted_offer_cannot_then_be_revoked() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    m.accept(0).unwrap();

    m.revoke(0)
        .expect_err("the trader has signed; the capital is committed");
}

#[test]
fn an_offer_cannot_be_accepted_twice() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    m.accept(0).unwrap();
    m.accept(0)
        .expect_err("the mandate already exists and the escrow is empty");
}

#[test]
fn an_offer_above_the_traders_tier_ceiling_is_refused() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    // Bronze tops out at $10,000, and a new profile is Bronze.
    m.post_offer(0, 20_000 * ONE_USDC, expiry, "")
        .expect_err("the tier bounds the mandate before the capital moves");
}

// --- the griefing regression ---------------------------------------------------------------

#[test]
fn a_stranger_cannot_brick_an_offer_by_paying_into_its_vault() {
    let mut m = setup();
    let before = m.env.token_balance(&m.investor_token);
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();

    let offer = offer_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0);
    let vault = offer_vault_pda(&offer);

    // A stranger dumps a single extra base unit into the escrow. This is permissionless and
    // free: a token account accepts a transfer from anyone. In the reference escrow's
    // documented bug, a take that computed its amount from the vault balance would then move
    // the wrong number and the offer could never be filled.
    let dust = 1u64;
    m.env
        .write_token_account(vault, m.env.usdc_mint, offer, PRINCIPAL + dust);

    // It still works, because acceptance moves the committed `offer.principal`.
    m.accept(0)
        .expect("a stranger's dust must not be able to brick an offer");

    let mandate = mandate_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0);
    assert_eq!(
        m.env.token_balance(&support::vault_pda(&mandate)),
        PRINCIPAL
    );
    // The dust stays behind rather than being swept into the mandate. The investor committed
    // to `PRINCIPAL`, so `PRINCIPAL` is what the trader gets to trade.
    assert_eq!(m.env.token_balance(&vault), dust);
    let _ = before;
}
