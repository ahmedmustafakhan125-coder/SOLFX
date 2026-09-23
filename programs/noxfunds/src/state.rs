//! On-chain state.
//!
//! `Mandate` is the centre of the design: it is the rule set an investor fixed at funding, and
//! it is the thing a funded trade is checked against before the fill.

use anchor_lang::prelude::*;

use crate::constants::{MAX_NICKNAME_LEN, MAX_NOTE_LEN};

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
    /// Bump of the mandate's USDC vault at `["vault", mandate]`, stored so every instruction that
    /// touches the vault re-checks its address without re-deriving it.
    pub vault_bump: u8,

    // --- carved from `_reserved` on 2026-09-23; `INIT_SPACE` is unchanged -------------------
    /// SolFX free collateral as NOXFUNDS last accounted for it.
    ///
    /// Moved only by the effect of NOXFUNDS' own instructions — never re-read from the account —
    /// so `free_collateral - last_free_collateral` is always exactly what positions closed
    /// *outside* NOXFUNDS (a stop, a take-profit, a liquidation) have credited back. That is how
    /// `reconcile_position` learns what a stop-out made after the position account is gone.
    /// Signed because a trader may spend a credit before it is reconciled.
    pub last_free_collateral: i64,
    /// Margin plus open fee of every tracked open position: what SolFX debited to open them.
    pub booked_margin_fees: u64,
    /// The UTC day `day_start_equity` belongs to, as `unix_timestamp / 86_400`.
    pub day: i64,
    /// Equity when `day` began, as last observed — the daily loss limit's baseline.
    pub day_start_equity: u64,
    /// Net result of every trade recorded against this mandate. Decides whether it settled in
    /// profit, because the vault balance can be inflated by anyone sending USDC to it.
    pub realized_pnl: i64,
    pub _reserved: [u8; 23],
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

    /// Account for a position NOXFUNDS closed itself — voluntarily or by wind-down.
    ///
    /// `margin` and `open_fee` are what opening it debited, and so what `book` added to
    /// `booked_margin_fees`; `credit` is what the close returned to free collateral; `realized`
    /// is the trade's net result. Every figure is NOXFUNDS' own effect, so the invariant that
    /// `free_collateral - last_free_collateral` equals unreconciled external credits survives.
    pub fn release_close(
        &mut self,
        margin: u64,
        open_fee: u64,
        credit: u64,
        realized: i64,
    ) -> Result<()> {
        let booked = margin
            .checked_add(open_fee)
            .ok_or(crate::errors::NoxError::MathOverflow)?;
        self.booked_margin_fees = self
            .booked_margin_fees
            .checked_sub(booked)
            .ok_or(crate::errors::NoxError::MathOverflow)?;
        self.last_free_collateral = self
            .last_free_collateral
            .checked_add(i64::try_from(credit).map_err(|_| crate::errors::NoxError::MathOverflow)?)
            .ok_or(crate::errors::NoxError::MathOverflow)?;
        self.realized_pnl = self
            .realized_pnl
            .checked_add(realized)
            .ok_or(crate::errors::NoxError::MathOverflow)?;
        Ok(())
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

// --- the track record -----------------------------------------------------------------------

/// What a trader has earned the right to be trusted with.
///
/// Assigned by [`TraderTier::for_stats`], recomputed on demand, and **able to fall**. A tier
/// that only ever rises is a tier that rewards getting lucky once; recomputation after every
/// settled mandate is what makes copying your way to funded and then trading badly cost
/// something.
#[derive(
    AnchorSerialize,
    AnchorDeserialize,
    InitSpace,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Debug,
)]
pub enum TraderTier {
    /// The entry tier. A profile exists; nothing has been proven.
    #[default]
    Bronze,
    Silver,
    Gold,
    Platinum,
}

impl TraderTier {
    /// The largest mandate an investor may open for a trader at this tier, in USDC.
    ///
    /// This is the only place the tier does anything binding. Everything else it drives —
    /// ordering in the marketplace, what a dashboard shows — is presentation.
    #[must_use]
    pub const fn max_mandate(self) -> u64 {
        match self {
            Self::Bronze => 10_000_000_000,    // $10,000
            Self::Silver => 25_000_000_000,    // $25,000
            Self::Gold => 75_000_000_000,      // $75,000
            Self::Platinum => 200_000_000_000, // $200,000
        }
    }

    /// How many mandates a trader may hold open at once.
    ///
    /// Concurrency is capped separately from size because the two failures differ: one
    /// oversized mandate is one investor's loss, while five simultaneous mandates is a trader
    /// whose attention is divided across capital that several investors each believed was being
    /// watched.
    #[must_use]
    pub const fn max_concurrent_mandates(self) -> u32 {
        match self {
            Self::Bronze => 1,
            Self::Silver => 2,
            Self::Gold => 3,
            Self::Platinum => 5,
        }
    }
}

/// A trader's record, at `["trader", authority]`. One per trader, spanning every mandate.
///
/// # Why NOXFUNDS has to store this at all
///
/// SolFX stores **no** per-account PnL, trade count, win count or drawdown. Those figures exist
/// only in its event stream, and a program cannot read events. So every statistic an investor
/// would judge a trader on has to be accumulated here, at the moment each trade closes, by the
/// instruction that closes it.
///
/// # Why gross profit and gross loss are stored separately
///
/// Net PnL alone cannot distinguish a trader who made $10,000 and lost $9,000 from one who made
/// $1,000 and lost nothing. Profit factor — the ratio of the two — is the statistic that
/// separates them, and it is only derivable if both sides are kept.
#[account]
#[derive(InitSpace)]
pub struct TraderProfile {
    pub authority: Pubkey,
    pub tier: TraderTier,

    pub trades: u32,
    pub wins: u32,
    pub losses: u32,

    /// Sum of every winning trade's realised PnL, net of fees, in USDC.
    pub gross_profit: u64,
    /// Sum of the absolute value of every losing trade's realised PnL, in USDC.
    pub gross_loss: u64,
    /// Kept so one lucky trade cannot masquerade as a record. A $50,000 profit factor built
    /// from a single trade reads very differently beside `largest_win`.
    pub largest_win: u64,
    pub largest_loss: u64,

    /// Sum of every closed trade's hold time, in slots. Divided by `trades` this is the average
    /// hold — the anti-scalping statistic, and the one a copy-trader cannot fake without
    /// actually holding.
    pub total_hold_slots: u128,

    /// Worst drawdown ever observed across all of this trader's mandates, in bps.
    ///
    /// Updated by the permissionless equity crank as well as by closes, which is the point: a
    /// trader cannot hide a drawdown by refusing to close the losing position, because the
    /// crank marks it anyway.
    pub max_drawdown_bps: u16,

    pub mandates_funded: u32,
    /// Mandates currently open. Bounded by the tier's `max_concurrent_mandates`.
    pub active_mandates: u32,
    /// Mandates that reached `Settled` with final equity above principal.
    pub mandates_settled_in_profit: u32,

    pub created_at: i64,
    pub bump: u8,

    // --- carved from `_reserved` on 2026-09-23; `INIT_SPACE` is unchanged -------------------
    /// Trades recorded with no measurable hold: a stop-out seen only after its position account
    /// was gone. Excluded from the average hold rather than counted as zero-length scalps.
    pub untimed_trades: u32,
    /// Trades that closed outside NOXFUNDS together with another, so how the combined result
    /// divides between them cannot be known. Published, because the record resolves that
    /// ambiguity against the trader and anyone reading it should be able to see how often.
    pub ambiguous_trades: u32,
    pub _reserved: [u8; 56],
}

impl TraderProfile {
    /// Profit factor at `BPS`: `gross_profit / gross_loss`. 12_000 is 1.2×.
    ///
    /// Floor-rounded, so a trader sitting exactly on a threshold does not cross it on a
    /// rounding artefact — adverse to whoever is claiming the tier, which is the same direction
    /// every other rounding in this codebase takes.
    ///
    /// With no losses at all the ratio is unbounded; it returns `u64::MAX` when there is profit
    /// and `0` when there is neither. A trader with two winning trades and no losses therefore
    /// reads as infinitely good on this one statistic, which is exactly why the tier rules also
    /// demand a trade count.
    #[must_use]
    pub fn profit_factor_bps(&self) -> u64 {
        if self.gross_loss == 0 {
            return if self.gross_profit == 0 { 0 } else { u64::MAX };
        }
        solfx_math::fixed::mul_div_floor(
            u128::from(self.gross_profit),
            u128::from(crate::constants::BPS),
            u128::from(self.gross_loss),
        )
        .and_then(solfx_math::fixed::to_u64)
        .unwrap_or(u64::MAX)
    }

    /// Win rate at `BPS`. Floor-rounded, for the same reason.
    #[must_use]
    pub fn win_rate_bps(&self) -> u64 {
        if self.trades == 0 {
            return 0;
        }
        solfx_math::fixed::mul_div_floor(
            u128::from(self.wins),
            u128::from(crate::constants::BPS),
            u128::from(self.trades),
        )
        .and_then(solfx_math::fixed::to_u64)
        .unwrap_or(0)
    }

    /// Fold one closed trade into the record.
    ///
    /// `realized_pnl` is net of fees and signed. Zero counts as a **loss**, not a win: a trade
    /// that netted exactly nothing after fees did not make money, and rounding it into the win
    /// column is how a win rate starts to flatter.
    pub fn record_trade(&mut self, realized_pnl: i64, hold_slots: u64) -> Result<()> {
        self.trades = self.trades.saturating_add(1);
        self.total_hold_slots = self.total_hold_slots.saturating_add(u128::from(hold_slots));

        if realized_pnl > 0 {
            let win =
                u64::try_from(realized_pnl).map_err(|_| crate::errors::NoxError::MathOverflow)?;
            self.wins = self.wins.saturating_add(1);
            self.gross_profit = self
                .gross_profit
                .checked_add(win)
                .ok_or(crate::errors::NoxError::MathOverflow)?;
            self.largest_win = self.largest_win.max(win);
        } else {
            // `unsigned_abs` rather than `-pnl`: negating `i64::MIN` overflows, and the
            // workspace denies the arithmetic that would let it through silently.
            let loss = realized_pnl.unsigned_abs();
            self.losses = self.losses.saturating_add(1);
            self.gross_loss = self
                .gross_loss
                .checked_add(loss)
                .ok_or(crate::errors::NoxError::MathOverflow)?;
            self.largest_loss = self.largest_loss.max(loss);
        }
        Ok(())
    }

    /// Average hold time in slots, over the trades whose hold was measured. Zero before one.
    ///
    /// A stop-out reconciled after the fact has no opening slot left to measure from; counting
    /// it as a zero-length hold would make a disciplined trader look like a scalper.
    #[must_use]
    pub fn avg_hold_slots(&self) -> u64 {
        let timed = self.trades.saturating_sub(self.untimed_trades);
        if timed == 0 {
            return 0;
        }
        solfx_math::fixed::mul_div_floor(self.total_hold_slots, 1, u128::from(timed))
            .and_then(solfx_math::fixed::to_u64)
            .unwrap_or(0)
    }
}

impl TraderTier {
    /// The tier these statistics earn. Pure, total, and the only place tier is decided.
    ///
    /// Each tier's requirements are **cumulative**: Gold demands everything Silver does and
    /// more. Written as a descending chain rather than four independent predicates so that
    /// cannot drift — a trader who qualifies for Platinum necessarily qualifies for Silver.
    #[must_use]
    pub fn for_stats(p: &TraderProfile) -> Self {
        use crate::constants as c;
        let pf = p.profit_factor_bps();
        let wr = p.win_rate_bps();

        let silver = p.trades >= c::SILVER_MIN_TRADES
            && wr >= c::SILVER_MIN_WIN_RATE_BPS
            && pf >= c::SILVER_MIN_PROFIT_FACTOR_BPS;
        if !silver {
            return Self::Bronze;
        }

        let gold = p.trades >= c::GOLD_MIN_TRADES
            && pf >= c::GOLD_MIN_PROFIT_FACTOR_BPS
            && p.max_drawdown_bps <= c::GOLD_MAX_DRAWDOWN_BPS;
        if !gold {
            return Self::Silver;
        }

        let platinum = p.trades >= c::PLATINUM_MIN_TRADES
            && pf >= c::PLATINUM_MIN_PROFIT_FACTOR_BPS
            && p.mandates_settled_in_profit >= c::PLATINUM_MIN_PROFITABLE_MANDATES;
        if platinum {
            Self::Platinum
        } else {
            Self::Gold
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::integer_division
)]
mod tier_tests {
    use super::*;
    use crate::constants as c;

    /// A profile with nothing on it. Each test moves only the fields it is about.
    fn blank() -> TraderProfile {
        TraderProfile {
            authority: Pubkey::default(),
            tier: TraderTier::Bronze,
            trades: 0,
            wins: 0,
            losses: 0,
            gross_profit: 0,
            gross_loss: 0,
            largest_win: 0,
            largest_loss: 0,
            total_hold_slots: 0,
            max_drawdown_bps: 0,
            mandates_funded: 0,
            active_mandates: 0,
            mandates_settled_in_profit: 0,
            created_at: 0,
            bump: 0,
            untimed_trades: 0,
            ambiguous_trades: 0,
            _reserved: [0; 56],
        }
    }

    /// A profile that clears Silver exactly, with room to be pushed either way.
    fn silver() -> TraderProfile {
        let mut p = blank();
        p.trades = c::SILVER_MIN_TRADES;
        p.wins = 9; // 9/20 = 4500 bps, exactly the floor
        p.losses = 11;
        p.gross_profit = 12_000;
        p.gross_loss = 10_000; // 1.2×, exactly the floor
        p
    }

    #[test]
    fn a_new_profile_is_bronze_and_nothing_else() {
        assert_eq!(TraderTier::for_stats(&blank()), TraderTier::Bronze);
    }

    /// **The boundaries are inclusive.** A trader sitting exactly on a threshold clears it;
    /// one unit below does not. Both halves, because only asserting the pass would let the
    /// comparison silently become `>` without failing anything.
    #[test]
    fn silver_is_inclusive_on_every_threshold() {
        assert_eq!(TraderTier::for_stats(&silver()), TraderTier::Silver);

        let mut one_trade_short = silver();
        one_trade_short.trades = c::SILVER_MIN_TRADES - 1;
        assert_eq!(TraderTier::for_stats(&one_trade_short), TraderTier::Bronze);

        let mut one_win_short = silver();
        one_win_short.wins = 8; // 8/20 = 4000 bps
        assert_eq!(TraderTier::for_stats(&one_win_short), TraderTier::Bronze);

        let mut pf_short = silver();
        pf_short.gross_profit = 11_999; // 1.1999×, floors to 11_999 bps
        assert_eq!(TraderTier::for_stats(&pf_short), TraderTier::Bronze);
    }

    #[test]
    fn gold_needs_a_shallow_drawdown_as_well_as_a_profit_factor() {
        let mut p = silver();
        p.trades = c::GOLD_MIN_TRADES;
        p.wins = 25;
        p.losses = 25;
        p.gross_profit = 15_000; // 1.5×
        p.max_drawdown_bps = c::GOLD_MAX_DRAWDOWN_BPS;
        assert_eq!(TraderTier::for_stats(&p), TraderTier::Gold);

        // One basis point deeper and the same trader is Silver. This is the rule that a
        // profitable-but-violent record does not clear.
        let mut deep = p;
        deep.max_drawdown_bps = c::GOLD_MAX_DRAWDOWN_BPS + 1;
        assert_eq!(TraderTier::for_stats(&deep), TraderTier::Silver);
    }

    #[test]
    fn platinum_needs_settled_mandates_not_only_statistics() {
        let mut p = blank();
        p.trades = c::PLATINUM_MIN_TRADES;
        p.wins = 60;
        p.losses = 40;
        p.gross_profit = 18_000;
        p.gross_loss = 10_000; // 1.8×
        p.max_drawdown_bps = 100;
        // Everything but the live results.
        p.mandates_settled_in_profit = c::PLATINUM_MIN_PROFITABLE_MANDATES - 1;
        assert_eq!(TraderTier::for_stats(&p), TraderTier::Gold);

        p.mandates_settled_in_profit = c::PLATINUM_MIN_PROFITABLE_MANDATES;
        assert_eq!(TraderTier::for_stats(&p), TraderTier::Platinum);
    }

    /// **The statistic this whole scheme exists to refuse.**
    ///
    /// A 90% win rate is the most persuasive number a bad trader can show. Paired with a
    /// profit factor below 1 it describes someone taking tiny wins and enormous losses, and
    /// no tier above Bronze accepts it.
    #[test]
    fn a_ninety_percent_win_rate_with_a_losing_profit_factor_stays_bronze() {
        let mut p = blank();
        p.trades = 100;
        p.wins = 90;
        p.losses = 10;
        p.gross_profit = 9_000; // 90 wins of 100
        p.gross_loss = 15_000; // 10 losses of 1,500
        assert_eq!(p.win_rate_bps(), 9_000);
        assert!(p.profit_factor_bps() < 10_000, "this trader loses money");
        assert_eq!(TraderTier::for_stats(&p), TraderTier::Bronze);
    }

    #[test]
    fn profit_factor_floors_and_handles_a_record_with_no_losses() {
        let mut p = blank();
        assert_eq!(p.profit_factor_bps(), 0, "nothing traded is not infinite");

        p.gross_profit = 500;
        assert_eq!(
            p.profit_factor_bps(),
            u64::MAX,
            "wins and no losses is unbounded"
        );

        p.gross_loss = 3;
        p.gross_profit = 10;
        // 10/3 = 3.333…×; floored to 33_333 bps, never rounded up to the trader's benefit.
        assert_eq!(p.profit_factor_bps(), 33_333);
    }

    #[test]
    fn a_trade_that_nets_exactly_zero_counts_as_a_loss() {
        let mut p = blank();
        p.record_trade(0, 10).unwrap();
        assert_eq!(p.losses, 1, "breaking even after fees is not a win");
        assert_eq!(p.wins, 0);
        assert_eq!(p.gross_loss, 0);
        assert_eq!(p.win_rate_bps(), 0);
    }

    #[test]
    fn record_trade_accumulates_both_sides_and_the_extremes() {
        let mut p = blank();
        p.record_trade(300, 50).unwrap();
        p.record_trade(-100, 150).unwrap();
        p.record_trade(700, 100).unwrap();

        assert_eq!((p.trades, p.wins, p.losses), (3, 2, 1));
        assert_eq!(p.gross_profit, 1_000);
        assert_eq!(p.gross_loss, 100);
        assert_eq!(p.largest_win, 700);
        assert_eq!(p.largest_loss, 100);
        assert_eq!(p.total_hold_slots, 300);
        assert_eq!(p.avg_hold_slots(), 100);
        assert_eq!(p.profit_factor_bps(), 100_000); // 10×
    }

    /// `i64::MIN` has no positive counterpart, so negating it overflows. `unsigned_abs` is why
    /// this does not panic or wrap — a value no real trade produces, but the arithmetic has to
    /// be total anyway.
    #[test]
    fn the_worst_possible_loss_does_not_overflow() {
        let mut p = blank();
        p.record_trade(i64::MIN, 1).unwrap();
        assert_eq!(p.gross_loss, 9_223_372_036_854_775_808);
        assert_eq!(p.losses, 1);
    }

    /// Tiers are a descending chain, so anything that earns a high tier necessarily satisfies
    /// every lower one. Swept rather than spot-checked, because the failure mode is a
    /// threshold that only one of the four branches reads.
    #[test]
    fn the_tier_ladder_never_skips_a_rung() {
        for trades in [0u32, 19, 20, 49, 50, 99, 100, 250] {
            for pf_num in [0u64, 9_000, 12_000, 15_000, 18_000, 40_000] {
                for dd in [0u16, 400, 401, 5_000] {
                    for settled in [0u32, 1, 2] {
                        let mut p = blank();
                        p.trades = trades;
                        p.wins = trades / 2 + trades % 2; // ≥ 50%, clears the win-rate gate
                        p.losses = trades - p.wins;
                        p.gross_loss = 10_000;
                        p.gross_profit = pf_num;
                        p.max_drawdown_bps = dd;
                        p.mandates_settled_in_profit = settled;

                        let tier = TraderTier::for_stats(&p);
                        // Each tier's own limits must rise with it — the only thing the tier
                        // is allowed to change.
                        if tier == TraderTier::Platinum {
                            assert!(trades >= c::PLATINUM_MIN_TRADES);
                            assert!(settled >= c::PLATINUM_MIN_PROFITABLE_MANDATES);
                        }
                        if tier >= TraderTier::Gold {
                            assert!(dd <= c::GOLD_MAX_DRAWDOWN_BPS);
                            assert!(trades >= c::GOLD_MIN_TRADES);
                        }
                        if tier >= TraderTier::Silver {
                            assert!(trades >= c::SILVER_MIN_TRADES);
                            assert!(p.profit_factor_bps() >= c::SILVER_MIN_PROFIT_FACTOR_BPS);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_tier_raises_both_of_its_limits() {
        let ladder = [
            TraderTier::Bronze,
            TraderTier::Silver,
            TraderTier::Gold,
            TraderTier::Platinum,
        ];
        for pair in ladder.windows(2) {
            let [lower, higher] = pair else {
                unreachable!()
            };
            assert!(higher.max_mandate() > lower.max_mandate());
            assert!(higher.max_concurrent_mandates() > lower.max_concurrent_mandates());
        }
    }
}

// --- the marketplace ---------------------------------------------------------------------
//
// # Why the marketplace is an escrow and not a chat
//
// Two parties who do not trust each other need to agree terms and then have those terms bind.
// Solana's canonical answer to exactly that shape is the escrow: a **maker** locks value and
// publishes terms, a **taker** fulfils them atomically, and the maker can refund while nobody
// has taken. Three reference implementations converge on it — `anchor/tests/escrow`,
// `solana-bootcamp-2026/04-escrow` and `program-examples/tokens/escrow` — and every listing
// venue on Solana is a variation of it.
//
// So `MandateOffer` *is* the negotiation. The investor's terms are the message, the escrowed
// USDC is the proof the message is serious, and the trader's acceptance is the signature on the
// contract. Nothing needs to be said in words for the binding part, which is why there is no
// messaging protocol here: a chat would be public forever, unencrypted, individually
// rent-bearing, and spammable by anyone who can afford an account.
//
// What is left for words is a single bounded `note` on each side. It rides on an account that
// already exists and already pays rent, so it adds no new surface.

/// A trader advertising for capital, at `["listing", trader]`.
///
/// Carries no money and takes no custody. It is an index entry: it exists so an investor
/// browsing the marketplace can find a trader, read their terms, and cross-reference the
/// `TraderProfile` at `["trader", trader]` — which the trader does not control and cannot
/// curate.
#[account]
#[derive(InitSpace)]
pub struct TraderListing {
    pub trader: Pubkey,

    /// The band of principal this trader will accept. An investor offering outside it is not
    /// refused on chain — the listing is advertising, not a rule — but the frontend can grey
    /// the offer form out, and a mismatch is visible to both sides before anyone signs.
    pub min_principal: u64,
    pub max_principal: u64,

    /// The markets the trader is asking to be allowed. Bitmap over `market_index`, same shape
    /// as `Mandate::allowed_markets`.
    pub wanted_markets: u128,

    /// The split the trader is asking for, in bps of net profit. Advisory: the binding number
    /// is the one on the offer the investor actually signs.
    pub wanted_split_bps: u16,

    pub note: [u8; MAX_NOTE_LEN],
    pub note_len: u8,

    /// Closed listings stay on chain rather than being deleted, so a trader cannot quietly
    /// withdraw a listing an investor is midway through responding to and leave a dangling
    /// reference. Reopening is a single flag.
    pub open: bool,

    pub created_at: i64,
    pub updated_at: i64,
    pub bump: u8,
    /// A display name, so the marketplace reads as people rather than as base58.
    ///
    /// Carved out of the reserved bytes rather than appended, so `INIT_SPACE` does not move: every
    /// listing already on chain stays valid and decodes with an empty nickname, because those
    /// bytes are zero. Growing the account instead would have meant a realloc, a migration, and
    /// rent nobody agreed to. The address stays beside it everywhere — a name is a convenience,
    /// never an identity, and two traders may choose the same one.
    pub nickname: [u8; MAX_NICKNAME_LEN],
    pub nickname_len: u8,
    pub _reserved: [u8; 7],
}

/// Where an offer is in its life.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum OfferState {
    /// Escrowed and waiting for the trader.
    Open,
    /// The trader signed; a `Mandate` exists and the escrow is empty.
    Accepted,
    /// The investor took it back. Terminal.
    Revoked,
    /// The trader said no, with a reason in `MandateOffer::reply`. The principal is still in
    /// escrow and the investor revokes to take it back — declining never moves money, so a
    /// trader cannot use it to strand or redirect anyone's capital.
    ///
    /// Appended rather than inserted: the variant order is the on-chain discriminant.
    Declined,
}

/// An investor's escrowed proposal to one trader, at `["offer", investor, trader, seq]`.
///
/// The full rule set is fixed here, before the trader has agreed to anything, and it is copied
/// onto the `Mandate` verbatim on acceptance. So the terms the trader accepts are provably the
/// terms the investor published: there is no step between agreement and funding in which either
/// side could substitute a different number.
#[account]
#[derive(InitSpace)]
pub struct MandateOffer {
    pub investor: Pubkey,
    /// The one trader who may accept. Offers are addressed, not open to the highest bidder —
    /// an open offer would let any trader with a profile take capital an investor had picked
    /// someone specific for.
    pub trader: Pubkey,
    pub seq: u8,

    /// Escrowed at `["offer_vault", offer]` from the moment the offer is posted.
    pub principal: u64,

    // --- the rules, identical in shape to `Mandate`'s and copied across on acceptance ------
    pub max_trade_notional: u64,
    pub max_total_notional: u64,
    pub max_drawdown_bps: u16,
    pub max_daily_loss_bps: u16,
    pub max_risk_per_trade_bps: u16,
    pub max_stop_distance_bps: u16,
    pub max_concurrent_positions: u8,
    pub allowed_markets: u128,
    pub min_hold_slots: u64,
    pub trader_split_bps: u16,

    pub note: [u8; MAX_NOTE_LEN],
    pub note_len: u8,

    /// After this, the trader can no longer accept and the investor can always revoke.
    ///
    /// An offer with no expiry is capital an investor can lose track of; an expiry the trader
    /// can ignore is not an expiry. Acceptance checks it against the cluster clock.
    pub expires_at: i64,

    pub state: OfferState,
    pub created_at: i64,
    pub bump: u8,
    pub vault_bump: u8,

    /// The trader's answer when they decline. Empty otherwise.
    ///
    /// On the offer rather than in an account of its own, so the reason is read in the same
    /// place as the terms it refers to, and so declining costs the trader no rent.
    pub reply: [u8; MAX_NOTE_LEN],
    pub reply_len: u8,
    pub _reserved: [u8; 32],
}

impl MandateOffer {
    /// The note as text, or an empty string if it is not valid UTF-8.
    ///
    /// Total rather than fallible on purpose: a note is decoration, and a client that cannot
    /// render one should still be able to render the offer.
    #[must_use]
    pub fn note_str(&self) -> &str {
        let len = (self.note_len as usize).min(MAX_NOTE_LEN);
        match self.note.get(..len) {
            Some(bytes) => core::str::from_utf8(bytes).unwrap_or(""),
            None => "",
        }
    }
}

/// An investor advertising capital, at `["inv_listing", investor]`.
///
/// The mirror of `TraderListing`. It commits nothing and escrows nothing — the binding step is
/// still an offer with the capital locked behind it. It exists so a trader browsing the
/// marketplace can find investors, and so a trader has a legitimate address to send a
/// `FundingRequest` to: requests are only accepted by an investor with an open listing.
#[account]
#[derive(InitSpace)]
pub struct InvestorListing {
    pub investor: Pubkey,

    /// The band of principal this investor is prepared to put behind one trader.
    pub min_principal: u64,
    pub max_principal: u64,

    // --- the terms on offer. Advisory: the binding numbers are on the offer itself. --------
    pub max_drawdown_bps: u16,
    pub max_risk_per_trade_bps: u16,
    pub allowed_markets: u128,
    pub offered_split_bps: u16,

    pub note: [u8; MAX_NOTE_LEN],
    pub note_len: u8,
    pub open: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub bump: u8,
    /// A display name, so the marketplace reads as people rather than as base58.
    ///
    /// Carved out of the reserved bytes rather than appended, so `INIT_SPACE` does not move: every
    /// listing already on chain stays valid and decodes with an empty nickname, because those
    /// bytes are zero. Growing the account instead would have meant a realloc, a migration, and
    /// rent nobody agreed to. The address stays beside it everywhere — a name is a convenience,
    /// never an identity, and two traders may choose the same one.
    pub nickname: [u8; MAX_NICKNAME_LEN],
    pub nickname_len: u8,
    pub _reserved: [u8; 7],
}

/// A trader asking one investor for capital, at `["request", trader, investor]`.
///
/// The trader-initiated half of the conversation. It carries no money; the investor answers by
/// posting a `MandateOffer`, which is the only thing that can bind either of them. One open
/// request per pair — the seeds make a second one impossible, so a trader cannot flood an
/// investor's inbox with copies.
///
/// The trader pays the rent and always gets it back, whichever side closes it.
#[account]
#[derive(InitSpace)]
pub struct FundingRequest {
    pub trader: Pubkey,
    pub investor: Pubkey,
    pub wanted_principal: u64,
    pub wanted_split_bps: u16,
    pub note: [u8; MAX_NOTE_LEN],
    pub note_len: u8,
    pub created_at: i64,
    pub bump: u8,
    pub _reserved: [u8; 16],
}

// --- the evaluation (Stage 3) ----------------------------------------------------------------
//
// # A simulation priced by the real venue's own code
//
// An evaluation holds a virtual USDC balance. Its trades are priced by the functions a real
// SolFX fill uses — `load_validated_price`, `execution_price_for`, the `solfx_math` notional,
// PnL, fee and carry functions — called, not copied. So a simulated fill is not an
// approximation of a SolFX fill; on the same price update and the same market state it *is*
// one, to the unit, and `tests/stage3.rs` holds that.
//
// No token moves in the simulation. The only token account an evaluation ever touches is its
// own stake escrow, which is what keeps SolFX's invariants untouched by construction.

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum EvaluationState {
    Active,
    /// Both phases passed. Terminal; the stake was refunded.
    Passed,
    /// A rule broke, or the trader walked away. Terminal; the stake is forfeit.
    Failed,
}

/// Why an evaluation failed, recorded on the account and in the event. Never a generic
/// "rule violation" — a trader who failed must be able to read which rule ended it.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum EvalRule {
    None,
    DailyLoss,
    Drawdown,
    Abandoned,
}

#[account]
#[derive(InitSpace)]
pub struct Evaluation {
    pub trader: Pubkey,
    pub seq: u8,
    /// 1 or 2.
    pub stage: u8,
    pub state: EvaluationState,
    pub failed_rule: EvalRule,

    /// The simulated starting balance for the current stage, in USDC.
    pub account_size: u64,
    /// Realised cash: starting balance plus every closed trade's net result, minus every fee.
    /// Signed, because a gap through a stop can in principle take it below zero and the record
    /// must not wrap.
    pub balance: i64,
    /// High-water mark of equity. Only ever rises.
    pub peak_equity: u64,

    /// The UTC day (`unix_timestamp / 86_400`) that `day_start_equity` belongs to.
    pub day: i64,
    pub day_start_equity: u64,
    /// Realised result so far today, and the best single day of the stage — the consistency
    /// rule's inputs.
    pub day_pnl: i64,
    pub best_day_pnl: i64,

    // --- the record ---------------------------------------------------------------------
    pub trades: u32,
    pub wins: u32,
    pub losses: u32,
    pub gross_profit: u64,
    pub gross_loss: u64,
    /// Voluntary closes only: stop-outs are exempt from the hold rules (§3.2.1).
    pub voluntary_closes: u32,
    pub voluntary_hold_secs: u64,
    pub trading_days: u16,
    pub last_trade_day: i64,

    pub open_positions: u8,
    pub last_equity: u64,
    pub last_observed_at: i64,
    pub started_at: i64,
    pub bump: u8,
    pub vault_bump: u8,
    pub _reserved: [u8; 64],
}

impl Evaluation {
    /// Equity as a non-negative figure: what the simulated account would be worth, floored at
    /// zero because an account cannot be worth less than nothing.
    #[must_use]
    pub fn floor_equity(value: i128) -> u64 {
        u64::try_from(value.max(0)).unwrap_or(u64::MAX)
    }

    /// Trailing drawdown from the peak, in bps, rounded **up** — a limit the account must stay
    /// under, so rounding down would admit a drawdown fractionally past it. Returns `u64::MAX` on
    /// an arithmetic failure, which every caller reads as "breached".
    #[must_use]
    pub fn drawdown_bps(&self, equity: u64) -> u64 {
        let peak = self.peak_equity.max(1);
        let lost = peak.saturating_sub(equity);
        solfx_math::fixed::mul_div_ceil(
            u128::from(lost),
            u128::from(crate::constants::BPS),
            u128::from(peak),
        )
        .and_then(solfx_math::fixed::to_u64)
        .unwrap_or(u64::MAX)
    }

    /// Loss since the start of today, in bps of the day's opening equity, rounded up.
    #[must_use]
    pub fn daily_loss_bps(&self, equity: u64) -> u64 {
        let start = self.day_start_equity.max(1);
        let lost = start.saturating_sub(equity);
        solfx_math::fixed::mul_div_ceil(
            u128::from(lost),
            u128::from(crate::constants::BPS),
            u128::from(start),
        )
        .and_then(solfx_math::fixed::to_u64)
        .unwrap_or(u64::MAX)
    }
}

/// One simulated position, at `["vpos", evaluation, market_index (LE), nonce]`.
///
/// Mirrors the fields of SolFX's `Position` that pricing and carry read, so closing it runs the
/// same arithmetic a real close runs.
#[account]
#[derive(InitSpace)]
pub struct VirtualPosition {
    pub evaluation: Pubkey,
    pub market_index: u16,
    pub nonce: u8,
    pub direction: solfx_core::state::Direction,
    pub size_base: u64,
    /// The execution price — spread and skew included — exactly as `open_position` fills.
    pub entry_price: i64,
    /// Notional at entry, in USDC. The carry basis, as on a real position.
    pub entry_notional: u64,
    pub stop_price: i64,
    pub open_fee: u64,
    /// `Market::cum_borrow_index` at entry, so carry accrues exactly as a real position's does.
    pub cum_borrow_entry: u128,
    pub opened_at: i64,
    pub bump: u8,
}
