//! # solfx-math
//!
//! Fixed-point financial primitives for the SolFX protocol.
//!
//! ## Why this is a standalone crate
//!
//! `ARCHITECTURE.md` § 11 places `math/` inside `programs/solfx-core`. It lives here instead,
//! with **zero dependencies**, for three reasons:
//!
//! 1. `cargo test` runs in seconds without the Solana BPF toolchain, so the property tests
//!    can run millions of cases in CI.
//! 2. The "100% coverage on `math/`" exit criterion is measurable in isolation.
//! 3. It enforces § 5.6's generic-engine constraint by construction: this crate cannot
//!    reference a `Market` or a `Position`, so no FX-specific assumption can leak into it.
//!
//! `solfx-core` depends on it and wraps these functions in Anchor instructions.
//!
//! ## Guarantees
//!
//! - **No floats.** Float non-determinism across validators is consensus-breaking (ADR-005).
//! - **No panics.** Every fallible operation returns [`MathError`]. No `unwrap`, no indexing.
//! - **No silent overflow.** All intermediates are `u128`/`i128`; narrowing is checked.
//! - **Explicit rounding.** Every division names its direction. See [`fixed`] for the rule.
//!
//! ## Reading order
//!
//! [`fixed`] first — it defines the rounding discipline everything else obeys. Then
//! [`pnl`], [`margin`], [`pricing`], [`fees`], [`funding`].

#![cfg_attr(not(test), forbid(unsafe_code))]
// Test code asserts against known constants and unwraps expected-Ok results. The strict
// production lints would make that unreadable without adding any safety.
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        clippy::indexing_slicing,
        clippy::integer_division,
        clippy::panic,
        clippy::unwrap_used,
    )
)]

pub mod constants;
pub mod error;
pub mod fees;
pub mod fixed;
pub mod funding;
pub mod margin;
pub mod oracle;
pub mod pnl;
pub mod pricing;
pub mod types;

pub use error::{MathError, MathResult};
pub use oracle::ValidatedPrice;
pub use types::{Direction, QuoteConversion, Side, TradeAction};
