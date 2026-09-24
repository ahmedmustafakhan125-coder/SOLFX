//! The NOXFUNDS crank.
//!
//! Every instruction here is permissionless on chain and was, until this module, run by
//! nobody. A mandate was marked only when someone marked it, so a trader could sit past the
//! drawdown limit indefinitely with the breach undetected; a stop that fired on SolFX never
//! reached the trader's record until someone reconciled it; and an evaluation's simulated stop
//! fired only if somebody fired it.
//!
//! # What it does, per pass
//!
//! - **Funded mandates.** A position that closed where NOXFUNDS could not see it (a stop, a
//!   take-profit, a liquidation) is reconciled first, because until it is the program refuses
//!   to observe the mandate at all. Then an `Active` mandate holding positions is marked. A
//!   `Breached` or `WindingDown` mandate has its positions closed and its stops cancelled once
//!   their positions are gone.
//! - **Evaluations.** A simulated stop whose price has been reached is fired, and an evaluation
//!   holding simulated positions is marked.
//! - **Tiers.** A profile whose record earns a different tier from the one it shows is
//!   recomputed.
//!
//! # What it deliberately does not do
//!
//! **Settle.** `claim_settlement` is permissionless too, and one click in the browser for
//! anyone. Running it from here would pay three parties at a moment none of them chose, and it
//! fails outright if any of their token accounts is missing — a crank that errors every pass on
//! a mandate it cannot finish is noise, not help.
//!
//! # The property it preserves
//!
//! Every decision uses the programs' own functions, compiled from the same source:
//! `load_validated_price` to price a leg, `TriggerKind::is_met` to judge a stop,
//! `TraderTier::for_stats` to judge a tier. And every decision is re-checked on chain, so a
//! stale view costs a rejected simulation, never a wrong outcome.

use std::collections::HashMap;
use std::sync::Arc;

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anyhow::Result;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use noxfunds::constants::{CONFIG_SEED, MANDATE_SIGNER_SEED, MANDATE_VAULT_SEED, TRADER_SEED};
use noxfunds::state::{
    Evaluation, EvaluationState, Mandate, MandateState, PositionSlot, TraderProfile, TraderTier,
    VirtualPosition,
};
use solfx_core::constants::POSITION_SEED;
use solfx_core::state::{Direction, Position, PriceSource, TriggerKind, UserAccount};

use crate::book::{market_pda, Book};
use crate::services::Shared;

/// A pass is capped like the other keepers' passes, for the same reason: an unbounded burst
/// from one signer is rate-limited and dropped, and lands less than a paced one would.
const MAX_ACTIONS_PER_PASS: usize = 8;

/// A mandate is marked when its estimated equity has moved this far from the last mark, in bps of
/// its peak — so the recorded peak, drawdown and daily loss are each within this of the truth at
/// the crank's resolution, and a breach is caught within this of the limit.
pub const MOVE_BPS: u64 = 25;

/// And at least this often while anything is open, whatever the estimate says. Evaluations are
/// marked on this alone: their pricing (`realise`) is private to the program, so there is no
/// estimate to compare, and a simulated stop — fired as soon as it is met — is what bounds an
/// evaluation position's loss between marks.
pub const HEARTBEAT_SECS: i64 = 300;

const TOKEN_PROGRAM: Pubkey = solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

// --- addresses -------------------------------------------------------------------------------

fn nox_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &noxfunds::ID).0
}

#[must_use]
pub fn position_pda(user_account: &Pubkey, market_index: u16, nonce: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            POSITION_SEED,
            user_account.as_ref(),
            &market_index.to_le_bytes(),
            &[nonce],
        ],
        &solfx_core::ID,
    )
    .0
}

// --- deciding --------------------------------------------------------------------------------

/// What the book can say about one market right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Priced {
    /// The market account, then its price legs in the order the program reads them: primary,
    /// then the secondary leg if the market is synthetic, then the conversion leg if it is not
    /// quoted in USD.
    pub legs: Vec<Pubkey>,
    /// The oracle price as `load_validated_price` normalises it.
    pub spot: i64,
    /// Whether the market's status allows a close. Marking needs only a valid price; closing
    /// and firing a stop need the market open as well.
    pub closable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MandateStep {
    /// Every open slot's position address, in slot order — the program demands exactly that.
    Reconcile {
        slots: Vec<Pubkey>,
    },
    /// The remaining accounts for `observe_mandate_equity`: one group per open position.
    Observe {
        groups: Vec<Pubkey>,
    },
    WindDown {
        position: Pubkey,
        market_index: u16,
    },
    CancelStop {
        trigger: Pubkey,
        position: Pubkey,
    },
}

/// What to do for one mandate. Pure, so the decisions are testable without a cluster.
///
/// `is_live` answers whether a SolFX position account exists; `priced` whether a market can be
/// priced (and closed) now. `triggers` is every SolFX trigger order owned by this mandate's
/// SolFX account, as (order, the position it protects).
pub fn plan_mandate(
    state: MandateState,
    user_account: &Pubkey,
    slots: &[PositionSlot],
    is_live: impl Fn(&Pubkey) -> bool,
    priced: impl Fn(u16) -> Option<Priced>,
    triggers: &[(Pubkey, Pubkey)],
) -> Vec<MandateStep> {
    if state == MandateState::Settled {
        return Vec::new();
    }
    let open: Vec<(&PositionSlot, Pubkey)> = slots
        .iter()
        .filter(|s| s.open)
        .map(|s| (s, position_pda(user_account, s.market_index, s.nonce)))
        .collect();

    let mut steps = Vec::new();
    let stopped = matches!(state, MandateState::Breached | MandateState::WindingDown);

    // A stopped mandate's stops are returned once their positions are gone — independent of
    // the slots, so it can happen in the same pass as a reconcile.
    if stopped {
        steps.extend(
            triggers
                .iter()
                .filter(|(_, position)| !is_live(position))
                .map(|(trigger, position)| MandateStep::CancelStop {
                    trigger: *trigger,
                    position: *position,
                }),
        );
    }

    // Reconcile before anything that reads the slots. Until it runs the program refuses to
    // observe the mandate (a slot's position cannot be supplied) and refuses to book a new
    // open at the stale slot's address.
    if open.iter().any(|(_, p)| !is_live(p)) {
        steps.push(MandateStep::Reconcile {
            slots: open.iter().map(|(_, p)| *p).collect(),
        });
        return steps;
    }
    if open.is_empty() {
        return steps;
    }

    if stopped {
        steps.extend(open.iter().filter_map(|(s, p)| {
            priced(s.market_index)
                .filter(|m| m.closable)
                .map(|_| MandateStep::WindDown {
                    position: *p,
                    market_index: s.market_index,
                })
        }));
        return steps;
    }

    // `Active` with positions: mark it, but only with every leg priceable. A partial group is
    // refused on chain (`IncompleteObservation`), so sending one would only fail.
    let mut groups = Vec::new();
    for (s, p) in &open {
        let Some(m) = priced(s.market_index) else {
            return steps;
        };
        groups.push(*p);
        groups.extend(m.legs);
    }
    steps.push(MandateStep::Observe { groups });
    steps
}

/// Whether a mark is worth a transaction.
///
/// Marking every pass cost a transaction every 30 seconds per open mandate — 2,880 a day — and
/// buried the few events a trader's record is made of under thousands of identical ones. Marking
/// on movement keeps the enforcement (a breach is a movement) and drops the repetition.
#[must_use]
pub fn observe_due(
    estimate: u64,
    last_equity: u64,
    peak_equity: u64,
    last_observed_at: i64,
    now: i64,
) -> bool {
    if now.saturating_sub(last_observed_at) >= HEARTBEAT_SECS {
        return true;
    }
    // moved / peak >= MOVE_BPS / BPS, compared by cross-multiplying so nothing is divided.
    let moved = u128::from(estimate.abs_diff(last_equity));
    let base = u128::from(peak_equity.max(1));
    match (
        moved.checked_mul(u128::from(noxfunds::constants::BPS)),
        base.checked_mul(u128::from(MOVE_BPS)),
    ) {
        (Some(lhs), Some(rhs)) => lhs >= rhs,
        // Unrepresentable means enormous; mark it and let the chain judge.
        _ => true,
    }
}

/// The mandate's equity as `observe_mandate_equity` will compute it: vault, plus free collateral,
/// plus each position marked by `solfx_core::risk::assess` — the same function, on the same
/// price. `None` when any input is missing, and the caller then marks rather than guesses.
fn estimate_equity(book: &Book, free: u64, vault: u64, positions: &[&Position]) -> Option<u64> {
    let mut equity = i128::from(free).checked_add(i128::from(vault))?;
    for p in positions {
        let market = book.markets.get(&p.market_index)?;
        let price = book.price_for_trading(p.market_index)?;
        let health = solfx_core::risk::assess(p, market, &price).ok()?;
        equity = equity.checked_add(i128::from(health.equity))?;
    }
    u64::try_from(equity.max(0)).ok()
}

/// One simulated position, as much of it as the decision needs.
#[derive(Debug, Clone, Copy)]
pub struct SimPosition {
    pub key: Pubkey,
    pub market_index: u16,
    pub direction: Direction,
    pub stop_price: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalStep {
    TriggerStop {
        position: Pubkey,
        market_index: u16,
    },
    /// (virtual position, market, price update) triples, each open position exactly once.
    Observe {
        triples: Vec<Pubkey>,
    },
}

/// What to do for one active evaluation.
///
/// Stops first, and nothing else in the same pass: firing one changes the set of open
/// positions, and a mark built from the old set would be refused as incomplete.
pub fn plan_evaluation(
    open_positions: u8,
    positions: &[SimPosition],
    priced: impl Fn(u16) -> Option<Priced>,
) -> Vec<EvalStep> {
    let stops: Vec<EvalStep> = positions
        .iter()
        .filter(|p| {
            priced(p.market_index).is_some_and(|m| {
                m.closable && TriggerKind::StopLoss.is_met(p.direction, p.stop_price, m.spot)
            })
        })
        .map(|p| EvalStep::TriggerStop {
            position: p.key,
            market_index: p.market_index,
        })
        .collect();
    if !stops.is_empty() {
        return stops;
    }

    // The account's count and the positions found must agree, or the view is mid-change.
    if open_positions == 0 || positions.len() != usize::from(open_positions) {
        return Vec::new();
    }
    let mut triples = Vec::new();
    for p in positions {
        // Evaluations price one leg only (`load_validated_price(.., None, None, ..)`), so the
        // market and its primary account are the whole of what the program reads.
        let Some(m) = priced(p.market_index) else {
            return Vec::new();
        };
        let mut legs = m.legs.into_iter();
        let (Some(market), Some(primary)) = (legs.next(), legs.next()) else {
            return Vec::new();
        };
        triples.extend([p.key, market, primary]);
    }
    vec![EvalStep::Observe { triples }]
}

/// Price a market from the book, through the **trading** gate — the one every NOXFUNDS
/// instruction here is judged against on chain.
pub fn priced_from_book(book: &Book, market_index: u16) -> Option<Priced> {
    let market = book.markets.get(&market_index)?;
    let feeds = book.feeds.get(&market_index)?;
    let price = book.price_for_trading(market_index)?;
    let mut legs = vec![market_pda(market_index), feeds.primary];
    // The program decides how many legs follow from the market, not from which feed ids happen
    // to be set, so this asks the same two questions it does.
    if matches!(market.price_source, PriceSource::Synthetic { .. }) {
        legs.push(feeds.secondary?);
    }
    if market.needs_quote_conversion() {
        legs.push(feeds.quote_conversion?);
    }
    Some(Priced {
        legs,
        spot: price.spot.price,
        closable: market.status.allows_close(),
    })
}

// --- the loop --------------------------------------------------------------------------------

pub async fn run_nox(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(shared.cfg.nox_interval());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if let Err(e) = nox_pass(&shared).await {
            tracing::error!(error = %format!("{e:#}"), "noxfunds pass failed");
        }
    }
}

struct Action {
    ix: Instruction,
    label: &'static str,
    subject: Pubkey,
}

/// What one pass read directly from the chain, by address.
#[derive(Default)]
struct Reads {
    positions: HashMap<Pubkey, Position>,
    /// SolFX account → its free collateral.
    free: HashMap<Pubkey, u64>,
    /// Mandate vault → its balance.
    vaults: HashMap<Pubkey, u64>,
}

/// An SPL token account's balance: bytes 64..72, after the mint and the owner. Anchor accounts
/// are tried first, so this only sees what is left — the vault.
fn token_amount(data: &[u8]) -> Option<u64> {
    if data.len() != 165 {
        return None;
    }
    let bytes: [u8; 8] = data.get(64..72)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

async fn nox_pass(shared: &Arc<Shared>) -> Result<()> {
    let chain = &shared.chain;
    let mandates: Vec<(Pubkey, Mandate)> = chain.all(&noxfunds::ID).await?;
    let mandates: Vec<(Pubkey, Mandate)> = mandates
        .into_iter()
        .filter(|(_, m)| m.state != MandateState::Settled)
        .collect();
    let evaluations: Vec<(Pubkey, Evaluation)> = chain.all(&noxfunds::ID).await?;
    let evaluations: Vec<(Pubkey, Evaluation)> = evaluations
        .into_iter()
        .filter(|(_, e)| e.state == EvaluationState::Active)
        .collect();
    let virtual_positions: Vec<(Pubkey, VirtualPosition)> =
        if evaluations.iter().any(|(_, e)| e.open_positions > 0) {
            chain.all(&noxfunds::ID).await?
        } else {
            Vec::new()
        };
    let profiles: Vec<(Pubkey, TraderProfile)> = chain.all(&noxfunds::ID).await?;

    // Read straight from the chain rather than from the book, which is up to a refresh old: a
    // reconcile decided on a position the book has not yet seen would only fail, and a pass of
    // those every half-minute is noise that hides the failures that matter. The SolFX account
    // and the vault come in the same call, for the equity estimate.
    let mut wanted: Vec<Pubkey> = Vec::new();
    for (key, m) in &mandates {
        if !m.slots.iter().any(|s| s.open) {
            continue;
        }
        wanted.push(m.solfx_user_account);
        wanted.push(nox_pda(&[MANDATE_VAULT_SEED, key.as_ref()]));
        wanted.extend(
            m.slots
                .iter()
                .filter(|s| s.open)
                .map(|s| position_pda(&m.solfx_user_account, s.market_index, s.nonce)),
        );
    }
    let mut read = Reads::default();
    if !wanted.is_empty() {
        for (data, key) in chain.multiple(&wanted).await?.into_iter().zip(wanted) {
            let Some(data) = data else { continue };
            if let Ok(p) = Position::try_deserialize(&mut data.as_slice()) {
                read.positions.insert(key, p);
            } else if let Ok(u) = UserAccount::try_deserialize(&mut data.as_slice()) {
                read.free.insert(key, u.free_collateral);
            } else if let Some(amount) = token_amount(&data) {
                read.vaults.insert(key, amount);
            }
        }
    }

    let actions = {
        let book = shared.book.read().await;
        if !crate::services::book_is_fresh(shared, &book) {
            return Ok(());
        }
        let mut actions = Vec::new();
        for (key, m) in &mandates {
            mandate_actions(shared, &book, &read, *key, m, &mut actions);
        }
        let mut by_eval: HashMap<Pubkey, Vec<SimPosition>> = HashMap::new();
        for (key, p) in &virtual_positions {
            by_eval.entry(p.evaluation).or_default().push(SimPosition {
                key: *key,
                market_index: p.market_index,
                direction: p.direction,
                stop_price: p.stop_price,
            });
        }
        for (key, e) in &evaluations {
            let positions = by_eval.remove(key).unwrap_or_default();
            evaluation_actions(shared, &book, *key, e, &positions, &mut actions);
        }
        for (key, p) in &profiles {
            if TraderTier::for_stats(p) != p.tier {
                actions.push(Action {
                    ix: Instruction {
                        program_id: noxfunds::ID,
                        accounts: noxfunds::accounts::RecomputeTier {
                            caller: shared.chain.pubkey(),
                            profile: *key,
                        }
                        .to_account_metas(None),
                        data: noxfunds::instruction::RecomputeTier {}.data(),
                    },
                    label: "recompute_tier",
                    subject: *key,
                });
            }
        }
        actions
    };

    tracing::debug!(
        mandates = mandates.len(),
        evaluations = evaluations.len(),
        actions = actions.len(),
        "noxfunds pass"
    );
    for a in actions.into_iter().take(MAX_ACTIONS_PER_PASS) {
        match shared.chain.send(a.ix, a.label).await {
            Ok(Some(sig)) => {
                tracing::info!(%sig, subject = %a.subject, action = a.label, "noxfunds")
            }
            Ok(None) => {}
            Err(e) if is_benign(&e) => {
                tracing::debug!(subject = %a.subject, action = a.label, "nothing to do, or lost a race");
            }
            Err(e) => tracing::warn!(
                subject = %a.subject,
                action = a.label,
                error = %format!("{e:#}"),
                "noxfunds action failed"
            ),
        }
    }
    Ok(())
}

fn mandate_actions(
    shared: &Shared,
    book: &Book,
    read: &Reads,
    key: Pubkey,
    m: &Mandate,
    out: &mut Vec<Action>,
) {
    let triggers: Vec<(Pubkey, Pubkey)> = book
        .triggers
        .iter()
        .filter(|(_, t)| t.user_account == m.solfx_user_account)
        .map(|(k, t)| (*k, t.position))
        .collect();
    // A trigger's position outside the open slots is gone unless the book still holds it —
    // and if it does, the chain refuses the cancel anyway (`StopProtectsOpenPosition`).
    let is_live = |p: &Pubkey| read.positions.contains_key(p) || book.positions.contains_key(p);
    let steps = plan_mandate(
        m.state,
        &m.solfx_user_account,
        &m.slots,
        is_live,
        |i| priced_from_book(book, i),
        &triggers,
    );

    let me = shared.chain.pubkey();
    let config = nox_pda(&[CONFIG_SEED]);
    let signer = nox_pda(&[MANDATE_SIGNER_SEED, key.as_ref()]);
    let profile = nox_pda(&[TRADER_SEED, m.trader.as_ref()]);

    for step in steps {
        let (accounts, data, label): (Vec<AccountMeta>, Vec<u8>, &'static str) = match step {
            MandateStep::Reconcile { slots } => {
                let mut metas = noxfunds::accounts::ReconcilePosition {
                    caller: me,
                    mandate: key,
                    user_account: m.solfx_user_account,
                    trader_profile: profile,
                }
                .to_account_metas(None);
                metas.extend(
                    slots
                        .into_iter()
                        .map(|p| AccountMeta::new_readonly(p, false)),
                );
                (
                    metas,
                    noxfunds::instruction::ReconcilePosition {}.data(),
                    "reconcile_position",
                )
            }
            MandateStep::Observe { groups } => {
                let positions: Vec<&Position> = m
                    .slots
                    .iter()
                    .filter(|s| s.open)
                    .filter_map(|s| {
                        read.positions.get(&position_pda(
                            &m.solfx_user_account,
                            s.market_index,
                            s.nonce,
                        ))
                    })
                    .collect();
                let estimate = read
                    .free
                    .get(&m.solfx_user_account)
                    .zip(
                        read.vaults
                            .get(&nox_pda(&[MANDATE_VAULT_SEED, key.as_ref()])),
                    )
                    .and_then(|(free, vault)| estimate_equity(book, *free, *vault, &positions));
                if let Some(e) = estimate {
                    if !observe_due(
                        e,
                        m.last_equity,
                        m.peak_equity,
                        m.last_observed_at,
                        book.clock.unix_timestamp,
                    ) {
                        continue;
                    }
                }
                let mut metas = noxfunds::accounts::ObserveMandateEquity {
                    observer: me,
                    mandate: key,
                    user_account: m.solfx_user_account,
                    trader_profile: profile,
                    mandate_vault: nox_pda(&[MANDATE_VAULT_SEED, key.as_ref()]),
                }
                .to_account_metas(None);
                metas.extend(
                    groups
                        .into_iter()
                        .map(|p| AccountMeta::new_readonly(p, false)),
                );
                (
                    metas,
                    noxfunds::instruction::ObserveMandateEquity {}.data(),
                    "observe_mandate_equity",
                )
            }
            MandateStep::WindDown {
                position,
                market_index,
            } => {
                let Some(feeds) = book.feeds.get(&market_index) else {
                    continue;
                };
                let v = shared.vaults;
                (
                    noxfunds::accounts::WindDownPosition {
                        closer: me,
                        config,
                        mandate: key,
                        mandate_signer: signer,
                        protocol: v.protocol,
                        user_account: m.solfx_user_account,
                        market: market_pda(market_index),
                        position,
                        trader_profile: profile,
                        collateral_vault: v.collateral_vault,
                        lp_pool: v.lp_pool,
                        lp_vault: v.lp_vault,
                        insurance_fund: v.insurance_fund,
                        insurance_vault: v.insurance_vault,
                        fee_vault: v.fee_vault,
                        price_update: feeds.primary,
                        secondary_price_update: feeds.secondary,
                        quote_conversion_price_update: feeds.quote_conversion,
                        token_program: TOKEN_PROGRAM,
                        solfx_core_program: solfx_core::ID,
                    }
                    .to_account_metas(None),
                    noxfunds::instruction::WindDownPosition {}.data(),
                    "wind_down_position",
                )
            }
            MandateStep::CancelStop { trigger, position } => (
                noxfunds::accounts::WindDownCancelStop {
                    closer: me,
                    config,
                    mandate: key,
                    mandate_signer: signer,
                    trigger_order: trigger,
                    position,
                    solfx_core_program: solfx_core::ID,
                }
                .to_account_metas(None),
                noxfunds::instruction::WindDownCancelStop {}.data(),
                "wind_down_cancel_stop",
            ),
        };
        out.push(Action {
            ix: Instruction {
                program_id: noxfunds::ID,
                accounts,
                data,
            },
            label,
            subject: key,
        });
    }
}

fn evaluation_actions(
    shared: &Shared,
    book: &Book,
    key: Pubkey,
    e: &Evaluation,
    positions: &[SimPosition],
    out: &mut Vec<Action>,
) {
    let me = shared.chain.pubkey();
    for step in plan_evaluation(e.open_positions, positions, |i| priced_from_book(book, i)) {
        let (accounts, data, label): (Vec<AccountMeta>, Vec<u8>, &'static str) = match step {
            EvalStep::TriggerStop {
                position,
                market_index,
            } => {
                let Some(feeds) = book.feeds.get(&market_index) else {
                    continue;
                };
                (
                    noxfunds::accounts::EvalTriggerStop {
                        keeper: me,
                        trader: e.trader,
                        evaluation: key,
                        virtual_position: position,
                        market: market_pda(market_index),
                        price_update: feeds.primary,
                    }
                    .to_account_metas(None),
                    noxfunds::instruction::EvalTriggerStop {}.data(),
                    "eval_trigger_stop",
                )
            }
            EvalStep::Observe { triples } => {
                if book.clock.unix_timestamp.saturating_sub(e.last_observed_at) < HEARTBEAT_SECS {
                    continue;
                }
                let mut metas = noxfunds::accounts::EvalObserveEquity {
                    observer: me,
                    evaluation: key,
                }
                .to_account_metas(None);
                metas.extend(
                    triples
                        .into_iter()
                        .map(|p| AccountMeta::new_readonly(p, false)),
                );
                (
                    metas,
                    noxfunds::instruction::EvalObserveEquity {}.data(),
                    "eval_observe_equity",
                )
            }
        };
        out.push(Action {
            ix: Instruction {
                program_id: noxfunds::ID,
                accounts,
                data,
            },
            label,
            subject: key,
        });
    }
}

/// Failures that mean "nothing to do" or "someone got there first", not a fault.
///
/// Its own list rather than an extension of [`crate::chain::is_benign_race`]: those names are
/// SolFX's, these are NOXFUNDS', and widening the other list would hide a SolFX failure that
/// happens to share a word.
fn is_benign(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}");
    [
        // The view changed between the read and the send: a position opened or closed.
        "IncompleteObservation",
        "PositionStillOpen",
        "StopNotTriggered",
        "StopProtectsOpenPosition",
        "MandateNotWindingDown",
        // Someone else closed or cancelled it first.
        "AccountNotInitialized",
        "AccountOwnedByWrongProgram",
        "already been processed",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

#[cfg(test)]
// A panic is the test's reporting mechanism; the workspace denies it elsewhere.
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    const USER: Pubkey = Pubkey::new_from_array([7; 32]);

    fn slot(market_index: u16, nonce: u8) -> PositionSlot {
        PositionSlot {
            open: true,
            market_index,
            nonce,
            notional: 1_000_000_000,
        }
    }

    fn priced_ok(_: u16) -> Option<Priced> {
        Some(Priced {
            legs: vec![
                Pubkey::new_from_array([1; 32]),
                Pubkey::new_from_array([2; 32]),
            ],
            spot: 100_000_000_000,
            closable: true,
        })
    }

    fn all_live(_: &Pubkey) -> bool {
        true
    }

    #[test]
    fn a_flat_active_mandate_is_left_alone() {
        let steps = plan_mandate(MandateState::Active, &USER, &[], all_live, priced_ok, &[]);
        assert!(steps.is_empty());
    }

    #[test]
    fn an_active_mandate_with_positions_is_marked_with_every_group() {
        let slots = [slot(3, 0), PositionSlot::default(), slot(8, 1)];
        let steps = plan_mandate(
            MandateState::Active,
            &USER,
            &slots,
            all_live,
            priced_ok,
            &[],
        );
        let [MandateStep::Observe { groups }] = steps.as_slice() else {
            panic!("expected one observe, got {steps:?}");
        };
        // Two positions, each followed by its market and its one price leg.
        assert_eq!(groups.len(), 6);
        assert_eq!(groups[0], position_pda(&USER, 3, 0));
        assert_eq!(groups[3], position_pda(&USER, 8, 1));
    }

    /// The program refuses a partial group, so an unpriceable leg means no mark at all rather
    /// than a mark that is certain to fail.
    #[test]
    fn a_position_that_cannot_be_priced_blocks_the_mark() {
        let slots = [slot(3, 0), slot(8, 1)];
        let steps = plan_mandate(
            MandateState::Active,
            &USER,
            &slots,
            all_live,
            |i| if i == 8 { None } else { priced_ok(i) },
            &[],
        );
        assert!(steps.is_empty());
    }

    /// A stop that fired on SolFX leaves the slot open in NOXFUNDS; until it is reconciled the
    /// mandate cannot be marked, so reconciling comes first and carries every open slot.
    #[test]
    fn a_position_gone_from_solfx_is_reconciled_before_anything_else() {
        let slots = [slot(3, 0), slot(8, 1)];
        let gone = position_pda(&USER, 8, 1);
        let steps = plan_mandate(
            MandateState::Active,
            &USER,
            &slots,
            |p| *p != gone,
            priced_ok,
            &[],
        );
        assert_eq!(
            steps,
            vec![MandateStep::Reconcile {
                slots: vec![position_pda(&USER, 3, 0), gone]
            }]
        );
    }

    #[test]
    fn a_breached_mandate_has_its_positions_closed() {
        let slots = [slot(3, 0)];
        let steps = plan_mandate(
            MandateState::Breached,
            &USER,
            &slots,
            all_live,
            priced_ok,
            &[],
        );
        assert_eq!(
            steps,
            vec![MandateStep::WindDown {
                position: position_pda(&USER, 3, 0),
                market_index: 3
            }]
        );
    }

    #[test]
    fn a_closed_market_is_not_wound_down_into() {
        let slots = [slot(3, 0)];
        let steps = plan_mandate(
            MandateState::WindingDown,
            &USER,
            &slots,
            all_live,
            |i| {
                priced_ok(i).map(|p| Priced {
                    closable: false,
                    ..p
                })
            },
            &[],
        );
        assert!(steps.is_empty());
    }

    /// Rule R-4: a stop may be cancelled only once its position is gone.
    #[test]
    fn a_stopped_mandates_stops_are_cancelled_only_once_their_positions_are_gone() {
        let trigger_live = (Pubkey::new_from_array([9; 32]), position_pda(&USER, 3, 0));
        let orphan_pos = Pubkey::new_from_array([5; 32]);
        let trigger_orphan = (Pubkey::new_from_array([10; 32]), orphan_pos);
        let steps = plan_mandate(
            MandateState::WindingDown,
            &USER,
            &[],
            |p| *p != orphan_pos,
            priced_ok,
            &[trigger_live, trigger_orphan],
        );
        assert_eq!(
            steps,
            vec![MandateStep::CancelStop {
                trigger: trigger_orphan.0,
                position: orphan_pos
            }]
        );
    }

    /// Only the trader cancels an active mandate's orders; the crank has no business there.
    #[test]
    fn an_active_mandates_orders_are_never_touched() {
        let orphan = (
            Pubkey::new_from_array([10; 32]),
            Pubkey::new_from_array([5; 32]),
        );
        let steps = plan_mandate(
            MandateState::Active,
            &USER,
            &[],
            |_| false,
            priced_ok,
            &[orphan],
        );
        assert!(steps.is_empty());
    }

    #[test]
    fn a_settled_mandate_is_never_touched() {
        let steps = plan_mandate(
            MandateState::Settled,
            &USER,
            &[slot(3, 0)],
            |_| false,
            priced_ok,
            &[],
        );
        assert!(steps.is_empty());
    }

    const PEAK: u64 = 500_000_000; // $500

    #[test]
    fn an_unmoved_mandate_is_not_marked_again() {
        assert!(!observe_due(PEAK, PEAK, PEAK, 1_000, 1_030));
    }

    #[test]
    fn a_move_of_the_threshold_either_way_is_marked() {
        let step = 1_250_000; // 25 bps of $500 = $1.25
        assert!(observe_due(PEAK - step, PEAK, PEAK, 1_000, 1_030));
        assert!(observe_due(PEAK + step, PEAK, PEAK, 1_000, 1_030));
        assert!(!observe_due(PEAK - step + 1, PEAK, PEAK, 1_000, 1_030));
    }

    #[test]
    fn the_heartbeat_marks_a_quiet_mandate() {
        assert!(observe_due(PEAK, PEAK, PEAK, 1_000, 1_000 + HEARTBEAT_SECS));
    }

    /// Never observed means `last_observed_at` is 0: the first mark is always due.
    #[test]
    fn a_mandate_never_marked_is_marked() {
        assert!(observe_due(PEAK, 0, PEAK, 0, 1_700_000_000));
    }

    #[test]
    fn the_token_amount_is_read_from_the_spl_layout() {
        let mut data = vec![0u8; 165];
        data[64..72].copy_from_slice(&123_456u64.to_le_bytes());
        assert_eq!(token_amount(&data), Some(123_456));
        assert_eq!(token_amount(&data[..100]), None);
    }

    fn sim(key: u8, direction: Direction, stop_price: i64) -> SimPosition {
        SimPosition {
            key: Pubkey::new_from_array([key; 32]),
            market_index: 5,
            direction,
            stop_price,
        }
    }

    fn at(spot: i64) -> impl Fn(u16) -> Option<Priced> {
        move |_| {
            Some(Priced {
                legs: vec![
                    Pubkey::new_from_array([1; 32]),
                    Pubkey::new_from_array([2; 32]),
                ],
                spot,
                closable: true,
            })
        }
    }

    /// Judged as SolFX judges a real stop: inclusively, on the oracle price.
    #[test]
    fn a_long_simulated_stop_fires_at_its_price_and_not_above() {
        let long = [sim(1, Direction::Long, 99)];
        assert!(matches!(
            plan_evaluation(1, &long, at(99)).as_slice(),
            [EvalStep::TriggerStop { .. }]
        ));
        assert!(matches!(
            plan_evaluation(1, &long, at(100)).as_slice(),
            [EvalStep::Observe { .. }]
        ));
    }

    #[test]
    fn a_short_simulated_stop_fires_at_its_price_and_not_below() {
        let short = [sim(1, Direction::Short, 101)];
        assert!(matches!(
            plan_evaluation(1, &short, at(101)).as_slice(),
            [EvalStep::TriggerStop { .. }]
        ));
        assert!(matches!(
            plan_evaluation(1, &short, at(100)).as_slice(),
            [EvalStep::Observe { .. }]
        ));
    }

    #[test]
    fn an_evaluation_is_marked_with_one_triple_per_position() {
        let two = [sim(1, Direction::Long, 50), sim(2, Direction::Short, 150)];
        let steps = plan_evaluation(2, &two, at(100));
        let [EvalStep::Observe { triples }] = steps.as_slice() else {
            panic!("expected one observe, got {steps:?}");
        };
        assert_eq!(triples.len(), 6);
        assert_eq!(triples[0], two[0].key);
        assert_eq!(triples[3], two[1].key);
    }

    /// A count that disagrees with what was found is a view caught mid-change; a mark built
    /// from it would be refused as incomplete.
    #[test]
    fn a_mismatched_count_is_not_marked() {
        let one = [sim(1, Direction::Long, 50)];
        assert!(plan_evaluation(2, &one, at(100)).is_empty());
        assert!(plan_evaluation(0, &[], at(100)).is_empty());
    }

    #[test]
    fn a_stop_on_a_closed_market_is_not_fired() {
        let long = [sim(1, Direction::Long, 99)];
        let closed = |_: u16| {
            Some(Priced {
                legs: vec![
                    Pubkey::new_from_array([1; 32]),
                    Pubkey::new_from_array([2; 32]),
                ],
                spot: 90,
                closable: false,
            })
        };
        assert!(matches!(
            plan_evaluation(1, &long, closed).as_slice(),
            [EvalStep::Observe { .. }]
        ));
    }

    /// The crank derives a position's address from SolFX's own seed constant; spelled out here
    /// byte for byte, so a change to either side fails this rather than a transaction.
    #[test]
    fn the_position_address_follows_solfx_seeds() {
        let a = position_pda(&USER, 3, 0);
        let b = Pubkey::find_program_address(
            &[b"position", USER.as_ref(), &3u16.to_le_bytes(), &[0]],
            &solfx_core::ID,
        )
        .0;
        assert_eq!(a, b);
    }
}
