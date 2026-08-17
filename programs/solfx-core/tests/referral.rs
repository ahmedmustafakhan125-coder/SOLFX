//! The introducing-broker programme (`ARCHITECTURE.md` § 8.5, § 5.1).
//!
//! Phase 6's exit criterion: *"Rebates accrue and claim correctly; every rebate emits a
//! verifiable event; tier boundaries tested."*
//!
//! These run **both programs together** in LiteSVM — `solfx-referral` reading core's real
//! `UserAccount` and claiming through the real CPI. A mock of either side would test the
//! mock.
//!
//! # What is actually being proven
//!
//! § 8.5 says the moat is not the code but *a network of IBs who trust your ledger because
//! they can audit it.* Trust is not a feature you can assert, so the tests below check the
//! three structural properties it is made of:
//!
//! 1. **Clients cannot be reassigned** — the binding is written once and nothing changes it.
//! 2. **Nobody can be short-changed or double-paid** — accrual derives from a monotonic
//!    public counter behind an idempotent watermark.
//! 3. **Nobody can be delayed** — claims are permissionless, with no approval step.

// Test code asserts against known values and unwraps expected-Ok results. The workspace
// denies `unwrap`, `expect` and `panic` because a panic in a *program* is a failed
// transaction with no named error; in a test a panic is the reporting mechanism.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

mod common;

use anchor_lang::prelude::Pubkey;
use common::*;
use solana_signer::Signer;
use solfx_core::state::Direction;
use solfx_math::referral::IbTier;

const MINI: u64 = ONE_LOT / 10;
const NO_BOUND_BUY: i64 = i64::MAX;
const NO_BOUND_SELL: i64 = 1;
const OVERRIDE_BPS: u16 = 2_000; // § 8.5: sub-IBs' parents earn 20% of the child

/// Protocol + market + pool + referral programme registered.
fn env_with_referral() -> Env {
    let mut env = Env::new();
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());
    env.seed_pool(1_000_000 * ONE_USDC);
    env.seed_insurance(50_000 * ONE_USDC);
    env.init_referral(OVERRIDE_BPS);
    env
}

/// A trader who names `referrer`, funded and ready.
fn funded_trader(env: &mut Env, referrer: Pubkey) -> User {
    let u = env.new_user(200_000 * ONE_USDC, referrer);
    env.deposit(&u, 100_000 * ONE_USDC).unwrap();
    u
}

/// Open and immediately close — generates fees in both directions.
fn round_trip(env: &mut Env, user: &User, nonce: u8, collateral: u64) {
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.open(
        user,
        0,
        nonce,
        Direction::Long,
        MINI,
        collateral,
        NO_BOUND_BUY,
        p,
    )
    .unwrap();
    let p = env.post_price_now(FEED_EUR_USD, PriceSpec::default());
    env.close(user, 0, nonce, NO_BOUND_SELL, p).unwrap();
}

// --- setup -------------------------------------------------------------------------------

/// Registration takes **two** steps by **two** authorities: the referral admin creates the
/// config, then core's admin separately decides to trust its PDA. Neither can do the other's
/// half, which is what keeps a deployed-but-unregistered referral program harmless.
#[test]
fn the_referral_programme_registers_with_core_in_two_steps() {
    let mut env = Env::new();
    env.init_protocol();
    assert_eq!(
        env.protocol_state().referral_authority,
        Pubkey::default(),
        "core launches with referral payouts disabled"
    );

    env.init_referral(OVERRIDE_BPS);

    assert_eq!(
        env.protocol_state().referral_authority,
        Env::referral_config_pda(),
        "core now honours the referral programme's PDA and nothing else"
    );
    let cfg = env.referral_config();
    assert_eq!(cfg.override_bps, OVERRIDE_BPS);
    assert_eq!(
        cfg.pool_split_bps, 1_000,
        "the config mirrors core's 10% referral split"
    );
}

#[test]
fn anyone_can_register_as_an_introducing_broker() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());

    let state = env.ib_state(&ib.pubkey());
    assert_eq!(state.authority, ib.pubkey());
    assert_eq!(state.parent, Pubkey::default());
    assert_eq!(state.unclaimed, 0);
    assert_eq!(state.tier(), IbTier::Bronze, "everyone starts at Bronze");
    assert_eq!(env.referral_config().ib_count, 1);
}

#[test]
fn an_ib_cannot_recruit_themselves() {
    let mut env = env_with_referral();
    let kp = solana_keypair::Keypair::new();
    env.svm.airdrop(&kp.pubkey(), 1_000_000_000).unwrap();

    let ix = Instruction {
        program_id: solfx_referral::ID,
        accounts: solfx_referral::accounts::RegisterIb {
            authority: kp.pubkey(),
            config: Env::referral_config_pda(),
            ib_account: Env::ib_pda(&kp.pubkey()),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: solfx_referral::instruction::RegisterIb {
            parent: kp.pubkey(),
        }
        .data(),
    };
    assert_err_contains(env.send(ix, &[&kp]), "cannot recruit themselves");
}

// --- accrual -----------------------------------------------------------------------------

/// The happy path: a referred trader trades, the IB syncs, the rebate is there.
#[test]
fn a_referred_trade_accrues_a_rebate() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let trader = funded_trader(&mut env, ib.pubkey());

    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.assert_invariants();

    // Core recorded the contribution on the trader's own account — a public, monotonic
    // counter anyone can read.
    let u = env.user_state(&trader);
    assert!(u.referral_fees_generated > 0, "core recorded no pool share");
    assert!(u.lifetime_volume > 0, "core recorded no volume");
    assert_eq!(
        env.protocol_state().total_referral_accrued,
        u.referral_fees_generated,
        "the protocol total must match what the only referred trader generated"
    );

    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();

    let state = env.ib_state(&ib.pubkey());
    assert!(state.unclaimed > 0, "the IB earned nothing");
    assert_eq!(state.lifetime_earned, state.unclaimed);
    assert_eq!(state.referred_volume, u.lifetime_volume);

    // Bronze is 8% of fees against a 10% pool, so the IB takes 80% of the pool share and
    // the remaining 20% sweeps to the treasury (§ 8.3).
    let expected =
        solfx_math::referral::entitlement(u.referral_fees_generated, 1_000, IbTier::Bronze)
            .unwrap();
    assert_eq!(state.unclaimed, expected);
}

/// **The property that makes the ledger trustworthy.** Syncing is permissionless and
/// idempotent — anyone may call it, any number of times, and it only ever credits the delta.
#[test]
fn syncing_is_permissionless_and_cannot_double_credit() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let trader = funded_trader(&mut env, ib.pubkey());
    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);

    // A complete stranger performs the sync. Nothing about the ledger depends on the IB, the
    // trader, or us being online.
    let stranger = solana_keypair::Keypair::new();
    env.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    let ix = env.sync_trader_ix(&ib.pubkey(), None, &trader, stranger.pubkey());
    env.send(ix, &[&stranger]).unwrap();

    let after_first = env.ib_state(&ib.pubkey()).unclaimed;
    assert!(after_first > 0);

    // A second sync with nothing new is refused outright.
    assert_err_contains(
        env.sync_trader(&ib.pubkey(), None, &trader),
        "Nothing new to sync",
    );
    assert_eq!(
        env.ib_state(&ib.pubkey()).unclaimed,
        after_first,
        "a repeated sync must credit nothing"
    );

    // More trading, then another sync: only the new delta lands.
    round_trip(&mut env, &trader, 1, 5_000 * ONE_USDC);
    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();
    let after_second = env.ib_state(&ib.pubkey()).unclaimed;
    assert!(
        after_second > after_first,
        "the second round trip earned nothing"
    );
}

/// **Clients cannot be reassigned.** An IB may only be credited for traders who named them,
/// and that name was written once at account creation.
#[test]
fn an_ib_cannot_claim_a_trader_who_named_someone_else() {
    let mut env = env_with_referral();
    let honest = env.register_ib(Pubkey::default());
    let thief = env.register_ib(Pubkey::default());

    let trader = funded_trader(&mut env, honest.pubkey());
    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);

    assert_err_contains(
        env.sync_trader(&thief.pubkey(), None, &trader),
        "was not referred by this introducing broker",
    );
    assert_eq!(env.ib_state(&thief.pubkey()).unclaimed, 0);

    // The rightful IB is unaffected.
    env.sync_trader(&honest.pubkey(), None, &trader).unwrap();
    assert!(env.ib_state(&honest.pubkey()).unclaimed > 0);
}

/// An unreferred trader generates nothing for anybody — the share sweeps to the treasury
/// (§ 8.3) rather than being earmarked for an IB who does not exist.
#[test]
fn an_unreferred_trader_accrues_nothing() {
    let mut env = env_with_referral();
    let trader = funded_trader(&mut env, Pubkey::default());
    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.assert_invariants();

    assert_eq!(env.user_state(&trader).referral_fees_generated, 0);
    assert_eq!(env.protocol_state().total_referral_accrued, 0);
    assert!(
        env.token_balance(&env.fee_vault) > 0,
        "the share should still be in the fee vault, as treasury money"
    );
}

/// Two traders under one IB both count, and their volumes aggregate for tiering.
#[test]
fn several_traders_aggregate_under_one_ib() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let a = funded_trader(&mut env, ib.pubkey());
    let b = funded_trader(&mut env, ib.pubkey());

    round_trip(&mut env, &a, 0, 5_000 * ONE_USDC);
    round_trip(&mut env, &b, 0, 5_000 * ONE_USDC);

    env.sync_trader(&ib.pubkey(), None, &a).unwrap();
    let after_a = env.ib_state(&ib.pubkey());
    env.sync_trader(&ib.pubkey(), None, &b).unwrap();
    let after_b = env.ib_state(&ib.pubkey());

    assert!(after_b.unclaimed > after_a.unclaimed);
    assert_eq!(
        after_b.referred_volume,
        env.user_state(&a).lifetime_volume + env.user_state(&b).lifetime_volume
    );
    env.assert_invariants();
}

// --- tiers (§ 8.5) -----------------------------------------------------------------------

/// The tier table, pinned exactly. A boundary that moves under an IB is the dispute this
/// whole design exists to prevent.
#[test]
fn the_tier_boundaries_match_the_published_table() {
    const M: u64 = 1_000_000_000_000; // $1M
    assert_eq!(IbTier::for_volume(0), IbTier::Bronze);
    assert_eq!(IbTier::for_volume(5 * M - 1), IbTier::Bronze);
    assert_eq!(IbTier::for_volume(5 * M), IbTier::Silver);
    assert_eq!(IbTier::for_volume(25 * M - 1), IbTier::Silver);
    assert_eq!(IbTier::for_volume(25 * M), IbTier::Gold);
    assert_eq!(IbTier::for_volume(100 * M - 1), IbTier::Gold);
    assert_eq!(IbTier::for_volume(100 * M), IbTier::Diamond);

    assert_eq!(IbTier::Bronze.share_bps(), 800);
    assert_eq!(IbTier::Silver.share_bps(), 1_000);
    assert_eq!(IbTier::Gold.share_bps(), 1_300);
    assert_eq!(IbTier::Diamond.share_bps(), 1_600);
}

/// An IB whose referred volume crosses $5M is paid at Silver on the sync that crosses it —
/// 10% of fees rather than 8%, which at the launch split is the whole pool.
#[test]
fn crossing_a_volume_boundary_promotes_the_ib() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let trader = funded_trader(&mut env, ib.pubkey());

    // A single round trip on a mini lot is far short of $5M, so this IB stays Bronze.
    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();
    assert_eq!(env.ib_state(&ib.pubkey()).tier(), IbTier::Bronze);

    // Trade enough notional to cross into Silver. Each round trip books ~$21.7k of volume
    // (open + close on ~$10.8k of notional), so ~230 are needed — instead the volume is
    // credited directly, which is what a longer session would produce.
    env.patch_market(0, |_m| {});
    let mut u = env.user_state(&trader);
    u.lifetime_volume += 6_000_000_000_000; // +$6M
    env.patch_user(&trader, |acct| {
        acct.lifetime_volume = u.lifetime_volume;
    });

    round_trip(&mut env, &trader, 1, 5_000 * ONE_USDC);
    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();

    let state = env.ib_state(&ib.pubkey());
    assert_eq!(
        state.tier(),
        IbTier::Silver,
        "referred volume {} should be Silver",
        state.referred_volume
    );
}

/// Promotion applies to what is synced afterwards, not retroactively — stated in the code
/// and pinned here, because a silent retroactive recalculation is exactly the kind of
/// surprise that starts disputes.
#[test]
fn a_promotion_is_not_applied_retroactively() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let trader = funded_trader(&mut env, ib.pubkey());

    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();
    let bronze_earned = env.ib_state(&ib.pubkey()).lifetime_earned;

    // Promote, then sync again with no new trading: nothing is re-credited.
    env.patch_user(&trader, |acct| {
        acct.lifetime_volume += 6_000_000_000_000;
    });
    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();

    let state = env.ib_state(&ib.pubkey());
    assert_eq!(
        state.lifetime_earned, bronze_earned,
        "a promotion must not repay history"
    );
    assert_eq!(state.tier(), IbTier::Silver);
}

// --- the two-level network (§ 8.5) -------------------------------------------------------

/// A sub-broker's parent earns 20% of what the child earned — an override on the child's
/// rebate, **not** a second bite of the trader's fees.
#[test]
fn a_parent_ib_earns_an_override_on_its_sub_brokers() {
    let mut env = env_with_referral();
    let parent = env.register_ib(Pubkey::default());
    let child = env.register_ib(parent.pubkey());
    let trader = funded_trader(&mut env, child.pubkey());

    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&child.pubkey(), Some(&parent.pubkey()), &trader)
        .unwrap();

    let child_state = env.ib_state(&child.pubkey());
    let parent_state = env.ib_state(&parent.pubkey());

    assert!(child_state.unclaimed > 0, "the child earned nothing");
    assert!(parent_state.unclaimed > 0, "the parent earned no override");

    let expected =
        solfx_math::referral::parent_override(child_state.unclaimed, OVERRIDE_BPS).unwrap();
    assert_eq!(
        parent_state.unclaimed, expected,
        "20% of the child's rebate"
    );
    assert!(
        parent_state.unclaimed < child_state.unclaimed,
        "an override must never exceed what the sub-broker earned"
    );
}

/// The chain never costs more than the pool set aside: child + parent together stay inside
/// what the trader's fees contributed.
#[test]
fn a_two_level_chain_stays_inside_the_referral_pool() {
    let mut env = env_with_referral();
    let parent = env.register_ib(Pubkey::default());
    let child = env.register_ib(parent.pubkey());
    let trader = funded_trader(&mut env, child.pubkey());

    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&child.pubkey(), Some(&parent.pubkey()), &trader)
        .unwrap();

    let pool_share = env.user_state(&trader).referral_fees_generated;
    let total = env.ib_state(&child.pubkey()).unclaimed + env.ib_state(&parent.pubkey()).unclaimed;
    assert!(
        total <= pool_share,
        "the chain paid {total} out of a pool share of {pool_share}"
    );
}

/// A missing parent account must not block the child's own rebate — the override is simply
/// not paid and stays in the pool.
#[test]
fn a_missing_parent_account_does_not_block_the_childs_rebate() {
    let mut env = env_with_referral();
    let parent = env.register_ib(Pubkey::default());
    let child = env.register_ib(parent.pubkey());
    let trader = funded_trader(&mut env, child.pubkey());

    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&child.pubkey(), None, &trader).unwrap();

    assert!(env.ib_state(&child.pubkey()).unclaimed > 0);
    assert_eq!(env.ib_state(&parent.pubkey()).unclaimed, 0);
}

// --- claiming ----------------------------------------------------------------------------

/// **No approval, no schedule, no counterparty.** The IB calls `claim` and is paid.
#[test]
fn an_ib_claims_permissionlessly_through_the_cpi() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let trader = funded_trader(&mut env, ib.pubkey());
    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();

    let owed = env.ib_state(&ib.pubkey()).unclaimed;
    let dest = env.new_token_account_for(ib.pubkey());

    env.claim_rebate(&ib, dest).unwrap();
    env.assert_invariants();

    assert_eq!(env.token_balance(&dest), owed, "the IB was paid in full");
    let state = env.ib_state(&ib.pubkey());
    assert_eq!(state.unclaimed, 0);
    assert_eq!(state.lifetime_claimed, owed);
    assert_eq!(env.protocol_state().total_referral_claimed, owed);
}

#[test]
fn claiming_with_nothing_owed_is_refused() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let dest = env.new_token_account_for(ib.pubkey());

    assert_err_contains(env.claim_rebate(&ib, dest), "Nothing to claim");
}

#[test]
fn one_ib_cannot_claim_anothers_rebate() {
    let mut env = env_with_referral();
    let earner = env.register_ib(Pubkey::default());
    let thief = env.register_ib(Pubkey::default());
    let trader = funded_trader(&mut env, earner.pubkey());
    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&earner.pubkey(), None, &trader).unwrap();

    // The thief signs, but the IB account PDA is derived from the *signer's* key, so they
    // can only ever reach their own — which is empty.
    let dest = env.new_token_account_for(thief.pubkey());
    assert_err_contains(env.claim_rebate(&thief, dest), "Nothing to claim");
    assert!(env.ib_state(&earner.pubkey()).unclaimed > 0);
}

/// **The constraint that protects the treasury.** The fee vault holds treasury money as well
/// as referral money; a claim may never reach past what referred traders actually generated.
#[test]
fn a_claim_can_never_exceed_the_referral_pool() {
    let mut env = env_with_referral();
    let ib = env.register_ib(Pubkey::default());
    let trader = funded_trader(&mut env, ib.pubkey());
    round_trip(&mut env, &trader, 0, 5_000 * ONE_USDC);
    env.sync_trader(&ib.pubkey(), None, &trader).unwrap();

    let accrued = env.protocol_state().total_referral_accrued;
    let fee_vault = env.token_balance(&env.fee_vault);
    assert!(
        fee_vault > accrued,
        "the fee vault should hold treasury money beyond the referral pool: \
         vault {fee_vault} vs accrued {accrued}"
    );

    // Inflate the IB's balance far past what the pool holds and try to draw it.
    env.patch_ib(&ib.pubkey(), |acct| acct.unclaimed = fee_vault);
    let dest = env.new_token_account_for(ib.pubkey());
    assert_err_contains(
        env.claim_rebate(&ib, dest),
        "would exceed the referral pool",
    );

    assert_eq!(env.token_balance(&dest), 0);
    env.assert_invariants();
}

/// Core refuses a payout signed by anyone other than the authority its admin registered —
/// the one thing core knows about the referral programme.
#[test]
fn core_refuses_a_payout_from_an_unregistered_authority() {
    let mut env = env_with_referral();
    let attacker = solana_keypair::Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let dest = env.new_token_account_for(attacker.pubkey());

    let ix = Instruction {
        program_id: solfx_core::ID,
        accounts: solfx_core::accounts::PayReferral {
            referral_authority: attacker.pubkey(),
            protocol: env.protocol,
            fee_vault: env.fee_vault,
            destination: dest,
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
        data: solfx_core::instruction::PayReferral { amount: 1 }.data(),
    };
    assert_err_contains(
        env.send(ix, &[&attacker]),
        "Signer is not the registered referral authority",
    );
}

/// Full lifecycle end to end, with the protocol's invariants holding throughout.
#[test]
fn the_whole_ib_lifecycle_preserves_every_invariant() {
    let mut env = env_with_referral();
    let parent = env.register_ib(Pubkey::default());
    let child = env.register_ib(parent.pubkey());

    let t1 = funded_trader(&mut env, child.pubkey());
    let t2 = funded_trader(&mut env, parent.pubkey());
    let t3 = funded_trader(&mut env, Pubkey::default()); // unreferred

    for (i, t) in [&t1, &t2, &t3].into_iter().enumerate() {
        round_trip(&mut env, t, u8::try_from(i).unwrap(), 5_000 * ONE_USDC);
        env.assert_invariants();
    }

    env.sync_trader(&child.pubkey(), Some(&parent.pubkey()), &t1)
        .unwrap();
    env.sync_trader(&parent.pubkey(), None, &t2).unwrap();
    env.assert_invariants();

    let parent_dest = env.new_token_account_for(parent.pubkey());
    let child_dest = env.new_token_account_for(child.pubkey());
    env.claim_rebate(&parent, parent_dest).unwrap();
    env.claim_rebate(&child, child_dest).unwrap();
    env.assert_invariants();

    assert!(env.token_balance(&parent_dest) > 0);
    assert!(env.token_balance(&child_dest) > 0);

    // The books reconcile three ways.
    let p = env.protocol_state();
    let cfg = env.referral_config();
    let still_owed =
        env.ib_state(&parent.pubkey()).unclaimed + env.ib_state(&child.pubkey()).unclaimed;

    assert_eq!(
        cfg.total_claimed + still_owed,
        cfg.total_accrued,
        "every rebate the programme credited is either paid or still owed"
    );
    assert_eq!(
        p.total_referral_claimed, cfg.total_claimed,
        "core and the referral programme agree on what was paid"
    );

    // The unreferred trader contributed nothing, and the tiers did not claim the whole
    // pool — the difference stayed with the treasury (§ 8.3).
    assert_eq!(env.user_state(&t3).referral_fees_generated, 0);
    assert!(
        cfg.total_accrued < p.total_referral_accrued,
        "Bronze takes 80% of the pool; the rest must sweep to treasury: \
         credited {} of {} accrued",
        cfg.total_accrued,
        p.total_referral_accrued
    );
}

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{InstructionData, ToAccountMetas};
