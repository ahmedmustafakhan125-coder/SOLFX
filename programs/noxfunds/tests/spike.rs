//! Stage 0 — the two ceilings, measured.
//!
//! `docs/NOXFUNDS-PLAN.md` Part 10 makes Stage 0's exit criterion "**fits the stack frame and
//! the packet.** Measured, not argued." This file does the packet half and the account count;
//! the stack half needs a running SVM and lives in `spike_svm.rs`.
//!
//! The derivation in `docs/program-upgrade-plan.md` predicted 22 accounts and 959 bytes at the
//! widest market. A prediction nobody checks is a guess, so both are asserted here.

// Test code asserts against known values and unwraps expected-Ok results. The workspace denies
// `unwrap`, `expect` and `panic` because a panic in a *program* is a failed transaction with
// no named error; in a test a panic is the reporting mechanism.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use anchor_lang::{InstructionData, ToAccountMetas};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_message::Message;

use anchor_lang::solana_program::{instruction::Instruction, pubkey::Pubkey};

/// Build `spike_open_with_stop` exactly as a client would.
///
/// `legs` is how many oracle accounts the market needs: 1 for a USD-quoted direct market,
/// 3 for the widest shape there is — synthetic *and* non-USD-quoted.
fn spike_ix(legs: usize) -> (Instruction, Pubkey) {
    let trader = Pubkey::new_unique();
    let mandate = Pubkey::new_unique();
    let (mandate_signer, _) = Pubkey::find_program_address(
        &[noxfunds::MANDATE_SIGNER_SEED, mandate.as_ref()],
        &noxfunds::ID,
    );

    let accounts = noxfunds::accounts::SpikeOpenWithStop {
        trader,
        nox_config: Pubkey::new_unique(),
        mandate,
        trader_profile: Pubkey::new_unique(),
        mandate_signer,
        protocol: Pubkey::new_unique(),
        user_account: Pubkey::new_unique(),
        market: Pubkey::new_unique(),
        position: Pubkey::new_unique(),
        trigger_order: Pubkey::new_unique(),
        collateral_vault: Pubkey::new_unique(),
        lp_pool: Pubkey::new_unique(),
        lp_vault: Pubkey::new_unique(),
        insurance_fund: Pubkey::new_unique(),
        insurance_vault: Pubkey::new_unique(),
        fee_vault: Pubkey::new_unique(),
        price_update: Pubkey::new_unique(),
        secondary_price_update: (legs > 1).then(Pubkey::new_unique),
        quote_conversion_price_update: (legs > 2).then(Pubkey::new_unique),
        token_program: anchor_spl::token::ID,
        system_program: anchor_lang::system_program::ID,
        solfx_core_program: solfx_core::ID,
    };

    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: accounts.to_account_metas(None),
        data: noxfunds::instruction::SpikeOpenWithStop {
            market_index: 0,
            nonce: 0,
            direction: solfx_core::state::Direction::Long,
            size_base: 1,
            collateral: 1,
            price_limit: i64::MAX,
            order_id: 0,
            stop_loss_price: 1,
        }
        .data(),
    };
    (ix, trader)
}

/// Measured the way a client actually sends it: one signature, and the two compute-budget
/// instructions in front. `solfx-core`'s own packet bound does the same — see
/// `programs/solfx-core/tests/common/mod.rs`, `measure_keeper_tx`.
fn wire_len((ix, trader): (Instruction, Pubkey)) -> usize {
    // **The trader is the fee payer.** An earlier version of this test used a fresh keypair,
    // which put a 25th key and a second required signature into the message and measured 1056
    // bytes against a derivation of 959. The derivation was right; the test was modelling a
    // client nobody would write. A funded trade has exactly one human signer.
    let ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(200_000),
        ComputeBudgetInstruction::set_compute_unit_price(1),
        ix,
    ];
    let message = Message::new(&ixs, Some(&trader));
    let signatures = message.header.num_required_signatures as usize;
    // Legacy wire format: a short-vec length byte, then 64 bytes per signature, then the
    // message. Identical arithmetic to `measure_keeper_tx` in solfx-core's harness, so the
    // two suites' packet figures are directly comparable rather than merely similar.
    1 + signatures * 64 + message.serialize().len()
}

/// The number Stage 0 exists to pin.
///
/// The plan said "~21 accounts" and never enumerated them. Anchor encodes an absent optional
/// account as the callee program's id rather than omitting it, so the slot count does not
/// change with the market shape — only the number of *distinct* keys does.
#[test]
fn the_account_count_is_twenty_two() {
    for legs in [1, 2, 3] {
        assert_eq!(
            spike_ix(legs).0.accounts.len(),
            22,
            "account slots must not vary with the oracle leg count ({legs} legs)"
        );
    }
}

/// The packet ceiling, at the widest market there is.
///
/// 1232 is the legacy/v0 limit and the one that binds today. v1 raises it to 4096 and is live
/// on devnet, but mainnet has not activated it — so the bound worth holding is the smaller.
#[test]
fn the_widest_market_fits_the_legacy_packet() {
    let bytes = wire_len(spike_ix(3));
    assert!(
        bytes <= 1232,
        "funded open + stop must fit one packet: {bytes} > 1232"
    );
    // Derived at 959 in docs/program-upgrade-plan.md. A drift either way means the account
    // list or the instruction data changed and the derivation needs revisiting.
    assert!(
        (940..=980).contains(&bytes),
        "expected ~959 bytes from the derivation, measured {bytes}"
    );
}

/// A USD-quoted direct market is the common case and must be comfortably smaller.
#[test]
fn a_simple_market_leaves_more_room() {
    let widest = wire_len(spike_ix(3));
    let simple = wire_len(spike_ix(1));
    assert!(simple < widest, "fewer distinct keys must mean fewer bytes");
    assert!(simple <= 1232);
}

/// Print the measured figures, so the report quotes hardware rather than arithmetic.
#[test]
fn report_the_measurements() {
    for (label, legs) in [
        ("USD-quoted direct", 1),
        ("one extra leg", 2),
        ("widest", 3),
    ] {
        let (ix, _) = spike_ix(legs);
        let bytes = wire_len(spike_ix(legs));
        println!(
            "  {label:<18} slots {}  bytes {bytes:>5}  /1232  spare {:>4}",
            ix.accounts.len(),
            1232usize.saturating_sub(bytes),
        );
    }
}
