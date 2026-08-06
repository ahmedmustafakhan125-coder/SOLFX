use anchor_lang::prelude::*;
use solfx_math::QuoteConversion;

use crate::constants::{
    MAX_ALLOWED_CONF_BPS, MAX_ALLOWED_LEVERAGE, MAX_ALLOWED_STALENESS_SECONDS, MAX_SYMBOL_LEN,
    SKEW_CAP_MIN_IMBALANCE_BPS_OF_AUM,
};
use crate::errors::SolfxError;
use crate::state::position::Direction;

/// Seconds in a day, for session-boundary validation.
const SECONDS_PER_DAY: u32 = 86_400;

/// Which regime state machine a market follows (`ARCHITECTURE.md` § 7.3).
///
/// A new asset class is a new variant plus its regime rules — never new maths. That is the
/// § 5.6 generic-engine constraint expressed as a type.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum FeedKind {
    /// Session-bound. Halts outside its own market hours. FX majors, EM FX **and metals** —
    /// Phase 0b measured all 137 Pyth Metal and Commodities feeds frozen at the Friday
    /// close, so metals are session-bound like everything else.
    SpotFx,
    /// 24/7 index feed; `WeekendMode` derating applies.
    ///
    /// **No feed currently qualifies.** Reserved against open question Q1b — whether Pyth
    /// Indices is separately entitled. It costs nothing to keep, and if Q1b resolves
    /// favourably a market flips regime by changing this one field with no upgrade.
    /// Until then `initialize_market` rejects it, so nothing can be listed against a
    /// promise the feed does not keep.
    ContinuousIndex,
    /// Natively 24/7 with no underlying session to converge to. Crypto never leaves
    /// `Active` and never derates.
    Crypto,
}

/// Where a market's price comes from (`ARCHITECTURE.md` § 3.5).
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PriceSource {
    /// One feed, read directly. Every market v1 lists.
    Direct,
    /// Composed from two legs. `invert_quote` selects division over multiplication —
    /// see `solfx_math::oracle::compose_synthetic`.
    ///
    /// Retained but off the v1 critical path: `oracle-feasibility.md` § 4 measured Pyth's
    /// ~180 native crosses as *tighter* than the majors they would be composed from, so
    /// composing one would produce a worse price while doubling the oracle failure surface.
    Synthetic { invert_quote: bool },
}

/// How to turn a PnL denominated in the market's quote currency into USDC (correction C-3).
///
/// The on-chain mirror of `solfx_math::QuoteConversion`. Getting this wrong mis-prices every
/// position by the FX rate — for USD/INR at ~88, by a factor of 88.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuoteConversionKind {
    /// Quote currency is USD. The fast path, and the only one Tier 1 markets use.
    None,
    /// Feed quotes **quote units per 1 USD** — the `USD/XXX` convention (USD/JPY 157).
    QuotePerUsd,
    /// Feed quotes **USD per 1 quote unit** — the `XXX/USD` convention (EUR/USD 1.085).
    UsdPerQuote,
}

impl From<QuoteConversionKind> for QuoteConversion {
    fn from(k: QuoteConversionKind) -> Self {
        match k {
            QuoteConversionKind::None => Self::None,
            QuoteConversionKind::QuotePerUsd => Self::QuotePerUsd,
            QuoteConversionKind::UsdPerQuote => Self::UsdPerQuote,
        }
    }
}

/// Market lifecycle (`ARCHITECTURE.md` § 5.3).
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarketStatus {
    /// Created, not yet tradeable.
    Initialized,
    /// Normal trading.
    Active,
    /// Closes and liquidations only. Set before a session close, or while winding down.
    ReduceOnly,
    /// Nothing but liquidations of already-underwater positions. Positions frozen.
    Halted,
    /// Session-bound market that has just reopened: closes and liquidations, no new opens,
    /// widened spread while the reopen gap is repriced.
    GapWindow,
    /// Continuous market trading on derated weekend parameters.
    WeekendMode,
    /// Continuous market in the 30 minutes before spot reopens: no new opens, because the
    /// index-to-spot convergence gap lands in that window.
    PreOpenWindow,
    /// Settlement only.
    Delisted,
}

impl MarketStatus {
    /// Whether a new position may be opened. Deliberately an allow-list: a status added
    /// later is closed to opens until someone writes it in here on purpose.
    #[must_use]
    pub fn allows_open(self) -> bool {
        matches!(self, Self::Active | Self::WeekendMode)
    }

    /// Whether a position may be closed or reduced by its owner.
    #[must_use]
    pub fn allows_close(self) -> bool {
        matches!(
            self,
            Self::Active
                | Self::ReduceOnly
                | Self::GapWindow
                | Self::WeekendMode
                | Self::PreOpenWindow
                | Self::Delisted
        )
    }

    /// Whether a keeper may liquidate. True in every status except `Initialized` — an
    /// underwater position must remain liquidatable even while the market is halted, or a
    /// halt would convert a bad position into bad debt.
    #[must_use]
    pub fn allows_liquidation(self) -> bool {
        !matches!(self, Self::Initialized)
    }
}

/// One tradeable instrument. PDA at `["market", market_index: u16]`.
///
/// **This struct is append-only.** Solana deserialises by byte offset, so reordering,
/// shrinking or retyping any field below corrupts every `Market` already on chain. New
/// fields take bytes from `_reserved`. There is no automated guard for this — it is
/// enforced in review (§ 5.6).
#[account]
#[derive(InitSpace)]
pub struct Market {
    pub market_index: u16,
    /// ASCII, zero-padded. `b"EURUSD\0..."`.
    pub symbol: [u8; MAX_SYMBOL_LEN],
    pub status: MarketStatus,

    // --- price sourcing (§ 3.5) ---
    pub feed_kind: FeedKind,
    pub price_source: PriceSource,
    /// Primary Pyth feed id; the base leg when synthetic.
    pub pyth_feed_id: [u8; 32],
    /// Quote leg when synthetic, otherwise zeroed.
    pub secondary_feed_id: [u8; 32],
    /// Quote-currency conversion feed for non-USD-quoted markets, otherwise zeroed (C-3).
    pub quote_conversion_feed: [u8; 32],
    /// Which direction to apply `quote_conversion_feed`. Without this the feed id alone
    /// does not say whether to multiply or divide.
    pub quote_conversion_kind: QuoteConversionKind,

    // --- session calendar, UTC (session-bound markets) ---
    // Each market carries its own. EM pairs follow *local* hours: USD/INR on Indian, USD/KRW
    // on Korean, USD/BRL 14–21 UTC. Assuming one global session would leave EM markets open
    // against a dead feed — the C-1 exploit on exactly the pairs where a devaluation gap is
    // most likely.
    pub session_open_dow: u8,
    pub session_open_seconds: u32,
    pub session_close_dow: u8,
    pub session_close_seconds: u32,

    // --- weekend overrides (continuous markets, § 7.3) ---
    // Dormant until Q1b resolves. Same instrument, materially worse risk: no spot market to
    // hedge into, thinner participation, and a certain convergence move on Monday.
    pub weekend_max_leverage: u16,
    pub weekend_oi_cap_bps: u16,
    pub weekend_spread_bps: u16,
    pub weekend_max_conf_bps: u16,

    // --- risk parameters (§ 6.3) ---
    pub max_leverage: u16,
    /// Initial margin ratio in bps. 200 = 2%.
    pub imr_bps: u16,
    /// Maintenance margin ratio in bps. 100 = 1%.
    pub mmr_bps: u16,
    pub liquidation_fee_bps: u16,
    /// Per-side open-interest caps, in **USDC** at `QUOTE_PRECISION` (§ 7.4). A cap has to
    /// be a money figure to mean anything across markets.
    pub max_oi_long: u64,
    pub max_oi_short: u64,
    /// Position size bounds, in **base-currency units** at `BASE_PRECISION` — the same scale
    /// as `Position::size_base`, not USDC. One standard lot is `100_000 * BASE_PRECISION`
    /// = 1e14.
    ///
    /// Base units rather than notional because the bound must not move when the price does:
    /// a notional cap would silently tighten on a rally and loosen on a selloff.
    pub max_position_size: u64,
    pub min_position_size: u64,

    // --- oracle guards (§ 7.1) ---
    pub max_staleness_seconds: u32,
    pub max_conf_bps: u16,
    pub max_deviation_bps: u16,

    // --- pricing (§ 6.6) ---
    pub base_spread_bps: u16,
    pub conf_spread_multiplier_bps: u16,
    pub skew_impact_bps_per_unit: u32,

    // --- fees, at RATE_PRECISION = 1e9 ---
    // Not bps: 0.8 bps is not expressible in whole basis points, and § 8.2's schedule needs
    // it (§ 5.3, "Note on fee precision").
    pub open_fee_rate: u64,
    pub close_fee_rate: u64,

    // --- funding and carry, cumulative index model (§ 6.7) ---
    // Written in Phase 4. Present now because § 5.6 forbids reordering later, and adding
    // them here costs nothing.
    pub cum_funding_long: i128,
    pub cum_funding_short: i128,
    pub cum_borrow_index: u128,
    pub last_funding_update_ts: i64,
    pub funding_rate_cap_per_hour: i64,
    /// The **protocol's markup** on carry, per hour at `RATE_PRECISION`. Always a charge, on
    /// both directions, and always non-negative.
    ///
    /// This is *only* the markup. The interest differential it sits on top of comes from
    /// `rate_base_annual` and `rate_quote_annual`, and the two are kept apart so the UI can
    /// show them as separate lines (§ 6.7).
    pub carry_rate_per_hour: i64,

    // --- live state ---
    pub oi_long: u64,
    pub oi_short: u64,
    pub base_oi_long: i128,
    pub base_oi_short: i128,
    /// Last spot price accepted by the validated read path, at `PRICE_PRECISION`.
    pub last_price: i64,
    /// Pyth's own EMA at the last accepted read, at `PRICE_PRECISION`.
    ///
    /// Observability and frontend display. The deviation breaker compares spot against the
    /// EMA carried in the *same* price message rather than against this stored copy — see
    /// `crate::oracle` for why a self-maintained reference wedges the market shut.
    pub ema_price: i64,
    pub last_price_update_ts: i64,
    pub total_fees_collected: u64,

    pub bump: u8,

    // --- appended in Phase 3 -----------------------------------------------------------
    // Added *after* `bump` and paid for out of `_reserved`, which shrank from 128 bytes to
    // 120. That is the § 5.6 forward-compatibility rule in action: every field above keeps
    // its byte offset, so an account written by the Phase 2 binary still deserialises. The
    // account's total size is unchanged.
    /// Minimum slots a position must be held before it may be closed (§ 6.6).
    ///
    /// Zero disables the check. Non-zero is strongly recommended on any market whose spread
    /// could ever be tighter than the true market spread — see `Position::opened_at_slot`.
    pub min_hold_slots: u64,
    /// Open positions across all users. Guards against delisting a market out from under
    /// live positions.
    pub open_position_count: u64,

    // --- appended in Phase 4 -----------------------------------------------------------
    // Taken from `_reserved`, which shrank from 120 bytes to 80. Existing offsets unchanged.
    /// Funding paid in by the heavy side but not yet claimed by the light side, in USDC.
    ///
    /// Funding is a transfer *between traders* and never touches LP capital (§ 6.7, I3), so
    /// it stays in the collateral vault the whole time — this is the part of that vault not
    /// yet attributed to any particular position. **Invariant I1 counts it.**
    pub funding_balance: u64,
    /// Sensitivity of the funding rate to book skew, at `RATE_PRECISION`.
    ///
    /// `funding_rate = clamp(skew_ratio × k, ±cap)`. Zero disables funding entirely, which
    /// is the correct setting for a market with no meaningful two-sided book yet.
    pub funding_rate_k: u64,
    /// Base-currency annual interest rate at `RATE_PRECISION` (4% = `40_000_000`).
    ///
    /// Stored alongside the quote rate and the markup rather than as one blended figure
    /// because **publishing the split is the product** (§ 6.7): a trader can read the true
    /// interest differential and the protocol's markup as separate line items instead of
    /// discovering a combined number after rollover. That is the one concrete improvement on
    /// XM/Exness opacity that costs nothing to build.
    pub rate_base_annual: i64,
    /// Quote-currency annual interest rate at `RATE_PRECISION`.
    pub rate_quote_annual: i64,
    /// When the session cranker last advanced this market's regime.
    pub last_session_crank_ts: i64,
    /// When `status` last changed.
    ///
    /// The `GapWindow` and `PreOpenWindow` timers need time-in-state, not time-since-crank.
    /// Deriving it from the crank timestamp would make the window's length depend on how
    /// often keepers happen to run, which is not a property a risk control should have.
    pub status_changed_at: i64,
    /// Bad debt the insurance fund could not cover, awaiting auto-deleveraging (§ 6.9).
    ///
    /// Non-zero means the pool is carrying a shortfall. `auto_deleverage` draws this down by
    /// withholding PnL from the most profitable opposing positions; until it reaches zero the
    /// market is in the state § 7.2 describes as insurance-exhausted.
    pub pending_adl_debt: u64,
    /// Confidence ceiling for the **liquidation** path, which must be wider than the
    /// trading one.
    ///
    /// # Why liquidation needs its own ceiling
    ///
    /// § 7.1 gates every price on `conf / price <= max_conf_bps`, and for *opening* a
    /// position that is exactly right (correction C-4): trading against a band you cannot
    /// price is how latency arbitrageurs get paid.
    ///
    /// Applying the same ceiling to liquidation inverts the logic. Confidence blows out
    /// during precisely the events that make positions liquidatable — the CHF depeg replay
    /// measured a band two orders of magnitude wider than normal — so a single ceiling means
    /// **liquidation stops working exactly when it is needed**, and positions keep falling
    /// with nobody able to close them. That is not caution; it is bad debt with extra steps.
    ///
    /// Opening into uncertainty is optional. Closing an insolvent position is not, and the
    /// alternative to liquidating at an uncertain price is not liquidating at all.
    ///
    /// Validated to be at least `max_conf_bps`. A market may set them equal, which restores
    /// the § 7.1 behaviour exactly.
    pub liquidation_max_conf_bps: u16,

    pub _reserved: [u8; 62],
}

impl Market {
    /// Symbol as `&str`, trailing NULs trimmed. Falls back to `""` on non-UTF-8 rather than
    /// erroring — this is used for logs and events, never for control flow.
    #[must_use]
    pub fn symbol_str(&self) -> &str {
        let end = self
            .symbol
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(MAX_SYMBOL_LEN);
        self.symbol
            .get(..end)
            .and_then(|s| core::str::from_utf8(s).ok())
            .unwrap_or("")
    }

    /// Confidence ceiling for the liquidation path. Never tightened by `WeekendMode`: a
    /// derated weekend is a reason to open less, not a reason to be unable to close.
    #[must_use]
    pub fn liquidation_conf_ceiling(&self) -> u16 {
        self.liquidation_max_conf_bps.max(self.max_conf_bps)
    }

    /// Confidence ceiling in force right now, tightened in `WeekendMode`.
    #[must_use]
    pub fn effective_max_conf_bps(&self) -> u16 {
        if self.status == MarketStatus::WeekendMode && self.weekend_max_conf_bps > 0 {
            self.weekend_max_conf_bps
        } else {
            self.max_conf_bps
        }
    }

    /// Leverage ceiling in force right now, halved in `WeekendMode`.
    #[must_use]
    pub fn effective_max_leverage(&self) -> u16 {
        if self.status == MarketStatus::WeekendMode && self.weekend_max_leverage > 0 {
            self.weekend_max_leverage
        } else {
            self.max_leverage
        }
    }

    /// Base spread in force right now, widened in `WeekendMode`.
    #[must_use]
    pub fn effective_base_spread_bps(&self) -> u16 {
        if self.status == MarketStatus::WeekendMode && self.weekend_spread_bps > 0 {
            self.weekend_spread_bps
        } else {
            self.base_spread_bps
        }
    }

    #[must_use]
    pub fn quote_conversion(&self) -> QuoteConversion {
        self.quote_conversion_kind.into()
    }

    #[must_use]
    pub fn is_synthetic(&self) -> bool {
        matches!(self.price_source, PriceSource::Synthetic { .. })
    }

    #[must_use]
    pub fn needs_quote_conversion(&self) -> bool {
        self.quote_conversion_kind != QuoteConversionKind::None
    }

    /// Validate every risk and oracle parameter. Called on creation and after any update,
    /// so a market can never be left in a configuration that was never checked.
    pub fn validate_risk_params(&self) -> Result<()> {
        require!(
            self.max_leverage > 0 && self.max_leverage <= MAX_ALLOWED_LEVERAGE,
            SolfxError::InvalidLeverage
        );

        // A position must not be liquidatable the moment it opens.
        require!(
            self.mmr_bps > 0 && self.mmr_bps < self.imr_bps,
            SolfxError::InvalidMarginRatios
        );

        // IMR and max_leverage are two statements of the same constraint and must agree.
        // If IMR were below 1/leverage, the leverage cap would be unreachable and the
        // effective ceiling would be whichever check ran first.
        let implied_min_imr = 10_000_u32
            .checked_div(u32::from(self.max_leverage))
            .ok_or(SolfxError::MathOverflow)?;
        require!(
            u32::from(self.imr_bps) >= implied_min_imr,
            SolfxError::InvalidMarginRatios
        );

        require!(
            self.liquidation_fee_bps > 0 && self.liquidation_fee_bps < self.mmr_bps,
            SolfxError::InvalidLiquidationFee
        );

        require!(
            self.max_staleness_seconds > 0
                && self.max_staleness_seconds <= MAX_ALLOWED_STALENESS_SECONDS,
            SolfxError::InvalidStaleness
        );
        require!(
            self.max_conf_bps > 0 && self.max_conf_bps <= MAX_ALLOWED_CONF_BPS,
            SolfxError::InvalidConfidenceLimit
        );
        // A liquidation ceiling *below* the trading one would mean a position could be opened
        // at a price it could never be liquidated at.
        require!(
            self.liquidation_max_conf_bps >= self.max_conf_bps,
            SolfxError::InvalidConfidenceLimit
        );
        require!(
            self.max_deviation_bps > 0,
            SolfxError::InvalidDeviationLimit
        );

        require!(
            self.min_position_size > 0 && self.min_position_size <= self.max_position_size,
            SolfxError::InvalidPositionSizeBounds
        );
        require!(
            self.max_oi_long > 0 && self.max_oi_short > 0,
            SolfxError::InvalidOiCap
        );

        Ok(())
    }

    // --- open interest (invariant I4, § 12.3) -------------------------------------------
    //
    // Two parallel sets of counters, and they answer different questions:
    //
    //   * `oi_long` / `oi_short` are in **quote units** and are what the OI caps in § 7.4
    //     are denominated in — a cap has to be a money figure to mean anything.
    //   * `base_oi_long` / `base_oi_short` are in **base units** and are what skew pricing
    //     (§ 6.6) and funding (§ 6.7) run on — an imbalance has to be size-denominated, or
    //     it would move every time the price did without any trade happening.
    //
    // Keeping only one would force the other to be derived at a price that changes between
    // the open and the close, and the derived figure would drift from reality.

    /// Record a position entering the book.
    pub fn add_open_interest(
        &mut self,
        direction: Direction,
        notional_quote: u64,
        size_base: u64,
    ) -> Result<()> {
        let size = i128::from(size_base);
        match direction {
            Direction::Long => {
                self.oi_long = self
                    .oi_long
                    .checked_add(notional_quote)
                    .ok_or(SolfxError::MathOverflow)?;
                self.base_oi_long = self
                    .base_oi_long
                    .checked_add(size)
                    .ok_or(SolfxError::MathOverflow)?;
            }
            Direction::Short => {
                self.oi_short = self
                    .oi_short
                    .checked_add(notional_quote)
                    .ok_or(SolfxError::MathOverflow)?;
                self.base_oi_short = self
                    .base_oi_short
                    .checked_add(size)
                    .ok_or(SolfxError::MathOverflow)?;
            }
        }
        Ok(())
    }

    /// Record a position leaving the book.
    ///
    /// Saturating rather than checked, and that is a deliberate choice with a cost.
    ///
    /// `oi_*` is a sum of notionals recorded at each *entry* price. A position opened at
    /// 1.0850 and partially closed contributes its share at that price, but rounding means
    /// the removals need not sum exactly back to the addition. Failing the close would trap
    /// a trader's collateral over a rounding unit; saturating leaks at most a few quote units
    /// into a cap that is measured in millions.
    ///
    /// The base-unit counters are checked, because those *must* balance exactly: they carry
    /// no price and drive funding, which has to net to zero across the market (I3).
    pub fn remove_open_interest(
        &mut self,
        direction: Direction,
        notional_quote: u64,
        size_base: u64,
    ) -> Result<()> {
        let size = i128::from(size_base);
        match direction {
            Direction::Long => {
                self.oi_long = self.oi_long.saturating_sub(notional_quote);
                self.base_oi_long = self
                    .base_oi_long
                    .checked_sub(size)
                    .ok_or(SolfxError::MathOverflow)?;
                require!(self.base_oi_long >= 0, SolfxError::OpenInterestUnderflow);
            }
            Direction::Short => {
                self.oi_short = self.oi_short.saturating_sub(notional_quote);
                self.base_oi_short = self
                    .base_oi_short
                    .checked_sub(size)
                    .ok_or(SolfxError::MathOverflow)?;
                require!(self.base_oi_short >= 0, SolfxError::OpenInterestUnderflow);
            }
        }
        Ok(())
    }

    /// Enforce the per-side OI cap (§ 7.4).
    ///
    /// Checked *after* the addition, so the cap is a ceiling on the resulting book rather
    /// than on the trade that got there.
    pub fn check_oi_cap(&self, direction: Direction) -> Result<()> {
        let (oi, cap) = match direction {
            Direction::Long => (self.oi_long, self.max_oi_long),
            Direction::Short => (self.oi_short, self.max_oi_short),
        };
        require!(oi <= cap, SolfxError::OpenInterestCapReached);
        Ok(())
    }

    /// The § 7.4 skew cap: block opens on the heavy side once the book is too one-sided
    /// **and** the imbalance is large enough to matter against the pool's capital.
    ///
    /// # Why funding is not enough on its own
    ///
    /// Funding is the *continuous* lever — it pays traders to correct skew, and at moderate
    /// imbalances that works. At extremes it is simply too slow: the vault's directional
    /// exposure grows faster than a per-hour rate can pull back, and § 7.4 says so directly.
    /// Past the threshold the heavy side closes to new opens entirely.
    ///
    /// # Why the ratio needs an absolute floor beside it
    ///
    /// § 7.4 specifies the ratio alone. That would make a market untradeable from empty — the
    /// first position is 100% one-sided by definition — and it asks the wrong question on a
    /// small book, where a lopsided ratio carries no real risk. See
    /// [`crate::constants::SKEW_CAP_MIN_IMBALANCE_BPS_OF_AUM`].
    ///
    /// # What is never blocked
    ///
    /// Trades that *reduce* skew. A cap that stopped the correcting trade would entrench the
    /// imbalance it exists to fix.
    ///
    /// Checked after the addition, like the OI cap: the limit describes the resulting book.
    pub fn check_skew_cap(&self, direction: Direction, max_skew_bps: u16, aum: u64) -> Result<()> {
        if max_skew_bps == 0 {
            return Ok(());
        }

        // Adding to the light side always reduces skew, whatever the numbers say.
        let light_side = if self.base_oi_long >= self.base_oi_short {
            Direction::Short
        } else {
            Direction::Long
        };
        if direction == light_side {
            return Ok(());
        }

        let total = self
            .base_oi_long
            .checked_add(self.base_oi_short)
            .ok_or(SolfxError::MathOverflow)?;
        if total <= 0 {
            return Ok(());
        }

        // Gate 1: is the imbalance material against the pool's capital?
        let imbalance_quote = u128::from(self.oi_long.abs_diff(self.oi_short));
        let floor = u128::from(aum)
            .checked_mul(SKEW_CAP_MIN_IMBALANCE_BPS_OF_AUM)
            .ok_or(SolfxError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(SolfxError::DivideByZero)?;
        if imbalance_quote <= floor {
            return Ok(());
        }

        // Gate 2: § 7.4's ratio.
        let diff = self
            .base_oi_long
            .checked_sub(self.base_oi_short)
            .ok_or(SolfxError::MathOverflow)?
            .checked_abs()
            .ok_or(SolfxError::MathOverflow)?;
        let skew_bps = diff
            .checked_mul(10_000)
            .ok_or(SolfxError::MathOverflow)?
            .checked_div(total)
            .ok_or(SolfxError::DivideByZero)?;

        require!(
            skew_bps <= i128::from(max_skew_bps),
            SolfxError::SkewCapReached
        );
        Ok(())
    }

    /// Validate the session calendar. Only meaningful for `SpotFx`, but the bounds are
    /// checked for every market so a later regime change cannot activate garbage.
    pub fn validate_session(&self) -> Result<()> {
        require!(
            self.session_open_dow <= 6 && self.session_close_dow <= 6,
            SolfxError::InvalidSession
        );
        require!(
            self.session_open_seconds < SECONDS_PER_DAY
                && self.session_close_seconds < SECONDS_PER_DAY,
            SolfxError::InvalidSession
        );
        Ok(())
    }
}
