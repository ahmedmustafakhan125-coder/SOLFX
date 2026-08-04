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

    pub bump: u8,
    pub lp_vault_bump: u8,
    pub lp_mint_bump: u8,
    pub _reserved: [u8; 128],
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
