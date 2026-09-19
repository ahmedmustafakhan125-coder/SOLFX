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

/// Assert a refusal, and that it is refused for the right reason.
///
/// `expect_err` alone passes on *any* failure — a missing account or a wrong seed would look
/// exactly like the rule working. Each refusal here names the one code it actually produces.
///
/// Measured, not assumed: where the signer's key is part of an account's seeds, a stranger is
/// stopped by `ConstraintSeeds` and the named check behind it (`NotTheOfferTrader`,
/// `NotTheListingInvestor`) never runs. Those checks stay as a second layer, but the tests assert
/// the layer that does the work. Likewise a second acceptance dies on the mandate account already
/// existing, before `OfferNotOpen` is reached.
fn refused(r: TestResult, any_of: &[&str]) {
    let err = r.expect_err("expected a refusal");
    assert!(
        any_of.iter().any(|c| err.contains(c)),
        "refused, but not for {any_of:?}:\n{err}"
    );
}

struct Market {
    env: Env,
    investor: solana_keypair::Keypair,
    trader: solana_keypair::Keypair,
    investor_token: Pubkey,
    /// When set, every instruction is metered instead of merely sent — see the budget test.
    meter: Option<Vec<Metered>>,
}

/// One marketplace instruction as the SVM actually charged it.
struct Metered {
    name: &'static str,
    cu: u64,
    wire: usize,
    accounts: usize,
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
        meter: None,
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
    /// Send, or — when metering — measure the compute charged and the wire size, then send.
    ///
    /// Wire size is taken with `measure_keeper_tx`, the helper `budgets.rs` and `solfx-core`'s
    /// packet test use, so the figures are comparable: it includes the two compute-budget
    /// instructions a real client puts in front.
    fn dispatch(
        &mut self,
        name: &'static str,
        ix: Instruction,
        signers: &[&solana_keypair::Keypair],
    ) -> TestResult {
        if self.meter.is_none() {
            return self.env.send(ix, signers);
        }
        let wire = self.env.measure_keeper_tx(ix.clone(), signers[0]);
        let accounts = ix.accounts.len();
        let cu = self.env.send_metered(ix, signers);
        if let Some(rows) = self.meter.as_mut() {
            rows.push(Metered {
                name,
                cu,
                wire,
                accounts,
            });
        }
        Ok(())
    }

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
        self.dispatch("post_listing", ix, &[&trader])
    }

    fn update_listing(&mut self, t: ListingTerms, open: bool) -> TestResult {
        let trader = self.trader.insecure_clone();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::UpdateListing {
                trader: trader.pubkey(),
                config: config_pda(),
                listing: listing_pda(&trader.pubkey()),
            }
            .to_account_metas(None),
            data: noxfunds::instruction::UpdateListing { terms: t, open }.data(),
        };
        self.dispatch("update_listing", ix, &[&trader])
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
        self.dispatch("post_offer", ix, &[&investor])
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
        self.dispatch("revoke_offer", ix, &[&investor])
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
        self.dispatch("accept_offer", ix, &[who])
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
    // a 100% split leaves the investor bearing loss for no gain

    refused(m.post_listing(t), &["InvalidListingTerms"]);
}

#[test]
fn a_listing_cannot_invert_its_own_principal_band() {
    let mut m = setup();
    let mut t = terms();
    t.min_principal = 50_000 * ONE_USDC;
    t.max_principal = 1_000 * ONE_USDC;
    // max below min is incoherent

    refused(m.post_listing(t), &["InvalidListingTerms"]);
}

#[test]
fn a_note_longer_than_the_field_is_refused_rather_than_truncated() {
    let mut m = setup();
    let mut t = terms();
    t.note = "x".repeat(noxfunds::constants::MAX_NOTE_LEN + 1);
    // truncating could cut a multi-byte character in half

    refused(m.post_listing(t), &["NoteTooLong"]);
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

    // the offer has expired

    refused(m.accept(0), &["OfferExpired"]);
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
    // a second revoke would drain an empty vault

    refused(m.revoke(0), &["OfferNotOpen"]);
}

#[test]
fn an_offer_cannot_be_posted_with_an_expiry_already_in_the_past() {
    let mut m = setup();
    let past = m.env.now - 1;
    // an offer nobody could ever accept is not an offer

    refused(m.post_offer(0, PRINCIPAL, past, ""), &["OfferExpired"]);
}

#[test]
fn an_investor_cannot_offer_capital_they_do_not_hold() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    let too_much = support::INVESTOR_START + 1;
    // the principal has to actually exist

    refused(
        m.post_offer(0, too_much, expiry, ""),
        &["InsufficientPrincipal"],
    );
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

    // an offer is addressed, not open to whoever gets there first

    refused(m.accept_as(&interloper, 0), &["ConstraintSeeds"]);

    assert_eq!(m.offer(0).state, OfferState::Open);
}

#[test]
fn an_accepted_offer_cannot_then_be_revoked() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    m.accept(0).unwrap();

    // the trader has signed; the capital is committed

    refused(m.revoke(0), &["OfferNotOpen"]);
}

#[test]
fn an_offer_cannot_be_accepted_twice() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    m.accept(0).unwrap();
    // the mandate already exists and the escrow is empty

    refused(m.accept(0), &["already in use"]);
}

#[test]
fn an_offer_above_the_traders_tier_ceiling_is_refused() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    // Bronze tops out at $10,000, and a new profile is Bronze.
    // the tier bounds the mandate before the capital moves

    refused(
        m.post_offer(0, 20_000 * ONE_USDC, expiry, ""),
        &["MandateExceedsTierLimit"],
    );
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

// --- the conversation -------------------------------------------------------------------------

fn investor_listing_pda(investor: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::INVESTOR_LISTING_SEED,
            investor.as_ref(),
        ],
        &noxfunds::ID,
    )
    .0
}
fn request_pda(trader: &Pubkey, investor: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::REQUEST_SEED,
            trader.as_ref(),
            investor.as_ref(),
        ],
        &noxfunds::ID,
    )
    .0
}

fn investor_terms() -> noxfunds::instructions::marketplace::InvestorListingTerms {
    noxfunds::instructions::marketplace::InvestorListingTerms {
        min_principal: 1_000 * ONE_USDC,
        max_principal: 25_000 * ONE_USDC,
        max_drawdown_bps: 600,
        max_risk_per_trade_bps: 100,
        allowed_markets: MARKET_0,
        offered_split_bps: 7_000,
        note: "Looking for patient FX swing traders.".to_string(),
    }
}

impl Market {
    fn decline_as(&mut self, who: &solana_keypair::Keypair, seq: u8, reason: &str) -> TestResult {
        let offer = offer_pda(&self.investor.pubkey(), &self.trader.pubkey(), seq);
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::DeclineOffer {
                trader: who.pubkey(),
                offer,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::DeclineOffer {
                reason: reason.to_string(),
            }
            .data(),
        };
        self.dispatch("decline_offer", ix, &[who])
    }

    fn decline(&mut self, seq: u8, reason: &str) -> TestResult {
        let trader = self.trader.insecure_clone();
        self.decline_as(&trader, seq, reason)
    }

    fn post_investor_listing(
        &mut self,
        t: noxfunds::instructions::marketplace::InvestorListingTerms,
    ) -> TestResult {
        let investor = self.investor.insecure_clone();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::PostInvestorListing {
                investor: investor.pubkey(),
                config: config_pda(),
                listing: investor_listing_pda(&investor.pubkey()),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::PostInvestorListing { terms: t }.data(),
        };
        self.dispatch("post_investor_listing", ix, &[&investor])
    }

    fn update_investor_listing_as(
        &mut self,
        who: &solana_keypair::Keypair,
        t: noxfunds::instructions::marketplace::InvestorListingTerms,
        open: bool,
    ) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::UpdateInvestorListing {
                investor: who.pubkey(),
                config: config_pda(),
                listing: investor_listing_pda(&self.investor.pubkey()),
            }
            .to_account_metas(None),
            data: noxfunds::instruction::UpdateInvestorListing { terms: t, open }.data(),
        };
        self.dispatch("update_investor_listing", ix, &[who])
    }

    fn post_request(&mut self, principal: u64, note: &str) -> TestResult {
        let trader = self.trader.insecure_clone();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::PostRequest {
                trader: trader.pubkey(),
                config: config_pda(),
                trader_profile: profile_pda(&trader.pubkey()),
                investor_listing: investor_listing_pda(&self.investor.pubkey()),
                request: request_pda(&trader.pubkey(), &self.investor.pubkey()),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::PostRequest {
                wanted_principal: principal,
                wanted_split_bps: 7_000,
                note: note.to_string(),
            }
            .data(),
        };
        self.dispatch("post_request", ix, &[&trader])
    }

    fn close_request_as(&mut self, who: &solana_keypair::Keypair) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::CloseRequest {
                closer: who.pubkey(),
                trader: self.trader.pubkey(),
                request: request_pda(&self.trader.pubkey(), &self.investor.pubkey()),
            }
            .to_account_metas(None),
            data: noxfunds::instruction::CloseRequest {}.data(),
        };
        self.dispatch("close_request", ix, &[who])
    }

    fn lamports(&self, key: &Pubkey) -> u64 {
        self.env.svm.get_account(key).map_or(0, |a| a.lamports)
    }

    fn stranger(&mut self) -> solana_keypair::Keypair {
        let k = solana_keypair::Keypair::new();
        self.env
            .svm
            .airdrop(&k.pubkey(), 10 * 1_000_000_000)
            .unwrap();
        k
    }
}

#[test]
fn a_trader_can_decline_with_a_reason_and_the_investor_still_gets_every_unit_back() {
    let mut m = setup();
    let before = m.env.token_balance(&m.investor_token);
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "10k, EUR/USD only")
        .unwrap();

    m.decline(0, "EUR/USD only is too narrow; I trade the yen crosses")
        .expect("the addressed trader may refuse");

    let o = m.offer(0);
    assert_eq!(o.state, OfferState::Declined);
    let len = usize::from(o.reply_len);
    assert_eq!(
        core::str::from_utf8(&o.reply[..len]).unwrap(),
        "EUR/USD only is too narrow; I trade the yen crosses"
    );
    // Declining is a message, not a movement: the capital is still in escrow.
    let offer = offer_pda(&m.investor.pubkey(), &m.trader.pubkey(), 0);
    assert_eq!(m.env.token_balance(&offer_vault_pda(&offer)), PRINCIPAL);

    m.revoke(0)
        .expect("a declined offer must never strand the capital");
    assert_eq!(m.env.token_balance(&m.investor_token), before);
    assert_eq!(m.offer(0).state, OfferState::Revoked);
}

#[test]
fn a_declined_offer_cannot_then_be_accepted() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    m.decline(0, "no thanks").unwrap();
    // a refusal is final for that offer; the investor revises with a new one

    refused(m.accept(0), &["OfferNotOpen"]);
}

#[test]
fn only_the_addressed_trader_may_decline() {
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "").unwrap();
    let stranger = m.stranger();
    // a stranger must not be able to kill someone else's offer

    refused(m.decline_as(&stranger, 0, "griefing"), &["ConstraintSeeds"]);
    assert_eq!(m.offer(0).state, OfferState::Open);
}

#[test]
fn the_full_negotiation_loop_ends_in_a_funded_mandate() {
    // offer → decline with reason → revised offer → accept. The conversation, end to end.
    let mut m = setup();
    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "first terms").unwrap();
    m.decline(0, "split too low").unwrap();
    m.revoke(0).unwrap();
    m.post_offer(1, PRINCIPAL, expiry, "revised: see the split")
        .unwrap();
    m.accept(1).expect("the revised offer is accepted");

    let mandate = mandate_pda(&m.investor.pubkey(), &m.trader.pubkey(), 1);
    assert_eq!(
        m.env.token_balance(&support::vault_pda(&mandate)),
        PRINCIPAL
    );
}

#[test]
fn an_investor_advertises_and_the_terms_are_readable_by_anyone() {
    let mut m = setup();
    m.post_investor_listing(investor_terms()).unwrap();
    let l: noxfunds::state::InvestorListing =
        m.env.read(&investor_listing_pda(&m.investor.pubkey()));
    assert_eq!(l.investor, m.investor.pubkey());
    assert_eq!(l.max_principal, 25_000 * ONE_USDC);
    assert_eq!(l.offered_split_bps, 7_000);
    assert!(l.open);
}

#[test]
fn an_investor_listing_cannot_allow_more_risk_per_trade_than_total_drawdown() {
    let mut m = setup();
    let mut t = investor_terms();
    t.max_risk_per_trade_bps = 700; // above the 600 drawdown: one stop-out would breach
                                    // incoherent terms are refused while they can still be edited

    refused(m.post_investor_listing(t), &["InvalidListingTerms"]);
}

#[test]
fn only_the_investor_may_edit_their_listing() {
    let mut m = setup();
    m.post_investor_listing(investor_terms()).unwrap();
    let stranger = m.stranger();
    // someone else's listing is not yours to close

    refused(
        m.update_investor_listing_as(&stranger, investor_terms(), false),
        &["ConstraintSeeds"],
    );
}

#[test]
fn a_trader_can_request_capital_from_an_investor_who_is_listed() {
    let mut m = setup();
    m.post_investor_listing(investor_terms()).unwrap();
    m.post_request(10_000 * ONE_USDC, "4 years EUR/USD, see my profile")
        .expect("a listed investor can be asked");

    let r: noxfunds::state::FundingRequest = m
        .env
        .read(&request_pda(&m.trader.pubkey(), &m.investor.pubkey()));
    assert_eq!(r.trader, m.trader.pubkey());
    assert_eq!(r.investor, m.investor.pubkey());
    assert_eq!(r.wanted_principal, 10_000 * ONE_USDC);
}

#[test]
fn a_trader_cannot_message_an_investor_who_has_not_listed() {
    let mut m = setup();
    // No listing exists. This is the anti-spam rule: no wallet can be messaged merely for
    // existing.
    // requests only reach investors who said they are looking

    refused(
        m.post_request(10_000 * ONE_USDC, "hi"),
        &["AccountNotInitialized"],
    );
}

#[test]
fn closing_a_listing_shuts_the_door_to_new_requests() {
    let mut m = setup();
    m.post_investor_listing(investor_terms()).unwrap();
    let investor = m.investor.insecure_clone();
    m.update_investor_listing_as(&investor, investor_terms(), false)
        .unwrap();
    // a closed listing accepts no requests

    refused(m.post_request(10_000 * ONE_USDC, "hi"), &["ListingNotOpen"]);
}

#[test]
fn a_trader_cannot_send_the_same_investor_two_requests() {
    let mut m = setup();
    m.post_investor_listing(investor_terms()).unwrap();
    m.post_request(10_000 * ONE_USDC, "first").unwrap();
    // one open request per pair — no flooding an inbox with copies

    refused(
        m.post_request(10_000 * ONE_USDC, "again"),
        &["already in use"],
    );
}

#[test]
fn an_investor_dismissing_a_request_returns_the_rent_to_the_trader() {
    let mut m = setup();
    m.post_investor_listing(investor_terms()).unwrap();
    m.post_request(10_000 * ONE_USDC, "hi").unwrap();

    let request = request_pda(&m.trader.pubkey(), &m.investor.pubkey());
    let rent = m.lamports(&request);
    let trader_before = m.lamports(&m.trader.pubkey());

    let investor = m.investor.insecure_clone();
    m.close_request_as(&investor)
        .expect("the investor may dismiss a request made to them");

    // The investor paid the fee for this transaction, so the trader's gain is exactly the rent.
    assert_eq!(m.lamports(&m.trader.pubkey()), trader_before + rent);
    // Closed: gone, or left with nothing in it.
    assert_eq!(m.lamports(&request), 0);

    // And the pair can talk again.
    m.post_request(12_000 * ONE_USDC, "second try").unwrap();
}

#[test]
fn a_stranger_cannot_close_someone_elses_request() {
    let mut m = setup();
    m.post_investor_listing(investor_terms()).unwrap();
    m.post_request(10_000 * ONE_USDC, "hi").unwrap();
    let stranger = m.stranger();
    // only the two parties may end the exchange

    refused(m.close_request_as(&stranger), &["NotARequestParty"]);
}

// --- the budget -------------------------------------------------------------------------------

/// Ceilings, derived rather than picked. Measured over eight runs on 2026-09-19.
///
/// Two kinds of instruction, and the table says which is which:
///
/// - **Deterministic** — no `init`, every seed checked against a stored bump. Eight runs gave
///   the same figure to the unit, so the ceiling is that figure plus about 30%.
/// - **Searched** — each `init` without a stored bump runs `find_program_address`, which counts
///   down from 255 and pays ~1,500 CU per on-curve miss. With fresh random keys the misses
///   differ every run (spreads of 7,500–10,521 CU were measured). A miss happens with
///   probability ≈ ½, so allowing **12 misses per search** puts a spurious failure at about
///   2⁻¹² per search. The ceiling is the lowest measured figure plus 12 × 1,500 per search.
///
/// Searches per instruction, from its `#[derive(Accounts)]`: `post_listing`,
/// `post_investor_listing` and `post_request` one each (the new account); `post_offer` two
/// (offer, offer vault); `accept_offer` three (mandate, mandate signer, mandate vault).
const MARKET_CU_CEILINGS: &[(&str, u64)] = &[
    // deterministic                   measured
    ("close_request", 6_000),            //  4,629
    ("decline_offer", 7_500),            //  5,790
    ("update_listing", 10_000),          //  7,578
    ("update_investor_listing", 10_000), // 7,642
    ("revoke_offer", 16_000),            // 12,198
    // searched                        lowest measured + 12 × 1,500 × searches
    ("post_investor_listing", 29_000), // 10,534 + 18,000
    ("post_listing", 31_000),          // 12,694 + 18,000
    ("post_request", 33_000),          // 14,639 + 18,000
    ("post_offer", 62_000),            // 25,402 + 36,000
    ("accept_offer", 90_000),          // 35,927 + 54,000
];

/// **Every marketplace instruction, metered, along the whole negotiation loop.**
///
/// Run with `-- --nocapture` for the table.
#[test]
fn every_marketplace_instruction_stays_within_its_budget() {
    let mut m = setup();
    m.meter = Some(Vec::new());

    m.post_listing(terms()).unwrap();
    m.update_listing(terms(), true).unwrap();
    m.post_investor_listing(investor_terms()).unwrap();
    let investor = m.investor.insecure_clone();
    m.update_investor_listing_as(&investor, investor_terms(), true)
        .unwrap();
    m.post_request(10_000 * ONE_USDC, "four years of EUR/USD")
        .unwrap();
    m.close_request_as(&investor).unwrap();

    let expiry = m.env.now + HOUR;
    m.post_offer(0, PRINCIPAL, expiry, "first terms").unwrap();
    m.decline(0, "split too low").unwrap();
    m.revoke(0).unwrap();
    m.post_offer(1, PRINCIPAL, expiry, "revised").unwrap();
    m.accept(1).unwrap();

    let rows = m.meter.take().unwrap();
    println!(
        "\n  {:<26} {:>9} {:>9} {:>9}",
        "instruction", "CU", "bytes", "accounts"
    );
    for r in &rows {
        println!(
            "  {:<26} {:>9} {:>9} {:>9}",
            r.name, r.cu, r.wire, r.accounts
        );
    }

    for (name, ceiling) in MARKET_CU_CEILINGS {
        let r = rows
            .iter()
            .find(|r| r.name == *name)
            .unwrap_or_else(|| panic!("{name} was never metered — the loop above must cover it"));
        assert!(
            r.cu <= *ceiling,
            "{name} consumed {} CU, above its {ceiling} ceiling",
            r.cu
        );
        assert!(
            r.wire <= 1_232,
            "{name} serialises to {} bytes, over the 1,232 packet limit",
            r.wire
        );
        assert!(
            r.accounts <= 64,
            "{name} locks {} accounts, over the 64 limit",
            r.accounts
        );
    }
}
