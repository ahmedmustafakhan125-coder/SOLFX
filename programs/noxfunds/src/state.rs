//! On-chain state.
//!
//! `Mandate` is the centre of the design: it is the rule set an investor fixed at funding, and
//! it is the thing a funded trade is checked against before the fill.

use anchor_lang::prelude::*;

/// Protocol configuration. One per deployment, at `["config"]`.
#[account]
#[derive(InitSpace)]
pub struct NoxConfig {
    pub admin: Pubkey,
    /// May pause, may not move funds. Same split as SolFX's guardian.
    pub guardian: Pubkey,
    /// Where the 5% performance fee and forfeited stakes accumulate.
    pub treasury: Pubkey,
    /// The venue this program trades on. Checked on every CPI so a mandate cannot be pointed
    /// at an impostor program that happens to share an instruction layout.
    pub solfx_program: Pubkey,
    pub usdc_mint: Pubkey,
    /// Protocol share of **gross** profit. 500 = 5%.
    pub protocol_fee_bps: u16,
    pub paused: bool,
    pub bump: u8,
    pub _reserved: [u8; 64],
}

/// How many positions one mandate can track at once. `MandateRules::validate` caps
/// `max_concurrent_positions` at this, so the rule and the storage cannot disagree.
pub const MAX_SLOTS: usize = 8;

/// One open position, as NOXFUNDS booked it.
///
/// # Why the mandate tracks positions individually
///
/// A counter and a running notional total were not enough, and three bugs proved it:
///
/// - **A stop-out never came back.** A stop fires through `solfx-core`'s own
///   `execute_trigger_order`, which never enters NOXFUNDS, so the counter stayed high forever.
///   Settlement needs it at zero and the equity crank needs a triple per counted position that
///   no longer exists — a stopped-out mandate could never settle or be observed again.
/// - **The book drifted.** Opens booked notional at the oracle price; closes unbooked core's
///   `entry_notional`, which is at the fill price after spread. A short left a residue on every
///   round trip until the book refused trades it had room for.
/// - **The crank could be fooled.** It checked only the *count* of triples, so the same
///   profitable position supplied twice inflated equity and hid a breach.
///
/// A slot records exactly what was booked and where, so each close releases the exact figure it
/// added, a stop-out can be reconciled against the chain, and the crank can demand each open
/// position once.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Default, PartialEq, Eq, Debug,
)]
pub struct PositionSlot {
    pub open: bool,
    pub market_index: u16,
    pub nonce: u8,
    /// Notional as NOXFUNDS booked it at open, in USDC. Released at this exact figure.
    pub notional: u64,
}

/// Where a mandate is in its life. `Breached` is deliberately distinct from `WindingDown`: it
/// records *why* the mandate ended, permanently, and that is exactly what an investor choosing
/// a trader is reading. Collapsing the two would make a rule violation indistinguishable from
/// a voluntary exit.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MandateState {
    Active,
    Breached,
    WindingDown,
    Settled,
}

/// An investor's capital, a trader's permission to trade it, and the rules that bound them.
///
/// # Why the rules are immutable
///
/// Written once at funding and never mutable, mirroring the referral programme's immutable
/// `referrer` binding. Nobody — including the operator — can retighten terms after the fact.
/// A trader who accepted a 3% drawdown limit cannot be moved to 1% mid-mandate, and an
/// investor cannot be talked into a wider one.
///
/// # Why this is not the SolFX authority
///
/// It cannot be. `open_position` and `place_trigger_order` are both `init, payer = authority`,
/// which Anchor services through the System Program, which refuses a `from` that carries data.
/// This account is an `#[account]` struct and therefore data-bearing. The authority is
/// [`MANDATE_SIGNER_SEED`](crate::constants::MANDATE_SIGNER_SEED) — a dataless, system-owned
/// PDA whose bump is stored here.
#[account]
#[derive(InitSpace)]
pub struct Mandate {
    pub investor: Pubkey,
    pub trader: Pubkey,
    /// Lets one investor fund the same trader more than once.
    pub seq: u8,

    /// The SolFX `UserAccount` this mandate's signer authorises.
    pub solfx_user_account: Pubkey,
    /// Bump of the dataless signer PDA. Stored so no instruction re-derives it — that costs
    /// ~1,500 CU for nothing, and this program signs two CPIs per trade.
    pub signer_bump: u8,

    /// What the investor put in, in USDC at `QUOTE_PRECISION`.
    pub principal: u64,
    /// High-water mark of equity. Only ever rises, so a trader cannot reset their drawdown by
    /// closing out or withdrawing.
    pub peak_equity: u64,

    // --- the rules, fixed at funding and never mutable ------------------------------------
    /// Ceiling on a single trade's notional, in USDC.
    pub max_trade_notional: u64,
    /// Ceiling on the sum of all open notional, in USDC.
    pub max_total_notional: u64,
    /// Trailing from `peak_equity`. 300 = 3%.
    pub max_drawdown_bps: u16,
    pub max_daily_loss_bps: u16,
    /// Risk at the stop, as bps of equity. This is the rule no centralized firm can enforce
    /// *before* the fill, and it is only checkable because the stop is mandatory and placed in
    /// the same transaction.
    pub max_risk_per_trade_bps: u16,
    /// A stop 99% away is not a stop.
    pub max_stop_distance_bps: u16,
    pub max_concurrent_positions: u8,
    /// Bitmap over `market_index`. `u128`, so 128 markets — SolFX lists 29 today and the
    /// upper tiers plausibly reach 50–60, which left `u64` uncomfortably close.
    pub allowed_markets: u128,
    /// The no-scalping rule. Applies to **voluntary** closes only; a stop-out is exempt.
    pub min_hold_slots: u64,

    // --- economics -------------------------------------------------------------------------
    /// Trader's share of net profit. 7000 = 70%.
    pub trader_split_bps: u16,

    // --- lifecycle ---------------------------------------------------------------------------
    pub state: MandateState,
    /// How many SolFX positions this mandate currently holds open. Maintained here because
    /// SolFX stores no per-account position count, and the concurrency rule needs one.
    pub open_positions: u8,
    /// Sum of the notional of every open position, in USDC, as booked at entry.
    ///
    /// Booked at entry rather than marked, for the same reason SolFX stores `entry_notional`
    /// on a position: a figure that moves with the price would make the total drift on a
    /// mandate that merely held, and the rule is about how much exposure the trader *took*.
    pub open_notional: u64,
    /// Equity at the last observation, in USDC. Zero until the first crank.
    pub last_equity: u64,
    /// When that observation happened. The crank cadence is the honesty of the drawdown rule,
    /// so it is recorded rather than implied.
    pub last_observed_at: i64,
    /// The positions currently open, one slot each. `open_positions` and `open_notional` are
    /// kept equal to what these slots sum to; they are stored for cheap reads by the rules.
    pub slots: [PositionSlot; MAX_SLOTS],
    pub opened_at: i64,
    pub bump: u8,
    pub _reserved: [u8; 64],
}

impl Mandate {
    /// Is `market_index` in the permitted set?
    #[must_use]
    pub fn permits_market(&self, market_index: u16) -> bool {
        // Indices at or beyond the bitmap's width are not permitted rather than wrapping —
        // a shift past the width is undefined in release builds and would silently admit
        // market 128 as market 0.
        if market_index >= 128 {
            return false;
        }
        self.allowed_markets & (1u128 << market_index) != 0
    }

    /// Record a newly opened position. Fails if every slot is taken.
    pub fn book(&mut self, market_index: u16, nonce: u8, notional: u64) -> Result<()> {
        // A slot already open at this address means a position there closed without NOXFUNDS
        // seeing it — a stop-out — and was never reconciled. Booking a second would leave the
        // stale one stranded forever, so the open is refused until `reconcile_position` runs.
        require!(
            !self
                .slots
                .iter()
                .any(|s| s.open && s.market_index == market_index && s.nonce == nonce),
            crate::errors::NoxError::SlotNotReconciled
        );
        let slot = self
            .slots
            .iter_mut()
            .find(|s| !s.open)
            .ok_or(crate::errors::NoxError::TooManyOpenPositions)?;
        *slot = PositionSlot {
            open: true,
            market_index,
            nonce,
            notional,
        };
        self.open_positions = self.open_positions.saturating_add(1);
        self.open_notional = self
            .open_notional
            .checked_add(notional)
            .ok_or(crate::errors::NoxError::MathOverflow)?;
        Ok(())
    }

    /// Release a position, returning the notional it was booked at.
    ///
    /// Releases the **booked** figure, not anything recomputed at close, so the book returns
    /// exactly to where it was before the position opened.
    pub fn unbook(&mut self, market_index: u16, nonce: u8) -> Result<u64> {
        let slot = self
            .slots
            .iter_mut()
            .find(|s| s.open && s.market_index == market_index && s.nonce == nonce)
            .ok_or(crate::errors::NoxError::PositionNotTracked)?;
        let notional = slot.notional;
        *slot = PositionSlot::default();
        self.open_positions = self.open_positions.saturating_sub(1);
        self.open_notional = self.open_notional.saturating_sub(notional);
        Ok(notional)
    }

    /// Equity, in USDC, as the drawdown rule measures it.
    ///
    /// Stage 1 marks a mandate at its vault balance plus capital deployed into SolFX. Stage 4
    /// replaces this with `solfx_core::risk::assess`, which marks open positions on the
    /// adverse side — the same number a liquidation would use.
    /// Returns `u64::MAX` only on an arithmetic failure that cannot occur for real balances —
    /// a drawdown that large reads as "breached", which is the safe direction.
    #[must_use]
    pub fn drawdown_bps(&self, equity_now: u64) -> u64 {
        if self.peak_equity == 0 || equity_now >= self.peak_equity {
            return 0;
        }
        let fallen = self.peak_equity.saturating_sub(equity_now);
        // Ceiling: a drawdown rounded down is a breach that has not been noticed yet. Through
        // `solfx-math` rather than by hand, so the widening to `u128` and the rounding
        // direction are the venue's and not a second opinion.
        solfx_math::fixed::mul_div_ceil(
            u128::from(fallen),
            u128::from(crate::constants::BPS),
            u128::from(self.peak_equity),
        )
        .and_then(solfx_math::fixed::to_u64)
        .unwrap_or(u64::MAX)
    }
}
