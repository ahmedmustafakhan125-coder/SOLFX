//! The marketplace: how an investor and a trader find each other and agree terms.
//!
//! # The shape, and why it is this one
//!
//! Two sides who do not trust each other have to agree a rule set and then have it bind. The
//! canonical Solana answer to that shape is the escrow — a maker locks value and publishes
//! terms, a taker fulfils atomically, the maker refunds while nobody has taken. This module is
//! that pattern with a mandate on the other end instead of a token swap.
//!
//! - A **trader** posts a `TraderListing`. No money, no custody: an index entry an investor can
//!   find, read, and check against the `TraderProfile` the trader cannot curate.
//! - An **investor** posts a `MandateOffer`. The principal moves into escrow at that moment and
//!   the full rule set is fixed. This is addressed to one trader.
//! - The **trader accepts**, and in one transaction the `Mandate`, its signer and its vault come
//!   into existence and the escrow empties into it. There is no window in which the terms are
//!   agreed but the money has not moved, and none in which either side can substitute a number.
//! - Or the **investor revokes** and takes the principal back, which they can do at any time
//!   before acceptance.
//!
//! # Why there is no messaging protocol here
//!
//! The terms are the message. What words remain ride in a bounded `note` on an account that
//! already exists. Chat as account state would be public forever, unencrypted, individually
//! rent-bearing per message, and spammable by anyone willing to pay for an
//! account — and none of that would make the binding part any more binding than the escrow
//! already makes it.
//!
//! # The griefing vector this deliberately avoids
//!
//! `accept_offer` moves `offer.principal` — the number the investor committed to — and never a
//! balance read from a token account at execution time. The reference escrow in
//! `program-examples` carries a regression test for precisely the other choice: it compared a
//! post-transfer balance against the wrong pre-transfer variable, so any third party could brick
//! an offer for free by sending one base unit to an account before the take. Deriving an amount
//! from a balance a stranger can move is the bug; committing the number up front is the fix.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::constants::{
    CONFIG_SEED, INVESTOR_LISTING_SEED, LISTING_SEED, MANDATE_SEED, MANDATE_SIGNER_SEED,
    MANDATE_VAULT_SEED, MAX_NICKNAME_LEN, MAX_NOTE_LEN, OFFER_SEED, OFFER_VAULT_SEED, REQUEST_SEED,
    TRADER_SEED,
};
use crate::errors::NoxError;
use crate::events::{
    InvestorListingClosed, InvestorListingPosted, ListingClosed, ListingPosted, MandateFunded,
    OfferAccepted, OfferDeclined, OfferPosted, OfferRevoked, RequestClosed, RequestPosted,
};
use crate::instructions::investor::MandateRules;
use crate::state::{
    FundingRequest, InvestorListing, Mandate, MandateOffer, MandateState, NoxConfig, OfferState,
    TraderListing, TraderProfile,
};

/// Copy a caller-supplied note into a fixed-size field.
///
/// Returns the byte length actually stored. Rejects anything over the cap rather than truncating:
/// a silently truncated note can cut a multi-byte character in half and is a worse outcome than
/// being told the note is too long while you can still edit it.
fn store_bounded<const N: usize>(src: &str, dst: &mut [u8; N], too_long: NoxError) -> Result<u8> {
    let bytes = src.as_bytes();
    // A plain `if`, not `require!`: the macro wants a literal error path and this one is a
    // parameter, so the two callers can name their own.
    if bytes.len() > N {
        return Err(too_long.into());
    }
    *dst = [0u8; N];
    dst.get_mut(..bytes.len())
        .ok_or(too_long)?
        .copy_from_slice(bytes);
    // `bytes.len() <= N` is checked above, and every N here is far below 255.
    u8::try_from(bytes.len()).map_err(|_| too_long.into())
}

fn store_note(src: &str, dst: &mut [u8; MAX_NOTE_LEN]) -> Result<u8> {
    store_bounded(src, dst, NoxError::NoteTooLong)
}

/// A listing's display name, bounded by the bytes the account already reserves.
fn store_nickname(src: &str, dst: &mut [u8; MAX_NICKNAME_LEN]) -> Result<u8> {
    store_bounded(src, dst, NoxError::NicknameTooLong)
}

// --- the trader's side -----------------------------------------------------------------------

/// What a trader advertises. One struct, for the same reason `MandateRules` is one.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ListingTerms {
    /// A display name, shown beside the address and never instead of it.
    pub nickname: String,
    pub min_principal: u64,
    pub max_principal: u64,
    pub wanted_markets: u128,
    pub wanted_split_bps: u16,
    pub note: String,
}

#[derive(Accounts)]
pub struct PostListing<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    /// The record the listing points at. Required, so a listing cannot advertise a trader who
    /// has no track record account at all — the marketplace's whole claim is that the stats
    /// beside a listing are real, and that needs the account to exist.
    #[account(
        seeds = [TRADER_SEED, trader.key().as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == trader.key() @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    /// `init_if_needed` is banned in this workspace, so re-posting is `update_listing` rather
    /// than a second `init`. One listing per trader, at a derived address, forever.
    #[account(
        init,
        payer = trader,
        space = 8 + TraderListing::INIT_SPACE,
        seeds = [LISTING_SEED, trader.key().as_ref()],
        bump,
    )]
    pub listing: Box<Account<'info, TraderListing>>,

    pub system_program: Program<'info, System>,
}

pub fn post_listing(ctx: Context<PostListing>, terms: ListingTerms) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    validate_terms(&terms)?;

    let now = Clock::get()?.unix_timestamp;
    let l = &mut ctx.accounts.listing;
    l.trader = ctx.accounts.trader.key();
    l.min_principal = terms.min_principal;
    l.max_principal = terms.max_principal;
    l.wanted_markets = terms.wanted_markets;
    l.wanted_split_bps = terms.wanted_split_bps;
    l.note_len = store_note(&terms.note, &mut l.note)?;
    l.nickname_len = store_nickname(&terms.nickname, &mut l.nickname)?;
    l.open = true;
    l.created_at = now;
    l.updated_at = now;
    l.bump = ctx.bumps.listing;

    emit!(ListingPosted {
        trader: l.trader,
        listing: l.key(),
        min_principal: l.min_principal,
        max_principal: l.max_principal,
        wanted_markets: l.wanted_markets,
        wanted_split_bps: l.wanted_split_bps,
        timestamp: now,
    });
    Ok(())
}

fn validate_terms(t: &ListingTerms) -> Result<()> {
    require!(t.min_principal > 0, NoxError::InvalidListingTerms);
    require!(
        t.max_principal >= t.min_principal,
        NoxError::InvalidListingTerms
    );
    require!(t.wanted_markets != 0, NoxError::InvalidListingTerms);
    // A trader asking for the entire profit is not negotiating, and an investor bearing all the
    // loss for none of the gain is not investing.
    require!(
        t.wanted_split_bps > 0 && u64::from(t.wanted_split_bps) < crate::constants::BPS,
        NoxError::InvalidListingTerms
    );
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateListing<'info> {
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, trader.key().as_ref()],
        bump = listing.bump,
        has_one = trader @ NoxError::NotTheListingTrader,
    )]
    pub listing: Box<Account<'info, TraderListing>>,
}

/// Edit the terms, or open and close the listing.
///
/// Closing does not delete the account. A deleted listing is a dangling reference for anyone
/// midway through responding to it, and the rent saved is not worth the broken link.
pub fn update_listing(ctx: Context<UpdateListing>, terms: ListingTerms, open: bool) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    validate_terms(&terms)?;

    let now = Clock::get()?.unix_timestamp;
    let l = &mut ctx.accounts.listing;
    let was_open = l.open;

    l.min_principal = terms.min_principal;
    l.max_principal = terms.max_principal;
    l.wanted_markets = terms.wanted_markets;
    l.wanted_split_bps = terms.wanted_split_bps;
    l.note_len = store_note(&terms.note, &mut l.note)?;
    l.nickname_len = store_nickname(&terms.nickname, &mut l.nickname)?;
    l.open = open;
    l.updated_at = now;

    if was_open && !open {
        emit!(ListingClosed {
            trader: l.trader,
            listing: l.key(),
            timestamp: now,
        });
    } else {
        emit!(ListingPosted {
            trader: l.trader,
            listing: l.key(),
            min_principal: l.min_principal,
            max_principal: l.max_principal,
            wanted_markets: l.wanted_markets,
            wanted_split_bps: l.wanted_split_bps,
            timestamp: now,
        });
    }
    Ok(())
}

// --- the investor's side ---------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(seq: u8)]
pub struct PostOffer<'info> {
    #[account(mut)]
    pub investor: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    /// CHECK: the trader this offer is addressed to. Signs nothing here — the whole point is
    /// that an investor can make an offer without the trader's cooperation.
    pub trader: UncheckedAccount<'info>,

    /// Required, so the tier limits can be checked before the investor's capital moves rather
    /// than when the trader tries to accept.
    #[account(
        seeds = [TRADER_SEED, trader.key().as_ref()],
        bump = trader_profile.bump,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    #[account(
        init,
        payer = investor,
        space = 8 + MandateOffer::INIT_SPACE,
        seeds = [OFFER_SEED, investor.key().as_ref(), trader.key().as_ref(), &[seq]],
        bump,
    )]
    pub offer: Box<Account<'info, MandateOffer>>,

    #[account(address = config.usdc_mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        token::mint = usdc_mint,
        token::authority = investor,
    )]
    pub investor_token: Box<Account<'info, TokenAccount>>,

    /// The escrow. Authority is the offer PDA itself, which holds no data problem here because
    /// — unlike the mandate signer — it never has to be the `from` of a System Program transfer.
    #[account(
        init,
        payer = investor,
        seeds = [OFFER_VAULT_SEED, offer.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = offer,
    )]
    pub offer_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn post_offer(
    ctx: Context<PostOffer>,
    seq: u8,
    principal: u64,
    rules: MandateRules,
    trader_split_bps: u16,
    expires_at: i64,
    note: String,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    require!(principal > 0, NoxError::ZeroAmount);
    rules.validate()?;
    require!(
        trader_split_bps > 0 && u64::from(trader_split_bps) < crate::constants::BPS,
        NoxError::InvalidMandateRules
    );
    require!(
        ctx.accounts.investor_token.amount >= principal,
        NoxError::InsufficientPrincipal
    );

    let now = Clock::get()?.unix_timestamp;
    require!(expires_at > now, NoxError::OfferExpired);

    // Checked here as well as on acceptance. Both matter: here so the investor is refused while
    // they are still filling in the form, and again on acceptance because a trader's tier and
    // active-mandate count can both move while an offer sits open.
    let tier = ctx.accounts.trader_profile.tier;
    require!(
        principal <= tier.max_mandate(),
        NoxError::MandateExceedsTierLimit
    );

    escrow_principal(&ctx, principal)?;

    let o = &mut ctx.accounts.offer;
    o.investor = ctx.accounts.investor.key();
    o.trader = ctx.accounts.trader.key();
    o.seq = seq;
    o.principal = principal;
    o.max_trade_notional = rules.max_trade_notional;
    o.max_total_notional = rules.max_total_notional;
    o.max_drawdown_bps = rules.max_drawdown_bps;
    o.max_daily_loss_bps = rules.max_daily_loss_bps;
    o.max_risk_per_trade_bps = rules.max_risk_per_trade_bps;
    o.max_stop_distance_bps = rules.max_stop_distance_bps;
    o.max_concurrent_positions = rules.max_concurrent_positions;
    o.allowed_markets = rules.allowed_markets;
    o.min_hold_slots = rules.min_hold_slots;
    o.trader_split_bps = trader_split_bps;
    o.note_len = store_note(&note, &mut o.note)?;
    o.expires_at = expires_at;
    o.state = OfferState::Open;
    o.created_at = now;
    o.bump = ctx.bumps.offer;
    o.vault_bump = ctx.bumps.offer_vault;

    emit!(OfferPosted {
        offer: o.key(),
        investor: o.investor,
        trader: o.trader,
        seq,
        principal,
        trader_split_bps,
        expires_at,
        timestamp: now,
    });
    Ok(())
}

/// Its own function so the `CpiContext` does not share a frame with the offer's initialisation.
#[inline(never)]
fn escrow_principal(ctx: &Context<PostOffer>, principal: u64) -> Result<()> {
    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.investor_token.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.offer_vault.to_account_info(),
                authority: ctx.accounts.investor.to_account_info(),
            },
        ),
        principal,
        ctx.accounts.usdc_mint.decimals,
    )
}

#[derive(Accounts)]
pub struct RevokeOffer<'info> {
    #[account(mut)]
    pub investor: Signer<'info>,

    #[account(
        mut,
        seeds = [OFFER_SEED, investor.key().as_ref(), offer.trader.as_ref(), &[offer.seq]],
        bump = offer.bump,
        has_one = investor @ NoxError::NotTheOfferInvestor,
    )]
    pub offer: Box<Account<'info, MandateOffer>>,

    #[account(address = offer_vault.mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        seeds = [OFFER_VAULT_SEED, offer.key().as_ref()],
        bump = offer.vault_bump,
    )]
    pub offer_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = usdc_mint,
        token::authority = investor,
    )]
    pub investor_token: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

/// Take the principal back.
///
/// Available at any time while the offer is `Open`, expired or not — the investor's capital is
/// theirs until a trader has actually signed for it. Deliberately not gated on the pause flag:
/// a paused protocol must never be able to strand an investor's money in an escrow.
pub fn revoke_offer(ctx: Context<RevokeOffer>) -> Result<()> {
    // `Declined` as well as `Open`: a trader saying no must never leave the capital stuck.
    require!(
        matches!(
            ctx.accounts.offer.state,
            OfferState::Open | OfferState::Declined
        ),
        NoxError::OfferNotOpen
    );

    let amount = ctx.accounts.offer.principal;
    refund_principal(&ctx, amount)?;

    let now = Clock::get()?.unix_timestamp;
    let o = &mut ctx.accounts.offer;
    o.state = OfferState::Revoked;

    emit!(OfferRevoked {
        offer: o.key(),
        investor: o.investor,
        trader: o.trader,
        principal: amount,
        timestamp: now,
    });
    Ok(())
}

#[inline(never)]
fn refund_principal(ctx: &Context<RevokeOffer>, amount: u64) -> Result<()> {
    // The offer PDA signs for its own vault. Seeds must match the derivation exactly, bump
    // included, and the bump is the stored one rather than a fresh `find_program_address`.
    let investor = ctx.accounts.offer.investor;
    let trader = ctx.accounts.offer.trader;
    let seq = [ctx.accounts.offer.seq];
    let bump = [ctx.accounts.offer.bump];
    let seeds: &[&[&[u8]]] = &[&[OFFER_SEED, investor.as_ref(), trader.as_ref(), &seq, &bump]];
    token::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.offer_vault.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.investor_token.to_account_info(),
                authority: ctx.accounts.offer.to_account_info(),
            },
            seeds,
        ),
        amount,
        ctx.accounts.usdc_mint.decimals,
    )
}

// --- acceptance: the atomic step ---------------------------------------------------------

#[derive(Accounts)]
pub struct AcceptOffer<'info> {
    /// The trader the offer is addressed to, and the only account that can do this.
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [OFFER_SEED, offer.investor.as_ref(), trader.key().as_ref(), &[offer.seq]],
        bump = offer.bump,
        constraint = offer.trader == trader.key() @ NoxError::NotTheOfferTrader,
    )]
    pub offer: Box<Account<'info, MandateOffer>>,

    #[account(
        mut,
        seeds = [TRADER_SEED, trader.key().as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == trader.key() @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    /// The mandate the offer becomes.
    ///
    /// Note the payer: the **trader**. An offer's escrow holds exactly the principal and nothing
    /// for rent, and the investor is not a signer on this transaction, so the account cannot be
    /// funded from either. That is the right incentive anyway — accepting capital costs the
    /// trader the rent, which prices in a trader who accepts offers they do not intend to trade.
    #[account(
        init,
        payer = trader,
        space = 8 + Mandate::INIT_SPACE,
        seeds = [MANDATE_SEED, offer.investor.as_ref(), trader.key().as_ref(), &[offer.seq]],
        bump,
    )]
    pub mandate: Box<Account<'info, Mandate>>,

    /// Dataless, so the System Program will accept it as the `from` of the rent transfers that
    /// `open_position` and `place_trigger_order` perform. See `Mandate`'s documentation.
    #[account(
        mut,
        seeds = [MANDATE_SIGNER_SEED, mandate.key().as_ref()],
        bump,
    )]
    pub mandate_signer: SystemAccount<'info>,

    /// CHECK: the SolFX `UserAccount` this mandate will trade through. Validated by `solfx-core`
    /// on every CPI; only its address is recorded.
    pub solfx_user_account: UncheckedAccount<'info>,

    #[account(address = config.usdc_mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        seeds = [OFFER_VAULT_SEED, offer.key().as_ref()],
        bump = offer.vault_bump,
    )]
    pub offer_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = trader,
        seeds = [MANDATE_VAULT_SEED, mandate.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = mandate_signer,
    )]
    pub mandate_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Turn an open offer into a funded mandate, in one transaction.
///
/// Everything happens together or nothing does: the mandate is created, the rules are copied
/// from the offer verbatim, the escrow empties into the mandate's vault, and the offer is marked
/// `Accepted`. There is no intermediate state in which the trader has agreed but the capital has
/// not moved, and no step at which either party could substitute a different number for one the
/// other agreed to.
pub fn accept_offer(ctx: Context<AcceptOffer>) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    require!(
        ctx.accounts.offer.state == OfferState::Open,
        NoxError::OfferNotOpen
    );

    let now = Clock::get()?.unix_timestamp;
    require!(now <= ctx.accounts.offer.expires_at, NoxError::OfferExpired);

    // Re-checked here and not only at `post_offer`. An offer can sit open for days, and in that
    // time the trader may have been promoted, demoted, or filled every mandate slot their tier
    // allows. The check that matters is the one at the moment capital actually moves.
    let tier = ctx.accounts.trader_profile.tier;
    require!(
        ctx.accounts.offer.principal <= tier.max_mandate(),
        NoxError::MandateExceedsTierLimit
    );
    require!(
        ctx.accounts.trader_profile.active_mandates < tier.max_concurrent_mandates(),
        NoxError::TooManyActiveMandates
    );

    // The committed number, never a balance read at execution time — see the module header.
    let principal = ctx.accounts.offer.principal;
    release_escrow(&ctx, principal)?;

    let o = &ctx.accounts.offer;
    let m = &mut ctx.accounts.mandate;
    m.investor = o.investor;
    m.trader = ctx.accounts.trader.key();
    m.seq = o.seq;
    m.solfx_user_account = ctx.accounts.solfx_user_account.key();
    m.signer_bump = ctx.bumps.mandate_signer;
    m.principal = principal;
    m.peak_equity = principal;
    m.max_trade_notional = o.max_trade_notional;
    m.max_total_notional = o.max_total_notional;
    m.max_drawdown_bps = o.max_drawdown_bps;
    m.max_daily_loss_bps = o.max_daily_loss_bps;
    m.max_risk_per_trade_bps = o.max_risk_per_trade_bps;
    m.max_stop_distance_bps = o.max_stop_distance_bps;
    m.max_concurrent_positions = o.max_concurrent_positions;
    m.allowed_markets = o.allowed_markets;
    m.min_hold_slots = o.min_hold_slots;
    m.trader_split_bps = o.trader_split_bps;
    m.state = MandateState::Active;
    m.opened_at = now;
    m.bump = ctx.bumps.mandate;
    m.vault_bump = ctx.bumps.mandate_vault;

    let mandate_key = m.key();

    let p = &mut ctx.accounts.trader_profile;
    p.mandates_funded = p.mandates_funded.saturating_add(1);
    p.active_mandates = p.active_mandates.saturating_add(1);

    let offer = &mut ctx.accounts.offer;
    offer.state = OfferState::Accepted;

    emit!(OfferAccepted {
        offer: offer.key(),
        mandate: mandate_key,
        investor: offer.investor,
        trader: offer.trader,
        principal,
        timestamp: now,
    });
    // Emitted as well, so an indexer watching for funded mandates sees this one whether it
    // arrived through `fund_mandate` or through the marketplace. Same event, same shape — a
    // consumer should not have to know which door the mandate came through.
    emit!(MandateFunded {
        mandate: mandate_key,
        investor: offer.investor,
        trader: offer.trader,
        principal,
        max_trade_notional: offer.max_trade_notional,
        max_drawdown_bps: offer.max_drawdown_bps,
        allowed_markets: offer.allowed_markets,
        trader_split_bps: offer.trader_split_bps,
        ts: now,
    });
    Ok(())
}

/// Its own frame, so the `CpiContext` never coexists with the mandate initialisation above.
#[inline(never)]
fn release_escrow(ctx: &Context<AcceptOffer>, amount: u64) -> Result<()> {
    let investor = ctx.accounts.offer.investor;
    let trader = ctx.accounts.offer.trader;
    let seq = [ctx.accounts.offer.seq];
    let bump = [ctx.accounts.offer.bump];
    let seeds: &[&[&[u8]]] = &[&[OFFER_SEED, investor.as_ref(), trader.as_ref(), &seq, &bump]];
    token::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.offer_vault.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.mandate_vault.to_account_info(),
                authority: ctx.accounts.offer.to_account_info(),
            },
            seeds,
        ),
        amount,
        ctx.accounts.usdc_mint.decimals,
    )
}

// --- the conversation: declining, investor listings, and requests ------------------------
//
// Every message below is a typed object tied to a real step, never free text on its own:
//
//   investor → trader   an offer (escrowed)         → accept, or decline with a reason
//   trader → investor   a request (to a listing)    → answered with an offer, or closed
//
// That closes the loop — offer, decline with reason, revised offer, accept — without a channel
// anyone can write arbitrary text into, and every object here is bounded, attributable and
// closable by the side that paid for it.

#[derive(Accounts)]
pub struct DeclineOffer<'info> {
    pub trader: Signer<'info>,

    #[account(
        mut,
        seeds = [OFFER_SEED, offer.investor.as_ref(), trader.key().as_ref(), &[offer.seq]],
        bump = offer.bump,
        constraint = offer.trader == trader.key() @ NoxError::NotTheOfferTrader,
    )]
    pub offer: Box<Account<'info, MandateOffer>>,
}

/// Say no to an offer, and say why.
///
/// Moves no money: the principal stays in escrow until the investor revokes, which they always
/// can. So declining is a message and nothing more, and a trader cannot use it to strand or
/// redirect anyone's capital.
///
/// Not gated on the pause flag. A pause stops new commitments; a refusal only removes one.
pub fn decline_offer(ctx: Context<DeclineOffer>, reason: String) -> Result<()> {
    require!(
        ctx.accounts.offer.state == OfferState::Open,
        NoxError::OfferNotOpen
    );

    let now = Clock::get()?.unix_timestamp;
    let o = &mut ctx.accounts.offer;
    o.reply_len = store_note(&reason, &mut o.reply)?;
    o.state = OfferState::Declined;

    emit!(OfferDeclined {
        offer: o.key(),
        investor: o.investor,
        trader: o.trader,
        timestamp: now,
    });
    Ok(())
}

/// What an investor advertises. Advisory throughout: the binding numbers are on the offer.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct InvestorListingTerms {
    /// A display name, shown beside the address and never instead of it.
    pub nickname: String,
    pub min_principal: u64,
    pub max_principal: u64,
    pub max_drawdown_bps: u16,
    pub max_risk_per_trade_bps: u16,
    pub allowed_markets: u128,
    pub offered_split_bps: u16,
    pub note: String,
}

fn validate_investor_terms(t: &InvestorListingTerms) -> Result<()> {
    require!(t.min_principal > 0, NoxError::InvalidListingTerms);
    require!(
        t.max_principal >= t.min_principal,
        NoxError::InvalidListingTerms
    );
    require!(t.allowed_markets != 0, NoxError::InvalidListingTerms);
    require!(
        t.max_drawdown_bps > 0 && u64::from(t.max_drawdown_bps) <= crate::constants::BPS,
        NoxError::InvalidListingTerms
    );
    // Risk per trade above the drawdown limit is incoherent — one stop-out would breach it —
    // the same rule `MandateRules::validate` applies to the binding version.
    require!(
        t.max_risk_per_trade_bps > 0 && t.max_risk_per_trade_bps <= t.max_drawdown_bps,
        NoxError::InvalidListingTerms
    );
    require!(
        t.offered_split_bps > 0 && u64::from(t.offered_split_bps) < crate::constants::BPS,
        NoxError::InvalidListingTerms
    );
    Ok(())
}

#[derive(Accounts)]
pub struct PostInvestorListing<'info> {
    #[account(mut)]
    pub investor: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        init,
        payer = investor,
        space = 8 + InvestorListing::INIT_SPACE,
        seeds = [INVESTOR_LISTING_SEED, investor.key().as_ref()],
        bump,
    )]
    pub listing: Box<Account<'info, InvestorListing>>,

    pub system_program: Program<'info, System>,
}

pub fn post_investor_listing(
    ctx: Context<PostInvestorListing>,
    terms: InvestorListingTerms,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    validate_investor_terms(&terms)?;

    let now = Clock::get()?.unix_timestamp;
    let l = &mut ctx.accounts.listing;
    l.investor = ctx.accounts.investor.key();
    write_investor_terms(l, &terms)?;
    l.open = true;
    l.created_at = now;
    l.updated_at = now;
    l.bump = ctx.bumps.listing;

    emit!(InvestorListingPosted {
        investor: l.investor,
        listing: l.key(),
        min_principal: l.min_principal,
        max_principal: l.max_principal,
        allowed_markets: l.allowed_markets,
        offered_split_bps: l.offered_split_bps,
        timestamp: now,
    });
    Ok(())
}

fn write_investor_terms(l: &mut InvestorListing, t: &InvestorListingTerms) -> Result<()> {
    l.min_principal = t.min_principal;
    l.max_principal = t.max_principal;
    l.max_drawdown_bps = t.max_drawdown_bps;
    l.max_risk_per_trade_bps = t.max_risk_per_trade_bps;
    l.allowed_markets = t.allowed_markets;
    l.offered_split_bps = t.offered_split_bps;
    l.note_len = store_note(&t.note, &mut l.note)?;
    l.nickname_len = store_nickname(&t.nickname, &mut l.nickname)?;
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateInvestorListing<'info> {
    pub investor: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    #[account(
        mut,
        seeds = [INVESTOR_LISTING_SEED, investor.key().as_ref()],
        bump = listing.bump,
        has_one = investor @ NoxError::NotTheListingInvestor,
    )]
    pub listing: Box<Account<'info, InvestorListing>>,
}

/// Edit the terms, or open and close the listing. Closing keeps the account, as for traders.
pub fn update_investor_listing(
    ctx: Context<UpdateInvestorListing>,
    terms: InvestorListingTerms,
    open: bool,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    validate_investor_terms(&terms)?;

    let now = Clock::get()?.unix_timestamp;
    let l = &mut ctx.accounts.listing;
    let was_open = l.open;
    write_investor_terms(l, &terms)?;
    l.open = open;
    l.updated_at = now;

    if was_open && !open {
        emit!(InvestorListingClosed {
            investor: l.investor,
            listing: l.key(),
            timestamp: now,
        });
    } else {
        emit!(InvestorListingPosted {
            investor: l.investor,
            listing: l.key(),
            min_principal: l.min_principal,
            max_principal: l.max_principal,
            allowed_markets: l.allowed_markets,
            offered_split_bps: l.offered_split_bps,
            timestamp: now,
        });
    }
    Ok(())
}

#[derive(Accounts)]
pub struct PostRequest<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, NoxConfig>>,

    /// Required, so the investor reading the request can read a real track record beside it.
    #[account(
        seeds = [TRADER_SEED, trader.key().as_ref()],
        bump = trader_profile.bump,
        constraint = trader_profile.authority == trader.key() @ NoxError::ProfileMismatch,
    )]
    pub trader_profile: Box<Account<'info, TraderProfile>>,

    /// The investor's listing, which must be open.
    ///
    /// This is the anti-spam rule. A request can only go to an investor who has publicly said
    /// they are looking, so no wallet can be messaged merely for existing — and closing the
    /// listing is how an investor shuts the door.
    #[account(
        seeds = [INVESTOR_LISTING_SEED, investor_listing.investor.as_ref()],
        bump = investor_listing.bump,
    )]
    pub investor_listing: Box<Account<'info, InvestorListing>>,

    /// One open request per pair. The seeds make a duplicate impossible, so a trader cannot
    /// flood an inbox with copies of the same ask.
    #[account(
        init,
        payer = trader,
        space = 8 + FundingRequest::INIT_SPACE,
        seeds = [REQUEST_SEED, trader.key().as_ref(), investor_listing.investor.as_ref()],
        bump,
    )]
    pub request: Box<Account<'info, FundingRequest>>,

    pub system_program: Program<'info, System>,
}

pub fn post_request(
    ctx: Context<PostRequest>,
    wanted_principal: u64,
    wanted_split_bps: u16,
    note: String,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, NoxError::ProtocolPaused);
    require!(ctx.accounts.investor_listing.open, NoxError::ListingNotOpen);
    require!(wanted_principal > 0, NoxError::ZeroAmount);
    require!(
        wanted_split_bps > 0 && u64::from(wanted_split_bps) < crate::constants::BPS,
        NoxError::InvalidListingTerms
    );

    let now = Clock::get()?.unix_timestamp;
    let r = &mut ctx.accounts.request;
    r.trader = ctx.accounts.trader.key();
    r.investor = ctx.accounts.investor_listing.investor;
    r.wanted_principal = wanted_principal;
    r.wanted_split_bps = wanted_split_bps;
    r.note_len = store_note(&note, &mut r.note)?;
    r.created_at = now;
    r.bump = ctx.bumps.request;

    emit!(RequestPosted {
        request: r.key(),
        trader: r.trader,
        investor: r.investor,
        wanted_principal,
        wanted_split_bps,
        timestamp: now,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct CloseRequest<'info> {
    /// Either party. The trader withdraws the ask; the investor dismisses it.
    pub closer: Signer<'info>,

    /// Where the rent goes: always back to the trader who paid it, whoever closes. An investor
    /// dismissing a request must not be able to pocket the trader's deposit.
    #[account(mut, address = request.trader @ NoxError::NotARequestParty)]
    pub trader: SystemAccount<'info>,

    #[account(
        mut,
        close = trader,
        seeds = [REQUEST_SEED, request.trader.as_ref(), request.investor.as_ref()],
        bump = request.bump,
        constraint = closer.key() == request.trader || closer.key() == request.investor
            @ NoxError::NotARequestParty,
    )]
    pub request: Box<Account<'info, FundingRequest>>,
}

/// Withdraw or dismiss a request. Not gated on the pause flag: it only removes a commitment.
pub fn close_request(ctx: Context<CloseRequest>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let r = &ctx.accounts.request;
    emit!(RequestClosed {
        request: r.key(),
        trader: r.trader,
        investor: r.investor,
        closed_by: ctx.accounts.closer.key(),
        timestamp: now,
    });
    Ok(())
}
