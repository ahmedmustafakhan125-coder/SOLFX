//! Ranking the book by how close each position is to liquidation.
//!
//! # Why a sorted list rather than a scan-and-fire loop
//!
//! The naive keeper iterates positions and sends a transaction for each liquidatable one. It
//! works until the day it matters. In a real cascade — the CHF depeg of Phase 4's replay, or
//! any Sunday-open gap — hundreds of positions cross the line in the same second, and a
//! keeper that fires in account-map order is submitting transactions in an order uncorrelated
//! with how much money is at stake. Blockspace during that minute is scarce and expensive, so
//! the ones that land are effectively random.
//!
//! Ranking by *notional at risk* means the transactions that land first are the ones whose
//! failure would cost the pool most. The keeper still tries to get to all of them; it just
//! stops treating a $200 position and a $2,000,000 position as interchangeable when it can
//! only fit one in the block.
//!
//! The near-miss band exists for a different reason: an operator needs to see pressure
//! building, not only the liquidations that resulted. A run of positions sitting at 1.1×
//! maintenance for an hour is the signal that the next gap will be expensive, and it is
//! invisible if the keeper only logs what it fired.

use solana_pubkey::Pubkey;
use solfx_core::risk::Health;

use crate::book::Book;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub position: Pubkey,
    pub user_account: Pubkey,
    pub market_index: u16,
    pub health: Health,
    pub liquidatable: bool,
    /// `10_000` == 1.0. Presentation and ranking only — the decision is `liquidatable`,
    /// which comes from the program's own strict test.
    pub health_factor_bps: u64,
}

/// Everything at or below `watch_hf_bps`, worst first.
///
/// "Worst" is **notional among the liquidatable**, then health factor among the rest. Those
/// two orderings answer different questions and the list serves both: act on the first group,
/// watch the second.
pub fn assess(book: &Book, watch_hf_bps: u64, trace: bool) -> Vec<Candidate> {
    let mut out = Vec::new();

    for (key, position) in &book.positions {
        if position.size_base == 0 {
            continue;
        }
        let Some(market) = book.markets.get(&position.market_index) else {
            continue;
        };
        // A market whose price will not pass even the liquidation gate cannot be acted on.
        // That is not the keeper giving up: it is the state § 7.2 puts the market into on
        // purpose, and firing into it would mean realising a price nobody can verify.
        let Some(price) = book.price_for_liquidation(position.market_index) else {
            if trace {
                tracing::debug!(%key, index = position.market_index, "no usable price");
            }
            continue;
        };
        let Ok(health) = solfx_core::risk::assess(position, market, &price) else {
            tracing::warn!(%key, "health assessment failed");
            continue;
        };

        let liquidatable = health.is_liquidatable();
        let hf = health.health_factor_bps().unwrap_or(0);
        if trace {
            tracing::debug!(
                %key,
                equity = health.liquidation_equity,
                mm = health.maintenance_margin,
                hf_bps = hf,
                liquidatable,
                "evaluated"
            );
        }
        if !liquidatable && hf > watch_hf_bps {
            continue;
        }
        out.push(Candidate {
            position: *key,
            user_account: position.user_account,
            market_index: position.market_index,
            health_factor_bps: hf,
            liquidatable,
            health,
        });
    }

    out.sort_by(|a, b| {
        b.liquidatable
            .cmp(&a.liquidatable)
            .then(b.health.notional.cmp(&a.health.notional))
            .then(a.health_factor_bps.cmp(&b.health_factor_bps))
    });
    out
}
