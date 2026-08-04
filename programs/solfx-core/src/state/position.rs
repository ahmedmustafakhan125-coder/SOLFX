use anchor_lang::prelude::*;
use solfx_math::Direction as MathDirection;

/// Which way the trader is betting.
///
/// The on-chain mirror of `solfx_math::Direction`. The maths crate deliberately carries no
/// Anchor derives — that is what keeps it testable without the BPF toolchain — so the two
/// types meet here and nowhere else.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Long,
    Short,
}

impl From<Direction> for MathDirection {
    fn from(d: Direction) -> Self {
        match d {
            Direction::Long => Self::Long,
            Direction::Short => Self::Short,
        }
    }
}

impl Direction {
    /// The opposite side. Used to work out which OI bucket a position leaves.
    #[must_use]
    pub fn flip(self) -> Self {
        match self {
            Self::Long => Self::Short,
            Self::Short => Self::Long,
        }
    }
}

/// One isolated position. PDA at `["position", user_account, market_index, nonce]`.
///
/// # Isolated margin (ADR-004)
///
/// `collateral` is moved *out* of `UserAccount.free_collateral` when the position opens and
/// lives here until it closes. Nothing else can draw on it, and liquidating this position
/// touches only this account — which is what keeps liquidation a single-account operation
/// cheap enough to stay profitable during the congestion that causes it.
///
/// It is also why `withdraw_collateral` needs no margin check: free collateral is genuinely
/// free, because a position's margin is not in it.
///
/// **Append-only**, like every other account struct (§ 5.6).
#[account]
#[derive(InitSpace)]
pub struct Position {
    pub user_account: Pubkey,
    pub market_index: u16,
    /// Lets one trader hold several positions in the same market.
    pub nonce: u8,
    pub direction: Direction,

    /// Size in base-currency units at `BASE_PRECISION`, never in lots. Lots are a
    /// presentation concept; storing base units lets the engine handle micro-lots and
    /// arbitrary sizes through one code path (§ 6.1).
    pub size_base: u64,
    /// Volume-weighted average entry, at `PRICE_PRECISION`, in the market's quote currency.
    pub entry_price: i64,
    /// Isolated margin, in USDC at `QUOTE_PRECISION`.
    pub collateral: u64,
    /// The USDC notional this position contributed to `Market.oi_*` when it opened.
    ///
    /// Stored rather than recomputed because open interest must be *removed* at exactly the
    /// figure it was added at. Recomputing it at close would use the current price and, on a
    /// non-USD-quoted market, the current conversion rate — so the counter would drift with
    /// every price move and invariant I4 would fail on a market that had merely traded.
    pub entry_notional: u64,

    // --- funding and carry snapshots (Phase 4) ---
    // The cumulative-index model: the market accumulates, each position stores its snapshot
    // at open, and the difference is what it owes. O(1) per position, because iterating every
    // open position on chain to charge funding is not possible (§ 6.5).
    pub cum_funding_entry: i128,
    pub cum_borrow_entry: u128,
    /// Funding already settled into `collateral` by a modify. Signed: the light side of the
    /// book receives.
    pub realized_funding: i64,

    pub opened_at: i64,
    /// Slot at open, for the minimum-hold check (§ 6.6).
    ///
    /// A trader who opens and closes inside one slot pays two fees but takes zero risk. If
    /// the protocol's spread is ever narrower than the true market spread, that is a riskless
    /// profit repeated by bots until the vault is empty. Requiring a few slots of exposure
    /// closes it, and costs an honest trader nothing.
    pub opened_at_slot: u64,
    pub last_updated_at: i64,

    /// Cumulative open fees paid, for the portfolio view.
    pub open_fee_paid: u64,
    /// Copied from `UserAccount` at open so a rebate can be attributed without loading the
    /// user account (§ 8.5). Immutable, like the binding it copies.
    pub referrer: Pubkey,

    pub bump: u8,
    pub _reserved: [u8; 64],
}

impl Position {
    /// Signed size for skew arithmetic: `+size` for a long, `−size` for a short.
    pub fn signed_size(&self) -> i128 {
        match self.direction {
            Direction::Long => i128::from(self.size_base),
            Direction::Short => -i128::from(self.size_base),
        }
    }
}
