//! Housekeeping: a settled mandate's SOL comes back, and the protocol's keys can be rotated.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]

#[path = "../../solfx-core/tests/common/mod.rs"]
mod common;
#[allow(dead_code)]
mod support;

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use common::*;
use noxfunds::instructions::investor::MandateRules;
use noxfunds::state::{MandateState, NoxConfig};
use solana_signer::Signer;

fn nox_so_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/noxfunds.so")
}
fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[noxfunds::constants::CONFIG_SEED], &noxfunds::ID).0
}
fn mandate_pda(investor: &Pubkey, trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            noxfunds::constants::MANDATE_SEED,
            investor.as_ref(),
            trader.as_ref(),
            &[0],
        ],
        &noxfunds::ID,
    )
    .0
}
fn profile_pda(trader: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::TRADER_SEED, trader.as_ref()],
        &noxfunds::ID,
    )
    .0
}
fn signer_pda(mandate: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[noxfunds::constants::MANDATE_SIGNER_SEED, mandate.as_ref()],
        &noxfunds::ID,
    )
    .0
}

fn refused(r: TestResult, code: &str) {
    let err = r.expect_err("expected a refusal");
    assert!(err.contains(code), "refused, but not for {code}:\n{err}");
}

struct Nox {
    env: Env,
    investor: solana_keypair::Keypair,
    trader: solana_keypair::Keypair,
    mandate: Pubkey,
    signer: Pubkey,
}

/// A funded mandate whose signer holds `signer_lamports`, as a trader's "Prepare it" leaves it.
fn setup(signer_lamports: u64) -> Nox {
    let mut env = Env::new();
    let so = std::fs::read(nox_so_path()).expect("build noxfunds.so first");
    env.svm.add_program(noxfunds::ID, &so).unwrap();
    let upgrade_authority = env.admin.pubkey();
    support::set_upgrade_authority(&mut env, Some(upgrade_authority));
    env.init_protocol();
    env.list_and_activate(0, &MarketSpec::eur_usd());

    let investor = solana_keypair::Keypair::new();
    let trader = solana_keypair::Keypair::new();
    for k in [&investor, &trader] {
        env.svm.airdrop(&k.pubkey(), 100 * 1_000_000_000).unwrap();
    }
    let admin = env.admin.insecure_clone();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeConfig {
            program: noxfunds::ID,
            program_data: support::program_data_address(),
            admin: admin.pubkey(),
            config: config_pda(),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeConfig {
            guardian: env.guardian.pubkey(),
            treasury: admin.pubkey(),
            usdc_mint: env.usdc_mint,
        }
        .data(),
    };
    env.send(ix, &[&admin]).unwrap();
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::InitializeTraderProfile {
            payer: admin.pubkey(),
            authority: trader.pubkey(),
            profile: profile_pda(&trader.pubkey()),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::InitializeTraderProfile {}.data(),
    };
    env.send(ix, &[&admin]).unwrap();

    let mandate = mandate_pda(&investor.pubkey(), &trader.pubkey());
    let signer = signer_pda(&mandate);
    let investor_token = Pubkey::new_unique();
    env.write_token_account(
        investor_token,
        env.usdc_mint,
        investor.pubkey(),
        support::INVESTOR_START,
    );
    let ix = Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::FundMandate {
            trader_profile: profile_pda(&trader.pubkey()),
            usdc_mint: env.usdc_mint,
            investor_token,
            mandate_vault: support::vault_pda(&mandate),
            token_program: spl_token::ID,
            investor: investor.pubkey(),
            config: config_pda(),
            trader: trader.pubkey(),
            mandate,
            mandate_signer: signer,
            solfx_user_account: Env::user_pda(&signer),
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: noxfunds::instruction::FundMandate {
            seq: 0,
            principal: 1_000 * ONE_USDC,
            rules: MandateRules {
                max_trade_notional: 2_000 * ONE_USDC,
                max_total_notional: 6_000 * ONE_USDC,
                max_drawdown_bps: 1_000,
                max_daily_loss_bps: 1_000,
                max_risk_per_trade_bps: 100,
                max_stop_distance_bps: 200,
                max_concurrent_positions: 5,
                allowed_markets: 1,
                min_hold_slots: 0,
            },
        }
        .data(),
    };
    let inv = investor.insecure_clone();
    env.send(ix, &[&inv, &trader]).unwrap();
    if signer_lamports > 0 {
        env.svm.airdrop(&signer, signer_lamports).unwrap();
    }
    Nox {
        env,
        investor,
        trader,
        mandate,
        signer,
    }
}

impl Nox {
    fn mandate(&self) -> noxfunds::state::Mandate {
        self.env.read(&self.mandate)
    }
    fn config(&self) -> NoxConfig {
        self.env.read(&config_pda())
    }
    fn stranger(&mut self) -> solana_keypair::Keypair {
        let k = solana_keypair::Keypair::new();
        self.env.svm.airdrop(&k.pubkey(), 1_000_000_000).unwrap();
        k
    }

    /// Settle the mandate: the investor asks, a stranger claims. With nothing ever posted to
    /// SolFX it pays straight out of the vault (internal review R-7).
    fn settle(&mut self) {
        let investor = self.investor.insecure_clone();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::RequestSettlement {
                investor: investor.pubkey(),
                mandate: self.mandate,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::RequestSettlement {}.data(),
        };
        self.env.send(ix, &[&investor]).unwrap();

        let mint = self.env.usdc_mint;
        let (inv_t, tr_t, treas_t) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let (inv, tr, admin) = (
            self.investor.pubkey(),
            self.trader.pubkey(),
            self.env.admin.pubkey(),
        );
        self.env.write_token_account(inv_t, mint, inv, 0);
        self.env.write_token_account(tr_t, mint, tr, 0);
        self.env.write_token_account(treas_t, mint, admin, 0);
        let settler = self.stranger();
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::ClaimSettlement {
                trader_profile: profile_pda(&self.trader.pubkey()),
                settler: settler.pubkey(),
                config: config_pda(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                protocol: self.env.protocol,
                user_account: Env::user_pda(&self.signer),
                usdc_mint: mint,
                collateral_vault: self.env.collateral_vault,
                mandate_vault: support::vault_pda(&self.mandate),
                investor_token: inv_t,
                trader_token: tr_t,
                treasury_token: treas_t,
                token_program: spl_token::ID,
                solfx_core_program: solfx_core::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::ClaimSettlement {}.data(),
        };
        self.env.send(ix, &[&settler]).expect("settle");
        assert_eq!(self.mandate().state, MandateState::Settled);
    }

    fn sweep(&mut self, caller: &solana_keypair::Keypair, to: Pubkey) -> TestResult {
        let ix = Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::SweepMandateSigner {
                caller: caller.pubkey(),
                mandate: self.mandate,
                mandate_signer: self.signer,
                trader: to,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: noxfunds::instruction::SweepMandateSigner {}.data(),
        };
        let c = caller.insecure_clone();
        self.env.send(ix, &[&c])
    }

    fn admin_ix(&self, signer: &Pubkey, data: Vec<u8>) -> Instruction {
        Instruction {
            program_id: noxfunds::ID,
            accounts: noxfunds::accounts::AdminConfig {
                admin: *signer,
                config: config_pda(),
            }
            .to_account_metas(None),
            data,
        }
    }

    fn as_(&mut self, who: &solana_keypair::Keypair, ix: Instruction) -> TestResult {
        let w = who.insecure_clone();
        self.env.send(ix, &[&w])
    }
}

// --- the sweep ---------------------------------------------------------------------------------

/// **The 0.02 SOL that used to be lost.** A settled mandate's signer is emptied into the trader's
/// wallet, by anyone, and only into the trader's.
#[test]
fn a_settled_mandate_returns_its_signers_sol_to_the_trader() {
    let mut nox = setup(20_000_000);
    let stranger = nox.stranger();
    let trader = nox.trader.pubkey();

    refused(nox.sweep(&stranger, trader), "MandateNotSettled");
    nox.settle();

    // Nobody can point it anywhere else.
    let elsewhere = stranger.pubkey();
    refused(nox.sweep(&stranger, elsewhere), "NotTheTrader");

    let held = nox.env.sol_balance(&nox.signer);
    assert!(held > 0);
    let before = nox.env.sol_balance(&trader);
    nox.sweep(&stranger, trader).expect("anyone may sweep");
    assert_eq!(nox.env.sol_balance(&trader), before + held, "every lamport");
    assert_eq!(nox.env.sol_balance(&nox.signer), 0);

    // An empty signer has nothing to sweep, and says so rather than emitting a zero.
    refused(nox.sweep(&stranger, trader), "ZeroAmount");
}

// --- rotating the keys --------------------------------------------------------------------------

#[test]
fn only_the_admin_sets_the_guardian_and_the_treasury() {
    let mut nox = setup(0);
    let stranger = nox.stranger();
    let new_guardian = Pubkey::new_unique();
    let new_treasury = Pubkey::new_unique();

    let ix = nox.admin_ix(
        &stranger.pubkey(),
        noxfunds::instruction::SetGuardian {
            guardian: new_guardian,
        }
        .data(),
    );
    refused(nox.as_(&stranger, ix), "NotTheAdmin");

    let admin = nox.env.admin.insecure_clone();
    let ix = nox.admin_ix(
        &admin.pubkey(),
        noxfunds::instruction::SetGuardian {
            guardian: Pubkey::default(),
        }
        .data(),
    );
    refused(nox.as_(&admin, ix), "InvalidKey");

    let ix = nox.admin_ix(
        &admin.pubkey(),
        noxfunds::instruction::SetGuardian {
            guardian: new_guardian,
        }
        .data(),
    );
    nox.as_(&admin, ix).unwrap();
    let ix = nox.admin_ix(
        &admin.pubkey(),
        noxfunds::instruction::SetTreasury {
            treasury: new_treasury,
        }
        .data(),
    );
    nox.as_(&admin, ix).unwrap();

    let cfg = nox.config();
    assert_eq!(cfg.guardian, new_guardian);
    assert_eq!(cfg.treasury, new_treasury);
}

/// **Two steps, so a typo hands the protocol to nobody.** Nothing moves on the proposal; the
/// successor must sign; the old admin loses its power the moment they do.
#[test]
fn the_admin_moves_only_when_the_successor_accepts() {
    let mut nox = setup(0);
    let admin = nox.env.admin.insecure_clone();
    let successor = nox.stranger();
    let impostor = nox.stranger();

    let ix = nox.admin_ix(
        &admin.pubkey(),
        noxfunds::instruction::ProposeAdmin {
            new_admin: successor.pubkey(),
        }
        .data(),
    );
    nox.as_(&admin, ix).unwrap();
    assert_eq!(
        nox.config().admin,
        admin.pubkey(),
        "nothing moves on a proposal"
    );
    assert_eq!(nox.config().pending_admin, successor.pubkey());

    let accept = |who: &Pubkey| Instruction {
        program_id: noxfunds::ID,
        accounts: noxfunds::accounts::AcceptAdmin {
            new_admin: *who,
            config: config_pda(),
        }
        .to_account_metas(None),
        data: noxfunds::instruction::AcceptAdmin {}.data(),
    };
    refused(
        nox.as_(&impostor, accept(&impostor.pubkey())),
        "NotThePendingAdmin",
    );
    nox.as_(&successor, accept(&successor.pubkey())).unwrap();
    let cfg = nox.config();
    assert_eq!(cfg.admin, successor.pubkey());
    assert_eq!(
        cfg.pending_admin,
        Pubkey::default(),
        "the proposal is cleared"
    );

    // The old admin is now a stranger to the config.
    let ix = nox.admin_ix(
        &admin.pubkey(),
        noxfunds::instruction::SetTreasury {
            treasury: Pubkey::new_unique(),
        }
        .data(),
    );
    refused(nox.as_(&admin, ix), "NotTheAdmin");
    // And nobody can accept a proposal that no longer exists.
    refused(
        nox.as_(&successor, accept(&successor.pubkey())),
        "NotThePendingAdmin",
    );
}

#[test]
fn a_mistaken_proposal_is_withdrawn_by_proposing_again() {
    let mut nox = setup(0);
    let admin = nox.env.admin.insecure_clone();
    let wrong = Pubkey::new_unique();
    for new_admin in [wrong, Pubkey::default()] {
        let ix = nox.admin_ix(
            &admin.pubkey(),
            noxfunds::instruction::ProposeAdmin { new_admin }.data(),
        );
        nox.as_(&admin, ix).unwrap();
    }
    assert_eq!(nox.config().pending_admin, Pubkey::default());
    assert_eq!(nox.config().admin, admin.pubkey());
}
