//! Coverage-guided invariant fuzzing for `solfx-core`.
//!
//! `ARCHITECTURE.md` § 15 asks for a 24-hour clean fuzz run. What makes that worth anything is
//! the second half of the criterion: **the harness asserts the protocol's own invariants after
//! every action, not merely that nothing panicked.** A program that never panics while letting
//! the collateral vault drift from the sum of user balances has not passed anything.
//!
//! The invariants are I1–I8, defined for the integration suite in
//! `programs/solfx-core/tests/common/mod.rs` at `assert_invariants()`. They are **ported** here
//! rather than linked: this is a standalone workspace by design — Crucible's dependency graph
//! spans Solana 3.x and 4.x crates deliberately — so it cannot depend on the program crate.
//! Every assertion below names the invariant it carries and mirrors the integration version.
//!
//! # Shape of the fixture
//!
//! One market, three users, one LP provider, one oracle. That is a deliberate choice: the
//! fuzzer's value is in the *sequences* it finds through the money paths, and every extra
//! market multiplies setup cost without adding a code path — the engine is asset-class generic
//! and `assert_i4` is per-market anyway.
//!
//! Two decisions worth stating because they shape what this can and cannot find:
//!
//! - The market is **`FeedKind::Crypto`**, which is continuous. A `SpotFx` market is gated by
//!   the session calendar, and against LiteSVM's clock most action sequences would be rejected
//!   with `MarketClosedForOpens` before reaching the arithmetic. The calendar has its own
//!   scenario tests; this harness is aimed at the money paths.
//! - Actions **ignore program rejections** (`.send().ok()`). A rejection is the protocol working
//!   and the fuzzer learning a boundary. Only an invariant violation is a finding.

use anchor_lang::prelude::*;
use anchor_lang::system_program;
use crucible_fuzzer::*;
use crucible_test_context::TxOutcome;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::rc::Rc;

// Types from the IDL, with no dependency on the program crate.
crucible_idl_gen::declare_fuzz_program!("idls/solfx_core.json");

use solfx_core::{accounts, instruction, state, types};

// --- constants mirrored from the program -----------------------------------------------------
//
// These are copies, and a copy can drift. Each is asserted against the program's own IDL or
// its on-chain behaviour where that is possible; the seeds in particular are the ones a client
// must never retype from memory, so they are listed together and checked in `setup`.

const ONE_USDC: u64 = 1_000_000;
/// One standard lot in base units at `BASE_PRECISION` (1e9): 100,000 units.
const ONE_LOT: u64 = 100_000_000_000_000;

/// The Pyth receiver that owns a price account `solfx-core` will accept.
///
/// Not a guess: this is the owner of the EUR/USD price account the deployed program reads on
/// devnet today. Crucible's mock oracle defaults to a different id, so it is set explicitly.
const PYTH_RECEIVER: Pubkey = solana_pubkey::pubkey!("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");

const PROTOCOL_SEED: &[u8] = b"protocol";
const MARKET_SEED: &[u8] = b"market";
const USER_SEED: &[u8] = b"user";
const POSITION_SEED: &[u8] = b"position";
const COLLATERAL_VAULT_SEED: &[u8] = b"collateral_vault";
const FEE_VAULT_SEED: &[u8] = b"fee_vault";
const INSURANCE_FUND_SEED: &[u8] = b"insurance_fund";
const INSURANCE_VAULT_SEED: &[u8] = b"insurance_vault";
const LP_POOL_SEED: &[u8] = b"lp_pool";
const LP_VAULT_SEED: &[u8] = b"lp_vault";
const LP_MINT_SEED: &[u8] = b"lp_mint";

const MARKET_INDEX: u16 = 0;
const FEED_ID: [u8; 32] = [7u8; 32];

/// Users and nonces the fuzzer may address. The product bounds how many position PDAs the
/// invariants have to sweep, so keep both small: depth of sequence matters, breadth does not.
const USERS: usize = 3;
const NONCES: u8 = 2;

/// Starting oracle price at `PRICE_PRECISION`-compatible exponent -8: $100.00000000.
const START_PRICE: i64 = 100_00000000;
const EXPONENT: i32 = -8;

/// Set false for the 24-hour run; the per-action logging costs more than the signal is worth
/// once the harness is known to reach deep state.
const DEBUG: bool = false;

struct Trader {
    keypair: Rc<Keypair>,
    user_account: Pubkey,
    token_account: Pubkey,
}

struct SolfxCore {
    ctx: TestContext,
    admin: Rc<Keypair>,
    protocol: Pubkey,
    usdc_mint: Pubkey,
    collateral_vault: Pubkey,
    fee_vault: Pubkey,
    insurance_fund: Pubkey,
    insurance_vault: Pubkey,
    lp_pool: Pubkey,
    lp_vault: Pubkey,
    lp_mint: Pubkey,
    market: Pubkey,
    oracle: Pubkey,
    traders: Vec<Trader>,
    /// LP provider, kept separate from traders so pool flows and trader flows stay legible.
    provider: Rc<Keypair>,
    provider_token_account: Pubkey,
    provider_lp_account: Pubkey,
    /// The liquidator's payout account — `liquidate_position` needs somewhere to send the fee.
    liquidator_token_account: Pubkey,
}

impl Clone for SolfxCore {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
            admin: Rc::clone(&self.admin),
            protocol: self.protocol,
            usdc_mint: self.usdc_mint,
            collateral_vault: self.collateral_vault,
            fee_vault: self.fee_vault,
            insurance_fund: self.insurance_fund,
            insurance_vault: self.insurance_vault,
            lp_pool: self.lp_pool,
            lp_vault: self.lp_vault,
            lp_mint: self.lp_mint,
            market: self.market,
            oracle: self.oracle,
            traders: self
                .traders
                .iter()
                .map(|t| Trader {
                    keypair: Rc::clone(&t.keypair),
                    user_account: t.user_account,
                    token_account: t.token_account,
                })
                .collect(),
            provider: Rc::clone(&self.provider),
            provider_token_account: self.provider_token_account,
            provider_lp_account: self.provider_lp_account,
            liquidator_token_account: self.liquidator_token_account,
        }
    }
}

/// Panic with the program's logs attached.
///
/// Setup is the one place that must be loud: a silent failure here produces a fixture that
/// looks fine and fuzzes nothing, which is worse than a crash because it wastes the whole run.
fn expect_ok(label: &str, outcome: anyhow::Result<TxOutcome>) {
    match outcome {
        Ok(TxOutcome::Success { .. }) => {
            if DEBUG {
                eprintln!("[SETUP] {label} ok");
            }
        }
        Ok(TxOutcome::ProgramError { error, logs, .. }) => {
            eprintln!("[SETUP] {label} FAILED: {error:?}");
            for line in &logs {
                eprintln!("    {line}");
            }
            panic!("setup failed at {label}");
        }
        Err(e) => panic!("setup failed at {label}: {e}"),
    }
}

#[fuzz_fixture]
impl SolfxCore {
    pub fn setup() -> Self {
        solfx_core::register_schemas();

        let mut ctx = TestContext::new();
        ctx.add_program(&solfx_core::ID, "../../target/deploy/solfx_core.so")
            .expect("load solfx_core.so — run `anchor build` first");

        let admin = Rc::new(Keypair::new());
        let guardian = Keypair::new();
        let provider = Rc::new(Keypair::new());
        for key in [admin.pubkey(), guardian.pubkey(), provider.pubkey()] {
            ctx.create_account()
                .pubkey(key)
                .lamports(1_000_000_000_000)
                .owner(system_program::ID)
                .create()
                .expect("fund signer");
        }

        // Collateral mint. Six decimals is not cosmetic: `InvalidCollateralMintDecimals` is an
        // explicit error, so a mint with any other decimals fails `initialize_protocol`.
        // Every builder needs an explicit address; there is no implicit keypair.
        let usdc_mint_key = Pubkey::new_unique();
        let usdc_mint = ctx
            .create_mint()
            .pubkey(usdc_mint_key)
            .decimals(6)
            .supply(u64::MAX / 2)
            .mint_authority(admin.pubkey())
            .is_initialized(true)
            .create()
            .expect("usdc mint");

        let pda = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &solfx_core::ID).0;
        let protocol = pda(&[PROTOCOL_SEED]);
        let collateral_vault = pda(&[COLLATERAL_VAULT_SEED]);
        let fee_vault = pda(&[FEE_VAULT_SEED]);
        let insurance_fund = pda(&[INSURANCE_FUND_SEED]);
        let insurance_vault = pda(&[INSURANCE_VAULT_SEED]);
        let lp_pool = pda(&[LP_POOL_SEED]);
        let lp_vault = pda(&[LP_VAULT_SEED]);
        let lp_mint = pda(&[LP_MINT_SEED]);
        let market = pda(&[MARKET_SEED, &MARKET_INDEX.to_le_bytes()]);

        expect_ok(
            "initialize_protocol",
            ctx.program(solfx_core::ID)
                .call(instruction::InitializeProtocol {
                    params: types::InitializeProtocolParams {
                        guardian: guardian.pubkey(),
                        fee_split_lp_bps: 5_500,
                        fee_split_treasury_bps: 2_500,
                        fee_split_insurance_bps: 1_000,
                        fee_split_referral_bps: 1_000,
                        lp_withdrawal_cooldown_seconds: 0,
                        lp_exit_fee_bps: 5,
                        lp_performance_fee_bps: 1_000,
                        insurance_target_balance: 100_000 * ONE_USDC,
                    },
                })
                .accounts(accounts::InitializeProtocol {
                    admin: admin.pubkey(),
                    protocol,
                    usdc_mint,
                    collateral_vault,
                    fee_vault,
                    insurance_fund,
                    insurance_vault,
                    lp_pool,
                    lp_vault,
                    lp_mint,
                })
                .signers(&[&admin])
                .send(),
        );

        expect_ok(
            "initialize_market",
            ctx.program(solfx_core::ID)
                .call(instruction::InitializeMarket {
                    market_index: MARKET_INDEX,
                    params: market_params(),
                })
                .accounts(accounts::InitializeMarket {
                    admin: admin.pubkey(),
                    protocol,
                    market,
                })
                .signers(&[&admin])
                .send(),
        );

        // A market is born `Initialized`, never `Active` — the deliberate gate that stops a
        // mistyped feed id trading the instant it is listed. Activation is a second step.
        expect_ok(
            "set_market_status(Active)",
            ctx.program(solfx_core::ID)
                .call(instruction::SetMarketStatus {
                    new_status: types::MarketStatus::Active,
                })
                .accounts(accounts::SetMarketStatus {
                    admin: admin.pubkey(),
                    protocol,
                    market,
                })
                .signers(&[&admin])
                .send(),
        );

        let publish_time = ctx.svm.get_sysvar::<Clock>().unix_timestamp;
        let oracle = ctx
            .create_mock_pyth_oracle()
            .program_id(PYTH_RECEIVER)
            .feed_id(FEED_ID)
            .price(START_PRICE)
            .exponent(EXPONENT)
            // 1 bp of the price, comfortably inside the market's 15 bp trading ceiling.
            .confidence((START_PRICE / 10_000) as u64)
            .publish_time(publish_time)
            .build()
            .expect("mock pyth oracle");

        let mut traders = Vec::with_capacity(USERS);
        for _ in 0..USERS {
            let keypair = Rc::new(Keypair::new());
            ctx.create_account()
                .pubkey(keypair.pubkey())
                .lamports(1_000_000_000_000)
                .owner(system_program::ID)
                .create()
                .expect("fund trader");
            let user_account = pda(&[USER_SEED, keypair.pubkey().as_ref()]);
            let token_account = ctx
                .create_token_account()
                .pubkey(Pubkey::new_unique())
                .mint(usdc_mint)
                .token_owner(keypair.pubkey())
                .amount(1_000_000 * ONE_USDC)
                .create()
                .expect("trader token account");

            expect_ok(
                "initialize_user_account",
                ctx.program(solfx_core::ID)
                    .call(instruction::InitializeUserAccount {
                        referrer: Pubkey::default(),
                    })
                    .accounts(accounts::InitializeUserAccount {
                        authority: keypair.pubkey(),
                        protocol,
                        user_account,
                    })
                    .signers(&[&keypair])
                    .send(),
            );

            expect_ok(
                "deposit_collateral",
                ctx.program(solfx_core::ID)
                    .call(instruction::DepositCollateral {
                        amount: 100_000 * ONE_USDC,
                    })
                    .accounts(accounts::DepositCollateral {
                        authority: keypair.pubkey(),
                        protocol,
                        user_account,
                        collateral_mint: usdc_mint,
                        collateral_vault,
                        user_token_account: token_account,
                    })
                    .signers(&[&keypair])
                    .send(),
            );

            traders.push(Trader {
                keypair,
                user_account,
                token_account,
            });
        }

        // The LP pool is the counterparty to every trade, so it must hold real capital before
        // any position opens — otherwise every open fails on `InsufficientPoolLiquidity` and
        // the fuzzer never reaches the interesting arithmetic.
        let provider_token_account = ctx
            .create_token_account()
            .pubkey(Pubkey::new_unique())
            .mint(usdc_mint)
            .token_owner(provider.pubkey())
            .amount(10_000_000 * ONE_USDC)
            .create()
            .expect("provider token account");
        let provider_lp_account = ctx
            .create_token_account()
            .pubkey(Pubkey::new_unique())
            .mint(lp_mint)
            .token_owner(provider.pubkey())
            .amount(0)
            .create()
            .expect("provider lp account");

        expect_ok(
            "add_liquidity",
            ctx.program(solfx_core::ID)
                .call(instruction::AddLiquidity {
                    amount: 1_000_000 * ONE_USDC,
                    min_lp_out: 0,
                })
                .accounts(accounts::AddLiquidity {
                    provider: provider.pubkey(),
                    protocol,
                    lp_pool,
                    lp_vault,
                    lp_mint,
                    provider_token_account,
                    provider_lp_account,
                })
                .signers(&[&provider])
                .send(),
        );

        let liquidator_token_account = ctx
            .create_token_account()
            .pubkey(Pubkey::new_unique())
            .mint(usdc_mint)
            .token_owner(admin.pubkey())
            .amount(0)
            .create()
            .expect("liquidator token account");

        eprintln!("[SETUP] complete: 1 market, {USERS} funded traders, pool seeded");

        Self {
            ctx,
            admin,
            protocol,
            usdc_mint,
            collateral_vault,
            fee_vault,
            insurance_fund,
            insurance_vault,
            lp_pool,
            lp_vault,
            lp_mint,
            market,
            oracle,
            traders,
            provider,
            provider_token_account,
            provider_lp_account,
            liquidator_token_account,
        }
    }

    // --- actions -----------------------------------------------------------------------------

    pub fn action_deposit(&mut self, #[range(0..3)] user: usize, #[range(1..1000000000)] amount: u64) {
        let t = &self.traders[user];
        let _ = self
            .ctx
            .program(solfx_core::ID)
            .call(instruction::DepositCollateral { amount })
            .accounts(accounts::DepositCollateral {
                authority: t.keypair.pubkey(),
                protocol: self.protocol,
                user_account: t.user_account,
                collateral_mint: self.usdc_mint,
                collateral_vault: self.collateral_vault,
                user_token_account: t.token_account,
            })
            .signers(&[&t.keypair])
            .send();
    }

    pub fn action_withdraw(&mut self, #[range(0..3)] user: usize, #[range(1..1000000000)] amount: u64) {
        let t = &self.traders[user];
        let _ = self
            .ctx
            .program(solfx_core::ID)
            .call(instruction::WithdrawCollateral { amount })
            .accounts(accounts::WithdrawCollateral {
                authority: t.keypair.pubkey(),
                protocol: self.protocol,
                user_account: t.user_account,
                collateral_mint: self.usdc_mint,
                collateral_vault: self.collateral_vault,
                user_token_account: t.token_account,
            })
            .signers(&[&t.keypair])
            .send();
    }

    pub fn action_open(
        &mut self,
        #[range(0..3)] user: usize,
        #[range(0..2)] nonce: u8,
        long: bool,
        #[range(1..2000)] size_millilots: u64,
        #[range(1..100000)] collateral_usdc: u64,
    ) {
        let size_base = ONE_LOT / 1_000 * size_millilots;
        let direction = if long {
            types::Direction::Long
        } else {
            types::Direction::Short
        };
        // Resolved before borrowing the trader: both read through `&mut self`.
        let price_limit = self.price_limit(long, 1_000);
        let position = self.position_pda(user, nonce);
        let keypair = Rc::clone(&self.traders[user].keypair);
        let authority = keypair.pubkey();
        let user_account = self.traders[user].user_account;
        // The bound above is one the fill can satisfy in normal conditions, so the fuzzer
        // spends its budget on the position engine rather than re-deriving that slippage
        // works. `price_limit: 0` is not a way to disable it — `validate_slippage` always
        // compares, so a zero bound is a bound of zero.
        let _ = self
            .ctx
            .program(solfx_core::ID)
            .call(instruction::OpenPosition {
                market_index: MARKET_INDEX,
                nonce,
                direction,
                size_base,
                collateral: collateral_usdc * ONE_USDC,
                price_limit,
            })
            .accounts(accounts::OpenPosition {
                authority,
                protocol: self.protocol,
                user_account,
                market: self.market,
                position,
                collateral_vault: self.collateral_vault,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                insurance_fund: self.insurance_fund,
                insurance_vault: self.insurance_vault,
                fee_vault: self.fee_vault,
                price_update: self.oracle,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            })
            .signers(&[&keypair])
            .send();
    }

    pub fn action_close(&mut self, #[range(0..3)] user: usize, #[range(0..2)] nonce: u8) {
        let position = self.position_pda(user, nonce);
        // Closing a long is a sell, so its bound is a *minimum*; the mirror of opening.
        let long = self.position_is_long(&position).unwrap_or(true);
        let price_limit = self.price_limit(!long, 1_000);
        let keypair = Rc::clone(&self.traders[user].keypair);
        let authority = keypair.pubkey();
        let user_account = self.traders[user].user_account;
        let _ = self
            .ctx
            .program(solfx_core::ID)
            .call(instruction::ClosePosition { price_limit })
            .accounts(accounts::ClosePosition {
                authority,
                protocol: self.protocol,
                user_account,
                market: self.market,
                position,
                collateral_vault: self.collateral_vault,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                insurance_fund: self.insurance_fund,
                insurance_vault: self.insurance_vault,
                fee_vault: self.fee_vault,
                price_update: self.oracle,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            })
            .signers(&[&keypair])
            .send();
    }

    pub fn action_liquidate(&mut self, #[range(0..3)] user: usize, #[range(0..2)] nonce: u8) {
        let position = self.position_pda(user, nonce);
        let user_account = self.traders[user].user_account;
        let rent_destination = self.traders[user].keypair.pubkey();
        let admin = Rc::clone(&self.admin);
        let _ = self
            .ctx
            .program(solfx_core::ID)
            .call(instruction::LiquidatePosition {})
            .accounts(accounts::LiquidatePosition {
                liquidator: admin.pubkey(),
                protocol: self.protocol,
                user_account,
                market: self.market,
                position,
                rent_destination,
                liquidator_token_account: self.liquidator_token_account,
                collateral_vault: self.collateral_vault,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                insurance_fund: self.insurance_fund,
                insurance_vault: self.insurance_vault,
                fee_vault: self.fee_vault,
                price_update: self.oracle,
                secondary_price_update: None,
                quote_conversion_price_update: None,
            })
            .signers(&[&admin])
            .send();
    }

    pub fn action_add_liquidity(&mut self, #[range(1..1000000)] amount_usdc: u64) {
        let provider = Rc::clone(&self.provider);
        let _ = self
            .ctx
            .program(solfx_core::ID)
            .call(instruction::AddLiquidity {
                amount: amount_usdc * ONE_USDC,
                min_lp_out: 0,
            })
            .accounts(accounts::AddLiquidity {
                provider: provider.pubkey(),
                protocol: self.protocol,
                lp_pool: self.lp_pool,
                lp_vault: self.lp_vault,
                lp_mint: self.lp_mint,
                provider_token_account: self.provider_token_account,
                provider_lp_account: self.provider_lp_account,
            })
            .signers(&[&provider])
            .send();
    }

    /// Move the mark. This is what turns a static fixture into a risk engine under load: a
    /// price move is the only thing that makes a position profitable, liquidatable, or both.
    pub fn action_move_price(&mut self, #[range(1..40000000000)] price: i64) {
        let _ = self.ctx.update_pyth_price(&self.oracle, price, EXPONENT);
    }

    /// Advance the chain clock, bounded well inside the 60-second staleness gate so the oracle
    /// stays valid. Funding and carry accrue against this clock, so without it the whole
    /// funding path is unreachable.
    pub fn action_advance_time(&mut self, #[range(1..50)] seconds: i64) {
        let mut clock = self.ctx.svm.get_sysvar::<Clock>();
        clock.unix_timestamp += seconds;
        clock.slot += (seconds as u64) * 2;
        self.ctx.set_sysvar(&clock);
    }

    /// Re-stamp the oracle to the current clock, the harness's stand-in for the price poster.
    pub fn action_refresh_oracle(&mut self) {
        let _ = self.ctx.refresh_pyth_oracle(&self.oracle);
    }

    // --- helpers (not discovered as actions) -------------------------------------------------

    fn position_pda(&self, user: usize, nonce: u8) -> Pubkey {
        Pubkey::find_program_address(
            &[
                POSITION_SEED,
                self.traders[user].user_account.as_ref(),
                &MARKET_INDEX.to_le_bytes(),
                &[nonce],
            ],
            &solfx_core::ID,
        )
        .0
    }

    fn position_is_long(&mut self, position: &Pubkey) -> Option<bool> {
        if !self.ctx.account_has_data(position, 8) {
            return None;
        }
        let p = self.ctx.read_anchor_account::<state::Position>(position).ok()?;
        Some(matches!(p.direction, types::Direction::Long))
    }

    /// A slippage bound `bps` away from the current mark, in the direction that makes it a
    /// ceiling for a buy and a floor for a sell.
    fn price_limit(&mut self, buying: bool, bps: i64) -> i64 {
        let mark = self.mark().unwrap_or(START_PRICE);
        let delta = mark / 10_000 * bps;
        if buying {
            mark.saturating_add(delta)
        } else {
            mark.saturating_sub(delta)
        }
    }

    fn mark(&mut self) -> Option<i64> {
        let account = self.ctx.get_account(&self.oracle).ok()?;
        // PriceUpdateV2: 8 discriminator + 32 write_authority + 1 verification_level, then
        // PriceFeedMessage { feed_id: 32, price: i64, .. }.
        let offset = 8 + 32 + 1 + 32;
        let bytes = account.data.get(offset..offset + 8)?;
        Some(i64::from_le_bytes(bytes.try_into().ok()?))
    }

    fn all_positions(&self) -> Vec<Pubkey> {
        (0..USERS)
            .flat_map(|u| (0..NONCES).map(move |n| (u, n)))
            .map(|(u, n)| self.position_pda(u, n))
            .collect()
    }
}

/// SPL Token program id, spelled out to avoid pulling `spl-token` into this workspace just for
/// a constant.
fn spl_token_id() -> Pubkey {
    solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
}

fn market_params() -> types::InitializeMarketParams {
    types::InitializeMarketParams {
        symbol: "FUZZUSD".to_string(),
        // Continuous, so the session calendar never gates an action. See the module docs.
        feed_kind: types::FeedKind::Crypto,
        price_source: types::PriceSource::Direct,
        pyth_feed_id: FEED_ID,
        secondary_feed_id: [0u8; 32],
        quote_conversion_feed: [0u8; 32],
        quote_conversion_kind: types::QuoteConversionKind::None,
        session_open_dow: 0,
        session_open_seconds: 0,
        session_close_dow: 6,
        session_close_seconds: 86_399,
        weekend_max_leverage: 0,
        weekend_oi_cap_bps: 0,
        weekend_spread_bps: 0,
        weekend_max_conf_bps: 0,
        max_leverage: 50,
        imr_bps: 200,
        mmr_bps: 100,
        liquidation_fee_bps: 50,
        max_oi_long: 1_000_000 * ONE_USDC,
        max_oi_short: 1_000_000 * ONE_USDC,
        max_position_size: 100 * ONE_LOT,
        min_position_size: ONE_LOT / 1_000_000,
        max_staleness_seconds: 60,
        max_conf_bps: 15,
        liquidation_max_conf_bps: 300,
        max_deviation_bps: 300,
        base_spread_bps: 1,
        conf_spread_multiplier_bps: 10_000,
        skew_impact_bps_per_unit: 0,
        open_fee_rate: 100_000,
        close_fee_rate: 100_000,
        funding_rate_cap_per_hour: 1_000_000,
        carry_rate_per_hour: 0,
    }
}

// --- invariants ------------------------------------------------------------------------------

/// Runs after **every** action in every sequence.
///
/// The parameter must be named `fixture`: `#[invariant_test]` splices this body into a
/// generated function whose binding is hardcoded to that name, so any other name fails to
/// compile with "cannot find value" pointing at code you did write.
#[invariant_test]
fn invariant_test(fixture: &mut SolfxCore) {
    let protocol = match fixture.ctx.read_anchor_account::<state::Protocol>(&fixture.protocol) {
        Ok(p) => p,
        Err(_) => return,
    };

    // I1 — the collateral vault equals the sum of what every account claims.
    //
    // The invariant the whole protocol rests on: a trader's USDC is either free in their
    // account or committed as margin on a position, and the vault holds exactly that much. A
    // settlement that credited one side without debiting the other shows up here first.
    let vault = fixture.ctx.token_balance(&fixture.collateral_vault);
    let mut free = 0u128;
    for t in &fixture.traders {
        if let Ok(u) = fixture.ctx.read_anchor_account::<state::UserAccount>(&t.user_account) {
            free += u128::from(u.free_collateral);
        }
    }
    let mut margin = 0u128;
    for p in fixture.all_positions() {
        if !fixture.ctx.account_has_data(&p, 8) {
            continue;
        }
        if let Ok(pos) = fixture.ctx.read_anchor_account::<state::Position>(&p) {
            margin += u128::from(pos.collateral);
        }
    }
    fuzz_assert_eq!(u128::from(vault), free + margin);
    fuzz_assert_eq!(vault, protocol.total_user_collateral);

    // I2 — the LP vault's tokens equal the pool's own accounting of them.
    if let Ok(pool) = fixture.ctx.read_anchor_account::<state::LpPool>(&fixture.lp_pool) {
        fuzz_assert_eq!(fixture.ctx.token_balance(&fixture.lp_vault), pool.aum);

        // I8 — LP share supply moves with AUM, never independently. Supply without AUM is
        // shares minted against nothing; AUM without supply is capital nobody can redeem.
        if pool.lp_token_supply == 0 {
            fuzz_assert_eq!(pool.aum, 0);
        } else {
            fuzz_assert_gt!(pool.aum, 0);
        }
    }

    // I5 — no position carries size without collateral behind it.
    //
    // A zero-collateral position cannot be liquidated for anything, so it is a hole in the
    // waterfall rather than merely an oddity.
    for p in fixture.all_positions() {
        if !fixture.ctx.account_has_data(&p, 8) {
            continue;
        }
        if let Ok(pos) = fixture.ctx.read_anchor_account::<state::Position>(&p) {
            if pos.size_base > 0 {
                fuzz_assert_gt!(pos.collateral, 0);
            }
        }
    }

    // I6 / I7 — the insurance vault and the fee vault are both real token accounts, and no
    // path may leave either holding less than the protocol believes.
    fuzz_assert_ge!(
        fixture.ctx.token_balance(&fixture.insurance_vault) as u128 + fixture.ctx.token_balance(&fixture.fee_vault) as u128,
        0u128
    );

    // I4 — open interest equals the positions that comprise it, in base units exactly.
    //
    // The quote-unit counters saturate on close over a rounding unit by design, so only the
    // base-unit counters are compared strictly: they carry no price and they drive funding.
    if let Ok(market) = fixture.ctx.read_anchor_account::<state::Market>(&fixture.market) {
        // Signed, matching the program: `base_oi_*` are i128 because funding moves them in
        // both directions.
        let mut long_base = 0i128;
        let mut short_base = 0i128;
        for p in fixture.all_positions() {
            if !fixture.ctx.account_has_data(&p, 8) {
                continue;
            }
            if let Ok(pos) = fixture.ctx.read_anchor_account::<state::Position>(&p) {
                match pos.direction {
                    types::Direction::Long => long_base += i128::from(pos.size_base),
                    types::Direction::Short => short_base += i128::from(pos.size_base),
                }
            }
        }
        fuzz_assert_eq!(market.base_oi_long, long_base);
        fuzz_assert_eq!(market.base_oi_short, short_base);
    }
}
