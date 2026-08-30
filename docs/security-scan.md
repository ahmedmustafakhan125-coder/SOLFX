# Ackee detector scan — first run

`scripts/security-scan.mjs` drives Ackee Blockchain Security's Solana detectors headlessly.
Run 30 Aug 2026 against the whole workspace.

```
Rust files      : 77
Anchor programs : 51
Files w/ issues : 30
Total issues    : 672
```

**672 is not 672 vulnerabilities.** Triaged below. Nothing actionable survived, and that is a
statement about these detectors on this codebase, not a clean bill of health — see the
caveat at the end.

| Detector | Count | Verdict |
|---|---:|---|
| `UNSAFE_ARITHMETIC` | 642 | false positive |
| `MISSING_SIGNER` | 29 | 28 unresolved, 1 deliberate |
| `INEFFICIENT_SYSVAR_ACCOUNT` | 1 | valid, negligible |

## `UNSAFE_ARITHMETIC` — 642

Roughly 470 land in `programs/solfx-core/tests/**`, which opens with an explicit
`#![allow(clippy::arithmetic_side_effects)]` because a panic in a test is the reporting
mechanism. Test arithmetic is meant to be plain.

The rest are in program source, and the detector is flagging two things it cannot see:

- **Crate-level lint configuration.** The workspace *denies* `arithmetic_side_effects`,
  `integer_division`, `unwrap_used`, `expect_used`, `panic` and the lossy casts, and clippy
  passes clean. `solfx-core` relaxes exactly four at crate level because Anchor's derive
  macros generate code that trips them — which is why 28 of these sit in `lib.rs`, the file
  that is almost entirely macro expansion.
- **Guarded division.** `crates/solfx-math/src/fixed.rs:36`, `:46` and `:66` are each `n / d`
  three lines below an explicit `if d == 0 { return Err(DivideByZero) }`, and
  `mul_div_floor_signed` additionally rejects `d < 0`. Every multiplication in that file is
  `checked_mul`, every addition `checked_add`, every subtraction `checked_sub`. The detector
  reports `/` and `%` as unchecked without tracking the guard.

## `MISSING_SIGNER` — 29

**28** read *"Context 'X' definition **not found**"* — `InitializeMarket`, `LiquidatePosition`,
`AdminOnly` and so on. Every one of those structs is defined under `instructions/**` and
reaches `lib.rs` through the glob re-export the `#[derive(Accounts)]` macro requires. The
detector cannot follow that, so it is reporting a resolution failure rather than a finding.
`AdminMarket` carries `admin: Signer` with `has_one = admin @ NotAdmin`; `OpenPosition`
carries `authority: Signer` with `has_one = authority @ AuthorityMismatch`.

**1** resolved properly: `solfx-referral/src/lib.rs:241`, `SyncSplit` has no signer. It is
correct that there is none, and it is deliberate — the handler copies one public number from
a verified account, and the code says so:

> Permissionless: it copies a public number from a verified account, so there is nothing to
> gain by calling it and something to lose by being unable to.

## `INEFFICIENT_SYSVAR_ACCOUNT` — 1

`initialize_protocol.rs:130` uses `Sysvar<'info, Rent>` where `Rent::get()?` would avoid
passing the account. Valid, and worth nothing: it is a one-time initialisation instruction,
not a hot path, and `programs/` is under a change freeze.

## What this does and does not mean

It means these detectors found nothing this codebase does not already handle by construction,
and the two categories they did fire on are ones the workspace lints already enforce more
strictly than the detector does.

It does not mean the protocol is audited. **SolFX has had zero external audits**, Trident
fuzzing is a Phase 9 exit criterion that has not run, and a static scanner that reports no
actionable findings has told you about the bug classes it models — not about the ones it
does not.
