# Test-Case Catalogue

One file per phase, generated from the actual test code (names, doc comments and expected
errors are extracted from source, so the catalogue cannot drift from what runs). **452
executable tests** plus Phase 0's measurement criteria.

| Phase | Tests | Catalogue | What it proves |
|---|---:|---|---|
| 0 — Oracle feasibility | 8 criteria + 5 self-checks | [phase-0-tests.md](phase-0-tests.md) | which feeds are real, listable, and at what leverage |
| 1 — Math engine | 166 | [phase-1-tests.md](phase-1-tests.md) | every formula, every rounding direction; **a round trip always loses** |
| 2 — Vault, markets, oracle | 131 | [phase-2-tests.md](phase-2-tests.md) | the six oracle gates; listing without redeploy; I1 three ways; the authority model |
| 3 — Position engine | 31 | [phase-3-tests.md](phase-3-tests.md) | the full lifecycle on every market shape; conservation before tokens move |
| 4 — Risk engine | 52 | [phase-4-tests.md](phase-4-tests.md) | liquidation waterfall; both regimes; **the CHF-depeg replay** |
| 5 — LP vault | 42 | [phase-5-tests.md](phase-5-tests.md) | share maths never dilutes; **the JIT attack fails**; I2 + I8 |
| 6 — IB programme | 30 | [phase-6-tests.md](phase-6-tests.md) | clients cannot be reassigned; syncing cannot double-credit; claims bounded by the pool |

## Invariants asserted after every instruction (§ 12.3)

| | Statement |
|---|---|
| **I1** | `CollateralVault == Σ free_collateral + Σ position margin + funding_balance` — checked **three ways** (token balance, account sum, protocol counter) |
| **I2** | `LpVault.amount == LpPool.aum` |
| **I3** | funding nets to zero and never touches LP capital — *structural* (funding moves no tokens) |
| **I4** | market OI (quote and base) == Σ over live positions |
| **I5** | no position with size > 0 and collateral == 0 |
| **I6** | `InsuranceVault.amount == InsuranceFund.balance` |
| **I7** | Σ(all vaults) == trader in − trader out + LP in − LP out + insurance in − liquidator rewards − treasury sweeps − referral claims |
| **I8** | LP `supply > 0 ⟺ aum > 0` (no stranded, unclaimable USDC) |

## Running everything

```bash
anchor build                       # required first — integration tests load the real .so
cargo test --workspace             # all 454
PROPTEST_CASES=100000 cargo test --release -p solfx-math --test properties   # deep run
cargo test -p solfx-core --test compute_budget -- --nocapture                # CU table
```
