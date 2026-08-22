use anchor_lang::prelude::*;

use crate::state::Direction;

/// Take-profit or stop-loss.
///
/// The two differ only in which side of the trigger price fires them, but they are named
/// separately because the *risk* they carry is not the same: a stop-loss that fails to fire
/// is a position that keeps losing, while a take-profit that fails to fire is a trader who is
/// merely annoyed. Naming them apart means monitoring can alert on one and not the other.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum TriggerKind {
    /// Fires when the price moves **in the trader's favour** past the trigger.
    TakeProfit,
    /// Fires when the price moves **against** the trader past the trigger.
    StopLoss,
}

impl TriggerKind {
    /// Has this trigger's condition been met at `price`?
    ///
    /// # The four cases, and why they are not two
    ///
    /// A take-profit on a long fires when the price *rises*; on a short, when it *falls*.
    /// The direction flips the comparison, so there are four combinations and each one is
    /// worth reading explicitly rather than being folded into a sign trick — a stop-loss
    /// wired backwards would close winning positions and hold losing ones, and would look
    /// like a market-conditions complaint rather than a bug.
    ///
    /// Comparisons are **inclusive**: a price exactly at the trigger fires it. A trader who
    /// sets a stop at 1.0800 expects it to fire at 1.0800.
    #[must_use]
    pub fn is_met(self, direction: Direction, trigger_price: i64, price: i64) -> bool {
        match (self, direction) {
            (Self::TakeProfit, Direction::Long) | (Self::StopLoss, Direction::Short) => {
                price >= trigger_price
            }
            (Self::TakeProfit, Direction::Short) | (Self::StopLoss, Direction::Long) => {
                price <= trigger_price
            }
        }
    }

    /// Is this trigger price on the correct side of the current price to be placeable?
    ///
    /// A take-profit below the market on a long is already met, and a stop-loss above it is
    /// too — either would fire on the next keeper pass, which is not an order, it is a
    /// delayed market close with extra steps. Rejecting it at placement tells the trader
    /// immediately rather than surprising them a second later.
    #[must_use]
    pub fn is_placeable(self, direction: Direction, trigger_price: i64, price: i64) -> bool {
        !self.is_met(direction, trigger_price, price)
    }
}

/// A resting take-profit or stop-loss. PDA at `["order", position, order_id]`.
///
/// # Why this is on chain rather than in the keeper
///
/// A stop-loss held off-chain is a promise from whoever runs the bot. On chain it is an
/// account with rules: **anyone** can execute it, the conditions are public, and the trader
/// can verify the order exists without trusting us. That is the same argument as the IB
/// ledger (§ 8.5) applied to risk management — and it is why this arrived with its executor
/// in Phase 7 rather than alone in Phase 4, since an order nothing fires is worse than no
/// order at all.
///
/// # What it does not promise
///
/// **A trigger is not a guaranteed fill price.** It fires when the *oracle* crosses the
/// trigger, and then fills at the execution price — which includes the adverse spread, and in
/// a gap may be far past the trigger. This is exactly how a stop works at any broker, and the
/// UI must say so, because the alternative is a trader who believes they were promised
/// something and discovers otherwise during the one event that matters.
#[account]
#[derive(InitSpace)]
pub struct TriggerOrder {
    pub position: Pubkey,
    pub user_account: Pubkey,
    /// The wallet that placed it — also the only one that can cancel it.
    pub authority: Pubkey,
    pub market_index: u16,
    /// Distinguishes several orders on one position: a TP and an SL are the usual pair.
    pub order_id: u8,
    pub kind: TriggerKind,
    /// At `PRICE_PRECISION`. Compared against the oracle price, not the fill price.
    pub trigger_price: i64,
    /// Base units to close. May be less than the position, for scaling out.
    pub size_base: u64,
    pub created_at: i64,
    pub bump: u8,
    pub _reserved: [u8; 32],
}
