use anchor_lang::prelude::*;

/// The liquidity vault's accounting. PDA at `["lp_pool"]`.
///
/// Created by `initialize_protocol` because that instruction is one-time and creating the
/// LP mint later would need a second init path. **The deposit and withdrawal logic is
/// Phase 5** — this struct exists now so the mint authority and vault addresses are fixed
/// from the start and never have to move.
///
/// The vault is the counterparty to every trade: it earns when traders lose and pays when
/// they win. That makes it a B-book, and the honest claim is not that there is no conflict
/// but that the pool is public, the price is Pyth's, and the rules are code (§ 1.3).
#[account]
#[derive(InitSpace)]
pub struct LpPool {
    /// `slpUSD`. Mint authority is this PDA.
    pub lp_mint: Pubkey,
    /// USDC token account owned by this PDA.
    pub lp_vault: Pubkey,

    /// Assets under management, at `QUOTE_PRECISION`. Invariant I2 requires this to equal
    /// the vault's token balance.
    pub aum: u64,
    /// `slpUSD` in circulation. Invariant I8: supply > 0 ⟺ aum > 0.
    pub lp_token_supply: u64,

    /// Seconds between `request_remove_liquidity` and `remove_liquidity`.
    ///
    /// Anti-JIT (threat T6). Without it an LP deposits ahead of a known trader loss and
    /// withdraws ahead of a known gain, harvesting the pool's edge without carrying its
    /// risk.
    pub withdrawal_cooldown_seconds: u32,
    /// Charged on exit, in bps.
    pub exit_fee_bps: u16,

    /// High-water mark for the LP performance fee (§ 8.1, stream 4).
    pub high_water_mark_per_share: u64,

    // --- appended in Phase 5 -----------------------------------------------------------
    // Taken from `_reserved`, which shrank from 128 bytes to 96. Existing offsets unchanged.
    /// Performance fee in bps, charged on the gain above the high-water mark. § 8.1 sets 10%.
    pub performance_fee_bps: u16,
    /// Shares with a withdrawal request outstanding.
    ///
    /// Tracked so the pool can report how much of its supply is queued to leave — a figure
    /// the frontend needs and a run is visible in.
    pub pending_withdrawal_shares: u64,
    pub total_performance_fees: u64,
    pub total_exit_fees: u64,
    /// Cumulative USDC deposited by providers, and cumulative USDC paid back out.
    ///
    /// Withdrawals are one of the few ways USDC crosses the protocol boundary, so invariant
    /// I7 — *every USDC the program holds is accounted for* — needs the outflow as a term.
    /// The pair also gives the LP dashboard lifetime inflow and outflow without walking the
    /// event stream.
    pub total_deposited: u64,
    pub total_withdrawn: u64,

    pub bump: u8,
    pub lp_vault_bump: u8,
    pub lp_mint_bump: u8,
    pub _reserved: [u8; 80],
}

/// A pending LP exit. PDA at `["lp_withdraw", authority]`.
///
/// # Why the cooldown is the whole defence
///
/// Threat T6 is the just-in-time attack: an LP deposits ahead of a trader loss the pool is
/// about to collect, and withdraws ahead of a gain it is about to pay. They capture the
/// pool's edge without ever carrying its risk, and the LPs who did carry it are diluted.
///
/// The defence is not the exit fee — 0.05% is trivially outrun by a large enough known move.
/// It is that **redemption is priced at NAV when it settles, not when it is requested.** An
/// LP who requests must then hold the position for the whole cooldown, exposed to everything
/// that happens in it. That converts the attack from "free" into "carry a day of risk", which
/// is precisely the risk they were trying to avoid.
///
/// It follows that this account must **not** snapshot a price. It records only *when* the
/// request may settle.
///
/// # Why the shares are not escrowed
///
/// Escrowing would need another token account and another set of transfers. It buys nothing:
/// the burn at settlement comes from the provider's own account, so a request for shares they
/// no longer hold simply fails. Moving tokens away cancels the request by making it
/// unsettleable, which is the correct outcome and costs no code.
#[account]
#[derive(InitSpace)]
pub struct LpWithdrawRequest {
    pub authority: Pubkey,
    /// Shares this request will burn.
    pub shares: u64,
    /// When the request was made. Kept for the audit trail and so a UI can show progress.
    pub requested_at: i64,
    /// Earliest timestamp at which `remove_liquidity` may settle.
    pub unlock_at: i64,
    pub bump: u8,
    pub _reserved: [u8; 32],
}

/// Bad-debt reserve. PDA at `["insurance_fund"]`.
///
/// Absorbs shortfalls from gap events before LPs feel them (§ 6.9). Launching with this
/// empty means the first gap hits LPs directly, and they do not come back.
#[account]
#[derive(InitSpace)]
pub struct InsuranceFund {
    /// Balance at `QUOTE_PRECISION`. Invariant I6 requires this to equal the vault balance.
    pub balance: u64,
    /// Target size — § 6.9 suggests 2% of maximum OI. Below 25% of this, markets go
    /// `ReduceOnly` (§ 7.2).
    pub target_balance: u64,
    /// Lifetime bad debt covered, for monitoring.
    pub total_bad_debt_covered: u64,
    pub bump: u8,
    pub vault_bump: u8,
    pub _reserved: [u8; 64],
}
