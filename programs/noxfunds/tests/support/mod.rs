//! NOXFUNDS test setup shared across the stage files.
//!
//! Not every test binary uses every helper, which is why each `mod support;` is declared
//! `#[allow(dead_code)]` — the same idiom the operator binaries use for `#[path]` modules.

use anchor_lang::prelude::{AccountDeserialize as _, ProgramData, Pubkey};
use anchor_lang::solana_program::bpf_loader_upgradeable;

use crate::common::Env;

/// Bytes before the ELF in a `ProgramData` account: a `u32` variant tag, a `u64` slot, and an
/// `Option<Pubkey>` that the loader always serialises at its full 33 bytes. 4 + 8 + 1 + 32.
const PROGRAMDATA_METADATA_LEN: usize = 45;
/// Offset of the `Option` tag for the upgrade authority inside that metadata.
const AUTHORITY_TAG_OFFSET: usize = 12;

/// The loader's `ProgramData` address for NOXFUNDS. Derived, never hardcoded.
pub fn program_data_address() -> Pubkey {
    program_data_address_of(&noxfunds::ID)
}

pub fn program_data_address_of(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[program.as_ref()], &bpf_loader_upgradeable::ID).0
}

/// Set who may upgrade a program that LiteSVM has already loaded.
///
/// `LiteSVM::add_program` creates a genuine upgradeable program — a `Program` account pointing
/// at a `ProgramData` account — but always with **no** upgrade authority, which would make
/// `initialize_config` unreachable. This rewrites that one field in place.
///
/// The layout is written by hand rather than through a serialiser, so the result is read back
/// through Anchor's own `ProgramData` — the exact deserialiser `initialize_config`'s constraint
/// uses. If this layout were ever wrong, every fixture would fail here, loudly, instead of the
/// guard silently passing or failing for a reason unrelated to the test.
pub fn set_upgrade_authority_of(env: &mut Env, program: &Pubkey, authority: Option<Pubkey>) {
    let address = program_data_address_of(program);
    let mut account = env
        .svm
        .get_account(&address)
        .expect("the program must be loaded with add_program first");
    assert!(
        account.data.len() >= PROGRAMDATA_METADATA_LEN,
        "not a ProgramData account"
    );

    let (tag, key) = match authority {
        Some(k) => (1u8, k.to_bytes()),
        None => (0u8, [0u8; 32]),
    };
    account.data[AUTHORITY_TAG_OFFSET] = tag;
    account.data[AUTHORITY_TAG_OFFSET + 1..PROGRAMDATA_METADATA_LEN].copy_from_slice(&key);
    env.svm.set_account(address, account).unwrap();

    let written = env.svm.get_account(&address).unwrap();
    let decoded = ProgramData::try_deserialize(&mut written.data.as_slice())
        .expect("ProgramData must still decode after the rewrite");
    assert_eq!(
        decoded.upgrade_authority_address, authority,
        "the upgrade authority did not round-trip through Anchor's ProgramData"
    );
}

pub fn set_upgrade_authority(env: &mut Env, authority: Option<Pubkey>) {
    set_upgrade_authority_of(env, &noxfunds::ID, authority);
}
