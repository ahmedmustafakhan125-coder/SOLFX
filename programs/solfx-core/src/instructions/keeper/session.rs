//! Market regimes (`ARCHITECTURE.md` § 7.3, ADR-007).
//!
//! **This is where C-1 is stopped.** The interbank market closes Friday and reopens Sunday;
//! Pyth's spot feeds follow it. A trader who sees a weekend event that will gap EUR/USD 150
//! pips can open maximum size against the frozen Friday price with *zero risk*, and the LP
//! vault pays the entire gap. One hole, one drained vault.
//!
//! # Two signals, and trading restricts if **either** says closed
//!
//! 1. **The feed itself.** If it is not publishing within the market's staleness window, the
//!    market is closed. This handles holidays with no calendar to maintain, and it is the
//!    signal that cannot be wrong — a feed that has stopped has stopped.
//! 2. **The on-chain session calendar.** Per-market, in UTC, as a fallback when the oracle's
//!    status is ambiguous.
//!
//! Fail closed, always. An ambiguous signal halts.
//!
//! # Per-market calendars, not one global session
//!
//! EM pairs follow their **local** markets: Phase 0b measured USD/BRL trading 14–21 UTC,
//! USD/COP 14–18, USD/CLP 14–20, USD/PEN 14–19 — nothing like the interbank week. Assuming
//! one global session would leave those markets open against a dead feed, which is the C-1
//! exploit on exactly the pairs where a devaluation gap is most likely.

use anchor_lang::prelude::*;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;

use crate::constants::{
    GAP_WINDOW_SECONDS, MARKET_SEED, PRE_CLOSE_REDUCE_ONLY_SECONDS, PROTOCOL_SEED,
};
use crate::errors::SolfxError;
use crate::events::{MarketStatusChanged, SessionCranked};
use crate::oracle::observe_market_price;
use crate::state::{FeedKind, Market, MarketStatus, Protocol};

const SECONDS_PER_DAY: i64 = 86_400;
const SECONDS_PER_WEEK: i64 = 7 * SECONDS_PER_DAY;

#[derive(Accounts)]
pub struct CrankMarketSession<'info> {
    /// Permissionless — see `crank_market_price`. A market whose session only its operator
    /// can advance is a market that stays open when the operator is asleep.
    pub keeper: Signer<'info>,

    #[account(seeds = [PROTOCOL_SEED], bump = protocol.bump)]
    pub protocol: Box<Account<'info, Protocol>>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, Market>>,

    /// Optional. When supplied, the feed's freshness is the primary signal; without it the
    /// calendar alone decides.
    ///
    /// Optional rather than required because the crank must keep working when the feed has
    /// stopped entirely — which is precisely the case it exists to handle. A required
    /// account would make the market unclosable exactly when it most needs closing.
    pub price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub secondary_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
    pub quote_conversion_price_update: Option<Box<Account<'info, PriceUpdateV2>>>,
}

/// Advance a market's regime.
pub fn crank_market_session(ctx: Context<CrankMarketSession>) -> Result<()> {
    let clock = Clock::get()?;
    let market = &mut ctx.accounts.market;

    require!(
        !matches!(
            market.status,
            MarketStatus::Initialized | MarketStatus::Delisted
        ),
        SolfxError::MarketNotActive
    );

    // Signal 1: is the feed alive? A read that fails for *any* reason — stale, too wide,
    // wrong feed — counts as "not publishing". Fail closed.
    let feed_live = match ctx.accounts.price_update.as_deref() {
        Some(update) => observe_market_price(
            market,
            update,
            ctx.accounts.secondary_price_update.as_deref().map(|a| &**a),
            ctx.accounts
                .quote_conversion_price_update
                .as_deref()
                .map(|a| &**a),
            &clock,
        )
        .is_ok(),
        None => false,
    };

    // Signal 2: does the calendar say this market should be open?
    let calendar_open = session_is_open(market, clock.unix_timestamp);
    let seconds_to_close = seconds_until_close(market, clock.unix_timestamp);

    let old = market.status;
    let new = next_status(
        old,
        market.feed_kind,
        feed_live,
        calendar_open,
        seconds_to_close,
        clock.unix_timestamp,
        market.status_changed_at,
    );

    market.last_session_crank_ts = clock.unix_timestamp;

    if new != old {
        market.status = new;
        market.status_changed_at = clock.unix_timestamp;
        emit!(MarketStatusChanged {
            market_index: market.market_index,
            old_status: old as u8,
            new_status: new as u8,
            actor: 2, // automatic
            ts: clock.unix_timestamp,
        });
    }

    emit!(SessionCranked {
        market_index: market.market_index,
        status: new as u8,
        feed_live,
        calendar_open,
        seconds_to_close,
        ts: clock.unix_timestamp,
    });

    Ok(())
}

/// The transition table for both regimes.
///
/// Written as a pure function of observable signals so it can be exhaustively unit-tested
/// without an SVM — a state machine that can only be exercised through transactions is a
/// state machine whose corners never get tested.
#[allow(clippy::too_many_arguments)]
fn next_status(
    current: MarketStatus,
    feed_kind: FeedKind,
    feed_live: bool,
    calendar_open: bool,
    seconds_to_close: i64,
    now: i64,
    // `status_since` is when the market entered its current status. The GapWindow and
    // PreOpenWindow timers must measure time *in state*: deriving them from the crank
    // cadence would make a risk control's duration depend on keeper scheduling.
    status_since: i64,
) -> MarketStatus {
    // Manual overrides win. An admin or guardian that halted a market did so for a reason the
    // cranker cannot see, and a cranker that could undo it would make the halt advisory.
    if current == MarketStatus::Halted && !(feed_live && calendar_open) {
        return MarketStatus::Halted;
    }

    let open = feed_live && calendar_open;

    match feed_kind {
        // --- Regime A: session-bound (§ 7.3) ---
        //
        // Active -> ReduceOnly -> Halted -> GapWindow -> Active
        FeedKind::SpotFx => match current {
            MarketStatus::Active => {
                if !open {
                    MarketStatus::Halted
                } else if (0..=PRE_CLOSE_REDUCE_ONLY_SECONDS).contains(&seconds_to_close) {
                    // Stop taking new risk before the feed goes dark, so nothing is opened
                    // that cannot be managed until Monday.
                    MarketStatus::ReduceOnly
                } else {
                    MarketStatus::Active
                }
            }
            MarketStatus::ReduceOnly => {
                if !open {
                    MarketStatus::Halted
                } else if seconds_to_close > PRE_CLOSE_REDUCE_ONLY_SECONDS {
                    // The window passed without a close — the calendar rolled to the next
                    // session, so resume.
                    MarketStatus::Active
                } else {
                    MarketStatus::ReduceOnly
                }
            }
            MarketStatus::Halted => {
                if open {
                    // Reopening is not a return to normal. The first minutes after a session
                    // gap carry the whole weekend's news in one jump; closes and liquidations
                    // are allowed, new risk is not.
                    MarketStatus::GapWindow
                } else {
                    MarketStatus::Halted
                }
            }
            MarketStatus::GapWindow => {
                if !open {
                    MarketStatus::Halted
                } else if now.saturating_sub(status_since) >= GAP_WINDOW_SECONDS {
                    MarketStatus::Active
                } else {
                    MarketStatus::GapWindow
                }
            }
            other => other,
        },

        // --- Regime B: continuous ---
        //
        // `Crypto` never leaves `Active`: there is no underlying session to converge to, so
        // neither `WeekendMode` nor `PreOpenWindow` means anything.
        //
        // `ContinuousIndex` is **unreachable in production** — Phase 0b found no 24/7 metals
        // feed, and `initialize_market` rejects the variant. The transitions are written and
        // tested anyway, because the day Q1b resolves this becomes a config change rather
        // than a code change, and code that has never run is code that does not work.
        FeedKind::Crypto => {
            if feed_live {
                MarketStatus::Active
            } else {
                // Even a 24/7 feed can stop. 24/7 is a property of the feed, never a promise
                // the protocol makes on its behalf.
                MarketStatus::Halted
            }
        }
        FeedKind::ContinuousIndex => {
            if !feed_live {
                return MarketStatus::Halted;
            }
            match current {
                MarketStatus::Active if !calendar_open => MarketStatus::WeekendMode,
                MarketStatus::WeekendMode => {
                    if calendar_open {
                        MarketStatus::Active
                    } else if seconds_to_open_is_near(seconds_to_close) {
                        // The index-to-spot convergence gap lands here.
                        MarketStatus::PreOpenWindow
                    } else {
                        MarketStatus::WeekendMode
                    }
                }
                MarketStatus::PreOpenWindow => {
                    if calendar_open {
                        MarketStatus::Active
                    } else {
                        MarketStatus::PreOpenWindow
                    }
                }
                MarketStatus::Halted if calendar_open => MarketStatus::GapWindow,
                MarketStatus::GapWindow => MarketStatus::Active,
                other => other,
            }
        }
    }
}

fn seconds_to_open_is_near(seconds_to_close: i64) -> bool {
    (0..=crate::constants::PRE_OPEN_WINDOW_SECONDS).contains(&seconds_to_close)
}

/// Is `now` inside a weekly UTC window?
///
/// Takes the calendar as four primitives rather than a `&Market` so the whole thing can be
/// unit-tested without constructing an account — a calendar that can only be exercised
/// through a transaction is a calendar whose corners never get tested.
///
/// A window that wraps the week boundary — the interbank week opens Sunday and closes
/// Friday — is handled by inverting the comparison when the open point sits after the close.
pub fn calendar_is_open(
    open_dow: u8,
    open_seconds: u32,
    close_dow: u8,
    close_seconds: u32,
    now: i64,
) -> bool {
    let open = week_position(open_dow, open_seconds);
    let close = week_position(close_dow, close_seconds);

    // A degenerate calendar (open == close) means "always open" rather than "never open":
    // never-open would brick a market on a single configuration mistake.
    if open == close {
        return true;
    }

    let pos = seconds_into_week(now);
    if open < close {
        pos >= open && pos < close
    } else {
        pos >= open || pos < close
    }
}

/// Seconds until the window closes, or `-1` if it is not currently open.
pub fn calendar_seconds_until_close(
    open_dow: u8,
    open_seconds: u32,
    close_dow: u8,
    close_seconds: u32,
    now: i64,
) -> i64 {
    if !calendar_is_open(open_dow, open_seconds, close_dow, close_seconds, now) {
        return -1;
    }
    let close = week_position(close_dow, close_seconds);
    let pos = seconds_into_week(now);
    if close >= pos {
        close - pos
    } else {
        SECONDS_PER_WEEK - pos + close
    }
}

/// Is this market inside its session? Crypto has none and is always open.
pub fn session_is_open(market: &Market, now: i64) -> bool {
    if market.feed_kind == FeedKind::Crypto {
        return true;
    }
    calendar_is_open(
        market.session_open_dow,
        market.session_open_seconds,
        market.session_close_dow,
        market.session_close_seconds,
        now,
    )
}

/// Seconds until this market's session closes, or `-1` if it is not currently open.
pub fn seconds_until_close(market: &Market, now: i64) -> i64 {
    if market.feed_kind == FeedKind::Crypto {
        return -1;
    }
    calendar_seconds_until_close(
        market.session_open_dow,
        market.session_open_seconds,
        market.session_close_dow,
        market.session_close_seconds,
        now,
    )
}

/// Position within the UTC week, in seconds. Day 0 is Sunday, matching `session_*_dow`.
///
/// The Unix epoch (1 Jan 1970) was a **Thursday**, so the offset is four days.
fn seconds_into_week(unix_timestamp: i64) -> i64 {
    const THURSDAY_OFFSET: i64 = 4 * SECONDS_PER_DAY;
    let shifted = unix_timestamp.rem_euclid(SECONDS_PER_WEEK) + THURSDAY_OFFSET;
    shifted.rem_euclid(SECONDS_PER_WEEK)
}

fn week_position(dow: u8, seconds: u32) -> i64 {
    i64::from(dow) * SECONDS_PER_DAY + i64::from(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-01 is a Saturday — the day Phase 0b measured every metals feed frozen.
    /// 00:00:00 UTC that day is 1785542400.
    ///
    /// `the_week_starts_on_sunday` asserts this lands on day 6, so a wrong constant here
    /// fails loudly rather than silently shifting every boundary below. It already caught one.
    const SATURDAY_MIDNIGHT: i64 = 1_785_542_400;

    /// The interbank week: Sunday 21:00 UTC to Friday 21:00 UTC.
    const FX: (u8, u32, u8, u32) = (0, 75_600, 5, 75_600);

    fn fx_open(now: i64) -> bool {
        calendar_is_open(FX.0, FX.1, FX.2, FX.3, now)
    }
    fn fx_to_close(now: i64) -> i64 {
        calendar_seconds_until_close(FX.0, FX.1, FX.2, FX.3, now)
    }

    // --- the calendar ---

    #[test]
    fn the_week_starts_on_sunday() {
        // The Unix epoch was a Thursday, so day 4 of a Sunday-based week.
        assert_eq!(seconds_into_week(0), 4 * SECONDS_PER_DAY);
        assert_eq!(
            seconds_into_week(SATURDAY_MIDNIGHT) / SECONDS_PER_DAY,
            6,
            "2026-08-01 must land on Saturday, or every session boundary below is wrong"
        );
    }

    /// The measurement that ended the weekend product, as a calendar assertion.
    #[test]
    fn the_interbank_week_is_closed_all_saturday() {
        assert!(!fx_open(SATURDAY_MIDNIGHT));
        assert!(!fx_open(SATURDAY_MIDNIGHT + 12 * 3_600));
        assert!(!fx_open(SATURDAY_MIDNIGHT + 23 * 3_600));
    }

    #[test]
    fn the_interbank_week_opens_sunday_evening() {
        let sunday = SATURDAY_MIDNIGHT + SECONDS_PER_DAY;
        assert!(!fx_open(sunday + 20 * 3_600), "20:00 Sunday still shut");
        assert!(fx_open(sunday + 21 * 3_600), "21:00 Sunday open");
        assert!(fx_open(sunday + 23 * 3_600));
    }

    #[test]
    fn the_interbank_week_closes_friday_evening() {
        let friday = SATURDAY_MIDNIGHT - SECONDS_PER_DAY;
        assert!(fx_open(friday + 20 * 3_600), "20:00 Friday still open");
        assert!(!fx_open(friday + 21 * 3_600), "21:00 Friday shut");
    }

    #[test]
    fn midweek_is_open() {
        let sunday_night = SATURDAY_MIDNIGHT + SECONDS_PER_DAY + 22 * 3_600;
        for day in 0..4 {
            let t = sunday_night + SECONDS_PER_DAY * day;
            assert!(fx_open(t), "day +{day} should be open");
        }
    }

    /// Phase 0b measured USD/BRL trading 14:00–21:00 UTC — nothing like the interbank week.
    /// Assuming one global session would leave it open against a dead feed.
    #[test]
    fn an_em_market_follows_its_own_local_hours() {
        let monday = SATURDAY_MIDNIGHT + 2 * SECONDS_PER_DAY;
        let brl = |t| calendar_is_open(1, 14 * 3_600, 1, 21 * 3_600, t);
        assert!(!brl(monday + 13 * 3_600), "13:00 too early");
        assert!(brl(monday + 15 * 3_600), "15:00 open");
        assert!(!brl(monday + 22 * 3_600), "22:00 too late");
    }

    #[test]
    fn seconds_until_close_counts_down_within_the_session() {
        let friday = SATURDAY_MIDNIGHT - SECONDS_PER_DAY;
        assert_eq!(fx_to_close(friday + 19 * 3_600), 2 * 3_600);
        assert_eq!(fx_to_close(friday + 20 * 3_600 + 3_000), 600);
    }

    #[test]
    fn a_closed_market_reports_no_countdown() {
        assert_eq!(fx_to_close(SATURDAY_MIDNIGHT), -1);
    }

    /// A configuration mistake must not brick a market permanently.
    #[test]
    fn a_degenerate_calendar_is_always_open_not_never_open() {
        assert!(calendar_is_open(0, 0, 0, 0, SATURDAY_MIDNIGHT));
    }

    // --- the transition table ---

    fn step(
        current: MarketStatus,
        feed_live: bool,
        calendar_open: bool,
        to_close: i64,
    ) -> MarketStatus {
        next_status(
            current,
            FeedKind::SpotFx,
            feed_live,
            calendar_open,
            to_close,
            1_000,
            900,
        )
    }

    #[test]
    fn an_open_market_stays_active() {
        assert_eq!(
            step(MarketStatus::Active, true, true, 100_000),
            MarketStatus::Active
        );
    }

    #[test]
    fn approaching_the_close_goes_reduce_only() {
        assert_eq!(
            step(MarketStatus::Active, true, true, 600),
            MarketStatus::ReduceOnly,
            "within 15 minutes of the close, stop taking risk that cannot be managed \
             until the market reopens"
        );
    }

    /// **The C-1 test.** A dead feed halts the market whatever the calendar says.
    #[test]
    fn a_dead_feed_halts_the_market() {
        assert_eq!(
            step(MarketStatus::Active, false, true, 100_000),
            MarketStatus::Halted
        );
        assert_eq!(
            step(MarketStatus::ReduceOnly, false, true, 100),
            MarketStatus::Halted
        );
    }

    /// The other half of C-1: a live feed outside session hours still halts. The two signals
    /// are independent and either one closes the market.
    #[test]
    fn a_live_feed_outside_session_hours_still_halts() {
        assert_eq!(
            step(MarketStatus::Active, true, false, -1),
            MarketStatus::Halted
        );
    }

    #[test]
    fn reopening_goes_through_the_gap_window_not_straight_to_active() {
        assert_eq!(
            step(MarketStatus::Halted, true, true, 100_000),
            MarketStatus::GapWindow,
            "the first minutes after a session gap carry the weekend's news in one jump"
        );
    }

    #[test]
    fn the_gap_window_clears_once_its_time_is_up() {
        let entered = 10_000;
        let still_in = next_status(
            MarketStatus::GapWindow,
            FeedKind::SpotFx,
            true,
            true,
            100_000,
            entered + GAP_WINDOW_SECONDS - 1,
            entered,
        );
        assert_eq!(still_in, MarketStatus::GapWindow);

        let cleared = next_status(
            MarketStatus::GapWindow,
            FeedKind::SpotFx,
            true,
            true,
            100_000,
            entered + GAP_WINDOW_SECONDS,
            entered,
        );
        assert_eq!(cleared, MarketStatus::Active);
    }

    /// The window's length must not depend on how often keepers happen to run.
    #[test]
    fn the_gap_window_length_is_independent_of_crank_cadence() {
        let entered = 500_000;
        for now in [entered + 1, entered + 60, entered + GAP_WINDOW_SECONDS - 1] {
            assert_eq!(
                next_status(
                    MarketStatus::GapWindow,
                    FeedKind::SpotFx,
                    true,
                    true,
                    100_000,
                    now,
                    entered
                ),
                MarketStatus::GapWindow,
                "still inside the window at t+{}",
                now - entered
            );
        }
    }

    #[test]
    fn a_halted_market_does_not_reopen_while_either_signal_is_bad() {
        assert_eq!(
            step(MarketStatus::Halted, false, true, -1),
            MarketStatus::Halted
        );
        assert_eq!(
            step(MarketStatus::Halted, true, false, -1),
            MarketStatus::Halted
        );
        assert_eq!(
            step(MarketStatus::Halted, false, false, -1),
            MarketStatus::Halted
        );
    }

    #[test]
    fn crypto_never_leaves_active_while_its_feed_lives() {
        for status in [
            MarketStatus::Active,
            MarketStatus::GapWindow,
            MarketStatus::Halted,
        ] {
            assert_eq!(
                next_status(status, FeedKind::Crypto, true, true, -1, 1_000, 900),
                MarketStatus::Active
            );
        }
    }

    /// 24/7 is a property of the feed, never a promise the protocol makes on its behalf.
    #[test]
    fn even_crypto_halts_when_its_feed_stops() {
        assert_eq!(
            next_status(
                MarketStatus::Active,
                FeedKind::Crypto,
                false,
                true,
                -1,
                1_000,
                900
            ),
            MarketStatus::Halted
        );
    }

    /// Unreachable in production — Phase 0b found no continuous metals feed, and
    /// `initialize_market` rejects the variant. Written and tested anyway so that resolving
    /// Q1b is a configuration change rather than a code change: code that has never run is
    /// code that does not work.
    #[test]
    fn the_continuous_regime_derates_then_converges_then_reopens() {
        let cont = |current, calendar_open, to_close| {
            next_status(
                current,
                FeedKind::ContinuousIndex,
                true,
                calendar_open,
                to_close,
                1_000,
                900,
            )
        };

        assert_eq!(
            cont(MarketStatus::Active, false, -1),
            MarketStatus::WeekendMode,
            "spot closed -> derated weekend parameters, not a halt"
        );
        assert_eq!(
            cont(MarketStatus::WeekendMode, false, -1),
            MarketStatus::WeekendMode
        );
        assert_eq!(
            cont(MarketStatus::WeekendMode, false, 60),
            MarketStatus::PreOpenWindow,
            "no new opens while the index converges to spot"
        );
        assert_eq!(
            cont(MarketStatus::PreOpenWindow, true, 100_000),
            MarketStatus::Active
        );
    }

    #[test]
    fn a_continuous_market_with_a_stale_index_halts() {
        assert_eq!(
            next_status(
                MarketStatus::WeekendMode,
                FeedKind::ContinuousIndex,
                false,
                false,
                -1,
                1_000,
                900
            ),
            MarketStatus::Halted,
            "a stale index in WeekendMode is the C-1 exploit wearing a different hat"
        );
    }
}
