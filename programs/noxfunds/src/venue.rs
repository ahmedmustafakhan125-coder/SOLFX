//! The two facts about a SolFX `UserAccount` that NOXFUNDS' accounting depends on.
//!
//! Read at fixed offsets rather than by deserializing the account, for two reasons. The open path
//! already carries twenty-one accounts and sits close to the 4 KB BPF frame, so it cannot afford
//! a second copy of a foreign struct. And the settlement and equity paths must work when the
//! account does not exist yet, which a typed `Account<UserAccount>` refuses outright. The offsets
//! are pinned by a test that serializes a real `UserAccount` and reads it back through these.

use anchor_lang::prelude::*;
use anchor_lang::Discriminator;
use solfx_core::state::UserAccount;

use crate::errors::NoxError;

/// 8-byte discriminator, then `authority: Pubkey`, then these two — see
/// `solfx_core::state::UserAccount`.
const FREE_COLLATERAL_AT: usize = 8 + 32;
const OPEN_POSITIONS_AT: usize = FREE_COLLATERAL_AT + 8;

/// What SolFX says about a user account, if it exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Venue {
    pub free_collateral: u64,
    pub open_positions: u16,
}

/// Read a SolFX user account, or `None` if nothing has been created at the address.
///
/// "Nothing created" means no data. Lamports alone do not count — anyone may send SOL to any
/// address, and that leaves it owned by the System Program with no data, which is still an
/// account that holds no collateral. Anything with data must be a real SolFX `UserAccount`.
pub(crate) fn read_user_account(info: &AccountInfo) -> Result<Option<Venue>> {
    if info.data_is_empty() {
        return Ok(None);
    }
    require_keys_eq!(*info.owner, solfx_core::ID, NoxError::NotTheTrader);
    let data = info.try_borrow_data()?;
    require!(
        data.get(..UserAccount::DISCRIMINATOR.len()) == Some(UserAccount::DISCRIMINATOR),
        NoxError::NotTheTrader
    );
    let free = data
        .get(FREE_COLLATERAL_AT..OPEN_POSITIONS_AT)
        .and_then(|b| <[u8; 8]>::try_from(b).ok())
        .ok_or(NoxError::MathOverflow)?;
    let open = data
        .get(OPEN_POSITIONS_AT..OPEN_POSITIONS_AT.saturating_add(2))
        .and_then(|b| <[u8; 2]>::try_from(b).ok())
        .ok_or(NoxError::MathOverflow)?;
    Ok(Some(Venue {
        free_collateral: u64::from_le_bytes(free),
        open_positions: u16::from_le_bytes(open),
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use anchor_lang::AccountSerialize;

    /// The offsets above are only as good as the layout they assume. Serialize a real
    /// `UserAccount` with distinctive values and read it back through `read_user_account`.
    #[test]
    fn the_offsets_match_solfx_cores_own_layout() {
        let ua = UserAccount {
            authority: Pubkey::new_unique(),
            free_collateral: 0x0102_0304_0506_0708,
            open_positions: 0x0A0B,
            referrer: Pubkey::new_unique(),
            thirty_day_volume: 11,
            volume_window_start_ts: 12,
            total_deposits: 13,
            total_withdrawals: 14,
            created_at: 15,
            referral_fees_generated: 16,
            lifetime_volume: 17,
            bump: 254,
            _reserved: [0; 48],
        };
        let mut data = Vec::new();
        ua.try_serialize(&mut data).unwrap();

        let key = Pubkey::new_unique();
        let owner = solfx_core::ID;
        let mut lamports = 1_000_000u64;
        let info = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);
        let v = read_user_account(&info).unwrap().unwrap();
        assert_eq!(v.free_collateral, 0x0102_0304_0506_0708);
        assert_eq!(v.open_positions, 0x0A0B);
    }

    #[test]
    fn no_data_reads_as_no_account() {
        let key = Pubkey::new_unique();
        let owner = anchor_lang::system_program::ID;
        let mut lamports = 5_000u64; // someone sent SOL to the address
        let mut data: Vec<u8> = Vec::new();
        let info = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);
        assert_eq!(read_user_account(&info).unwrap(), None);
    }
}
