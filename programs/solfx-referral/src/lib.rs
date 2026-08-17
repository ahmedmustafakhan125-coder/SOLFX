//! # solfx-referral — the introducing-broker ledger
//!
//! `ARCHITECTURE.md` § 8.5 calls this the highest-leverage product decision in the project,
//! and § 5.1 requires it be a **separate program**.
//!
//! ## Why separate
//!
//! The financial core wants to be frozen after audit. The growth engine will not be: tiers
//! get retuned, override rules change, campaign logic gets bolted on. Keeping them apart
//! means the referral programme can be upgraded and re-audited without reopening the
//! financial core — and it only works because **the dependency points one way**. This
//! program reads `solfx-core`'s public state and calls one instruction to be paid; core
//! knows nothing about it beyond a pubkey it was told to trust.
//!
//! ## Why it is the moat
//!
//! XM and Exness did not win on spreads. They won on IB networks. And every IB in that
//! industry has the same complaint: *the broker controls the ledger* — rebates miscounted,
//! clients reassigned, payouts delayed, and no way to verify any of it.
//!
//! Three structural fixes, none of which a legacy broker can copy:
//!
//! 1. **The binding is immutable.** `UserAccount.referrer` is written once at account
//!    creation in `solfx-core` and there is no instruction anywhere in either program that
//!    changes it. Clients cannot be reassigned because nothing can reassign them.
//! 2. **Accrual is derived, not asserted.** An IB's entitlement is computed from
//!    `UserAccount.referral_fees_generated` — a monotonic counter on a public account.
//!    Anyone can recompute every rebate independently, without trusting this program, the
//!    indexer, or us.
//! 3. **Claims are permissionless.** No approval, no payout schedule, no counterparty. An
//!    IB calls `claim` whenever they like.
//!
//! *"The smart contract is replicable in a few months by anyone. A network of IBs who trust
//! your ledger because they can audit it is not."*
//!
//! ## The flow
//!
//! ```text
//!  trader trades  ──► solfx-core: UserAccount.referral_fees_generated += referral share
//!                                 Protocol.total_referral_accrued     += same
//!                                        │  (no CPI, no extra accounts — just a counter
//!                                        │   on an account the trade already loads)
//!  anyone calls   ──► sync_trader ───────┘  reads that counter, credits the delta to the
//!                                           IB (and the parent's override), advances the
//!                                           TraderLink watermark so it cannot double-count
//!  IB calls       ──► claim ─────────────►  CPI into solfx-core::pay_referral
//! ```
//!
//! ## Lints
//!
//! Same relaxation as `solfx-core`, for the same reason: Anchor's macros trip four lints
//! that cannot be scoped to generated code, and the arithmetic that matters lives in
//! `solfx-math` under the full strict set.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    clippy::result_large_err
)]

use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use solfx_math::referral::{entitlement, parent_override, IbTier};

use solfx_core::program::SolfxCore;
use solfx_core::state::{Protocol, UserAccount};

declare_id!("J7dwkNcyPjHtRyqkpnqq3wgE6XmYCaENhYZozX2MHsyt");

pub const CONFIG_SEED: &[u8] = b"referral_config";
pub const IB_SEED: &[u8] = b"ib";
pub const LINK_SEED: &[u8] = b"link";

/// A sub-broker's parent earns this share of what the child earns (§ 8.5's 2-level rule).
pub const DEFAULT_OVERRIDE_BPS: u16 = 2_000;

// --- state --------------------------------------------------------------------------------

/// Programme configuration, and the PDA that signs claims against `solfx-core`.
#[account]
#[derive(InitSpace)]
pub struct ReferralConfig {
    pub admin: Pubkey,
    /// The `solfx-core` protocol account this programme draws from.
    pub protocol: Pubkey,
    /// Parent's share of a sub-broker's earnings, in bps.
    pub override_bps: u16,
    /// Mirror of `Protocol.fee_split_referral_bps` at registration, needed to reconstruct
    /// the original fee from a pool share. Refreshed by `sync_split`, so a change to the
    /// core split cannot silently mis-price every entitlement.
    pub pool_split_bps: u16,
    pub total_accrued: u64,
    pub total_claimed: u64,
    pub ib_count: u32,
    pub bump: u8,
    pub _reserved: [u8; 64],
}

/// An introducing broker. PDA at `["ib", authority]`.
#[account]
#[derive(InitSpace)]
pub struct IbAccount {
    pub authority: Pubkey,
    /// The IB who recruited this one, or `Pubkey::default()`.
    ///
    /// **Immutable, exactly like a trader's referrer.** A network whose parent links could
    /// be edited would have the same trust problem as a broker who can reassign clients.
    pub parent: Pubkey,
    /// Earned and not yet claimed, in USDC.
    pub unclaimed: u64,
    pub lifetime_earned: u64,
    pub lifetime_claimed: u64,
    /// Referred trading volume driving the tier (§ 8.5).
    pub referred_volume: u64,
    /// Referral-pool money this IB's traders have generated, before the tier is applied.
    pub referred_pool_share: u64,
    pub referred_trader_count: u32,
    pub created_at: i64,
    pub bump: u8,
    pub _reserved: [u8; 64],
}

impl IbAccount {
    #[must_use]
    pub fn tier(&self) -> IbTier {
        IbTier::for_volume(self.referred_volume)
    }
}

/// How much of one trader's lifetime contribution has already been credited to their IB.
/// PDA at `["link", ib, user_account]`.
///
/// This watermark is what makes `sync_trader` **idempotent and permissionless**: it can be
/// called by anyone, any number of times, and only ever credits the delta since last time.
/// Without it, a caller could credit the same trade repeatedly.
#[account]
#[derive(InitSpace)]
pub struct TraderLink {
    pub ib: Pubkey,
    pub user_account: Pubkey,
    /// Watermark on `UserAccount.referral_fees_generated`.
    pub credited_pool_share: u64,
    /// Watermark on `UserAccount.lifetime_volume`.
    pub credited_volume: u64,
    pub bump: u8,
    pub _reserved: [u8; 32],
}

// --- events -------------------------------------------------------------------------------

#[event]
pub struct IbRegistered {
    pub ib: Pubkey,
    pub authority: Pubkey,
    pub parent: Pubkey,
    pub ts: i64,
}

/// A rebate accrued. **The verifiable record § 8.5 asks for.**
///
/// Carries the inputs as well as the result — the trader's watermark, the volume, the tier
/// applied — so an IB can recompute the figure from public data and see exactly how it was
/// reached rather than being handed a number.
#[event]
pub struct RebateAccrued {
    pub ib: Pubkey,
    pub user_account: Pubkey,
    pub pool_share_delta: u64,
    pub volume_delta: u64,
    pub tier: u8,
    pub earned: u64,
    pub parent: Pubkey,
    pub parent_override: u64,
    pub ib_unclaimed_after: u64,
    pub ts: i64,
}

#[event]
pub struct RebateClaimed {
    pub ib: Pubkey,
    pub authority: Pubkey,
    pub amount: u64,
    pub lifetime_claimed: u64,
    pub ts: i64,
}

// --- errors -------------------------------------------------------------------------------

#[error_code]
pub enum ReferralError {
    #[msg("Arithmetic overflow")]
    MathOverflow,
    #[msg("Signer is not the referral programme admin")]
    NotAdmin,
    #[msg("This trader was not referred by this introducing broker")]
    NotReferredByThisIb,
    #[msg("An introducing broker cannot recruit themselves")]
    SelfParent,
    #[msg("Nothing to claim")]
    NothingToClaim,
    #[msg("Nothing new to sync")]
    NothingToSync,
    #[msg("Wrong protocol account for this referral programme")]
    WrongProtocol,
    #[msg("Parent account does not match the IB's recorded parent")]
    WrongParent,
}

// --- program ------------------------------------------------------------------------------

#[program]
pub mod solfx_referral {
    use super::*;

    /// One-time setup. The config PDA becomes the authority `solfx-core` will honour, so the
    /// core admin must register `config.key()` via `set_referral_authority` before any claim
    /// can succeed — two deliberate steps, by two different authorities.
    pub fn initialize_referral(ctx: Context<InitializeReferral>, override_bps: u16) -> Result<()> {
        require!(override_bps <= 10_000, ReferralError::MathOverflow);

        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.protocol = ctx.accounts.protocol.key();
        config.override_bps = override_bps;
        config.pool_split_bps = ctx.accounts.protocol.fee_split_referral_bps;
        config.total_accrued = 0;
        config.total_claimed = 0;
        config.ib_count = 0;
        config.bump = ctx.bumps.config;
        Ok(())
    }

    /// Re-read the core's referral split.
    ///
    /// Permissionless: it copies a public number from a verified account, so there is
    /// nothing to gain by calling it and something to lose by being unable to. If the admin
    /// widens the pool to fund Gold and Diamond tiers in full, this is what makes the
    /// entitlement maths see it.
    pub fn sync_split(ctx: Context<SyncSplit>) -> Result<()> {
        ctx.accounts.config.pool_split_bps = ctx.accounts.protocol.fee_split_referral_bps;
        Ok(())
    }

    /// Register as an introducing broker, optionally under a parent.
    ///
    /// Permissionless — anyone may become an IB; earning anything still requires a trader to
    /// have named them at account creation. The parent link is written once here and is
    /// immutable thereafter, for the same reason the trader binding is.
    pub fn register_ib(ctx: Context<RegisterIb>, parent: Pubkey) -> Result<()> {
        let authority = ctx.accounts.authority.key();
        require!(parent != authority, ReferralError::SelfParent);

        let clock = Clock::get()?;
        let ib = &mut ctx.accounts.ib_account;
        ib.authority = authority;
        ib.parent = parent;
        ib.unclaimed = 0;
        ib.lifetime_earned = 0;
        ib.lifetime_claimed = 0;
        ib.referred_volume = 0;
        ib.referred_pool_share = 0;
        ib.referred_trader_count = 0;
        ib.created_at = clock.unix_timestamp;
        ib.bump = ctx.bumps.ib_account;

        let config = &mut ctx.accounts.config;
        config.ib_count = config.ib_count.saturating_add(1);

        emit!(IbRegistered {
            ib: ib.key(),
            authority,
            parent,
            ts: clock.unix_timestamp,
        });
        Ok(())
    }

    /// Credit an IB for everything a referred trader has generated since the last sync.
    ///
    /// # Permissionless, and idempotent
    ///
    /// Anyone may call this — the IB, the trader, a keeper, a stranger. It reads two
    /// monotonic counters on a `solfx-core` account (Anchor verifies the owner, so the
    /// numbers cannot be forged), credits only the delta above the `TraderLink` watermark,
    /// and advances the watermark. Calling it twice in a row credits nothing the second
    /// time; never calling it loses nothing, because the counters keep accumulating.
    ///
    /// That is what removes the payout schedule an IB would otherwise have to trust.
    ///
    /// # The tier is applied at sync time, on current volume
    ///
    /// An IB who crosses into Gold is paid Gold on everything they sync afterwards, not
    /// retroactively. Stated plainly because the alternative — recomputing history at each
    /// boundary — is both unaffordable on chain and the kind of surprise that starts
    /// disputes.
    pub fn sync_trader(ctx: Context<SyncTrader>) -> Result<()> {
        let user = &ctx.accounts.user_account;
        let ib_authority = ctx.accounts.ib_account.authority;

        // The binding is the whole security model: an IB may only be credited for traders
        // who named them, and that name was written once and cannot be changed.
        require!(
            user.referrer == ib_authority,
            ReferralError::NotReferredByThisIb
        );

        let link = &ctx.accounts.link;
        let pool_delta = user
            .referral_fees_generated
            .saturating_sub(link.credited_pool_share);
        let volume_delta = user.lifetime_volume.saturating_sub(link.credited_volume);
        require!(
            pool_delta > 0 || volume_delta > 0,
            ReferralError::NothingToSync
        );

        // Volume first: an IB who crosses a boundary with this very sync is paid at the new
        // tier for it. Ordering it the other way would mean the trade that promoted them was
        // the last one paid at the old rate, which reads as a penalty for growing.
        let ib = &mut ctx.accounts.ib_account;
        ib.referred_volume = ib
            .referred_volume
            .checked_add(volume_delta)
            .ok_or(ReferralError::MathOverflow)?;
        ib.referred_pool_share = ib
            .referred_pool_share
            .checked_add(pool_delta)
            .ok_or(ReferralError::MathOverflow)?;

        let tier = ib.tier();
        let earned = entitlement(pool_delta, ctx.accounts.config.pool_split_bps, tier)
            .map_err(|_| ReferralError::MathOverflow)?;

        // The parent's override comes out of the same pool share, after the child's cut —
        // so a two-level chain can never cost more than the pool set aside.
        let budget_left = pool_delta.saturating_sub(earned);
        let override_amount = parent_override(earned, ctx.accounts.config.override_bps)
            .map_err(|_| ReferralError::MathOverflow)?
            .min(budget_left);

        ib.unclaimed = ib
            .unclaimed
            .checked_add(earned)
            .ok_or(ReferralError::MathOverflow)?;
        ib.lifetime_earned = ib
            .lifetime_earned
            .checked_add(earned)
            .ok_or(ReferralError::MathOverflow)?;
        let ib_key = ib.key();
        let ib_parent = ib.parent;
        let unclaimed_after = ib.unclaimed;

        // Credit the parent when one is supplied and matches the recorded link.
        let mut paid_override = 0u64;
        if override_amount > 0 && ib_parent != Pubkey::default() {
            if let Some(parent) = ctx.accounts.parent_ib.as_mut() {
                require!(parent.authority == ib_parent, ReferralError::WrongParent);
                parent.unclaimed = parent
                    .unclaimed
                    .checked_add(override_amount)
                    .ok_or(ReferralError::MathOverflow)?;
                parent.lifetime_earned = parent
                    .lifetime_earned
                    .checked_add(override_amount)
                    .ok_or(ReferralError::MathOverflow)?;
                paid_override = override_amount;
            }
            // No parent account supplied: the override simply is not paid, and stays in the
            // pool. Requiring the account would let a missing parent block the child's own
            // rebate, which is the wrong failure.
        }

        let link = &mut ctx.accounts.link;
        if link.ib == Pubkey::default() {
            link.ib = ib_key;
            link.user_account = ctx.accounts.user_account.key();
            link.bump = ctx.bumps.link;
        }
        link.credited_pool_share = ctx.accounts.user_account.referral_fees_generated;
        link.credited_volume = ctx.accounts.user_account.lifetime_volume;

        let config = &mut ctx.accounts.config;
        config.total_accrued = config
            .total_accrued
            .checked_add(earned)
            .ok_or(ReferralError::MathOverflow)?
            .checked_add(paid_override)
            .ok_or(ReferralError::MathOverflow)?;

        emit!(RebateAccrued {
            ib: ib_key,
            user_account: ctx.accounts.user_account.key(),
            pool_share_delta: pool_delta,
            volume_delta,
            tier: tier as u8,
            earned,
            parent: ib_parent,
            parent_override: paid_override,
            ib_unclaimed_after: unclaimed_after,
            ts: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }

    /// Claim accrued rebates. **No approval, no schedule, no counterparty.**
    ///
    /// CPIs into `solfx-core::pay_referral` signed by this programme's config PDA. Core
    /// checks only that the signature is the authority its admin registered and that the
    /// total claimed stays inside the referral pool; the policy that decided *this IB is
    /// owed this much* lives entirely here.
    pub fn claim(ctx: Context<Claim>) -> Result<()> {
        let amount = ctx.accounts.ib_account.unclaimed;
        require!(amount > 0, ReferralError::NothingToClaim);

        let bump = ctx.accounts.config.bump;
        let seeds: &[&[&[u8]]] = &[&[CONFIG_SEED, &[bump]]];

        solfx_core::cpi::pay_referral(
            CpiContext::new_with_signer(
                ctx.accounts.solfx_core.key(),
                solfx_core::cpi::accounts::PayReferral {
                    referral_authority: ctx.accounts.config.to_account_info(),
                    protocol: ctx.accounts.protocol.to_account_info(),
                    fee_vault: ctx.accounts.fee_vault.to_account_info(),
                    destination: ctx.accounts.destination.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info(),
                },
                seeds,
            ),
            amount,
        )?;

        let ib = &mut ctx.accounts.ib_account;
        ib.unclaimed = 0;
        ib.lifetime_claimed = ib
            .lifetime_claimed
            .checked_add(amount)
            .ok_or(ReferralError::MathOverflow)?;
        let lifetime = ib.lifetime_claimed;
        let ib_key = ib.key();

        let config = &mut ctx.accounts.config;
        config.total_claimed = config
            .total_claimed
            .checked_add(amount)
            .ok_or(ReferralError::MathOverflow)?;

        emit!(RebateClaimed {
            ib: ib_key,
            authority: ctx.accounts.authority.key(),
            amount,
            lifetime_claimed: lifetime,
            ts: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }
}

// --- accounts -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeReferral<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + ReferralConfig::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Box<Account<'info, ReferralConfig>>,

    /// `solfx-core`'s protocol account. Anchor verifies the owner, so the fee split read
    /// from it cannot be forged.
    pub protocol: Box<Account<'info, Protocol>>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SyncSplit<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.protocol == protocol.key() @ ReferralError::WrongProtocol,
    )]
    pub config: Box<Account<'info, ReferralConfig>>,
    pub protocol: Box<Account<'info, Protocol>>,
}

#[derive(Accounts)]
pub struct RegisterIb<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, ReferralConfig>>,

    #[account(
        init,
        payer = authority,
        space = 8 + IbAccount::INIT_SPACE,
        seeds = [IB_SEED, authority.key().as_ref()],
        bump,
    )]
    pub ib_account: Box<Account<'info, IbAccount>>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SyncTrader<'info> {
    /// Anyone. See `sync_trader`'s note on why this is permissionless.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, ReferralConfig>>,

    #[account(
        mut,
        seeds = [IB_SEED, ib_account.authority.as_ref()],
        bump = ib_account.bump,
    )]
    pub ib_account: Box<Account<'info, IbAccount>>,

    /// The sub-broker's parent, when there is one. Optional so a missing parent cannot block
    /// the child's own rebate.
    #[account(
        mut,
        seeds = [IB_SEED, parent_ib.authority.as_ref()],
        bump = parent_ib.bump,
    )]
    pub parent_ib: Option<Box<Account<'info, IbAccount>>>,

    /// The referred trader's `solfx-core` account. **Read-only, and owner-checked by
    /// Anchor** — the counters this programme derives everything from cannot be forged.
    pub user_account: Box<Account<'info, UserAccount>>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + TraderLink::INIT_SPACE,
        seeds = [LINK_SEED, ib_account.key().as_ref(), user_account.key().as_ref()],
        bump,
    )]
    pub link: Box<Account<'info, TraderLink>>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Claim<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.protocol == protocol.key() @ ReferralError::WrongProtocol,
    )]
    pub config: Box<Account<'info, ReferralConfig>>,

    #[account(
        mut,
        seeds = [IB_SEED, authority.key().as_ref()],
        bump = ib_account.bump,
        constraint = ib_account.authority == authority.key() @ ReferralError::NotAdmin,
    )]
    pub ib_account: Box<Account<'info, IbAccount>>,

    /// CHECK: validated by `solfx-core` inside the CPI.
    #[account(mut)]
    pub protocol: UncheckedAccount<'info>,
    /// CHECK: validated by `solfx-core` inside the CPI.
    #[account(mut)]
    pub fee_vault: UncheckedAccount<'info>,

    #[account(mut)]
    pub destination: Box<Account<'info, TokenAccount>>,

    pub solfx_core: Program<'info, SolfxCore>,
    pub token_program: Program<'info, Token>,
}
