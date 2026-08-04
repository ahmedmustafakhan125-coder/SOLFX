//! Permissionless upkeep.
//!
//! Every instruction here can be called by anyone. That is not an oversight — § 9.2 requires
//! at least three independent keeper instances from day one, and a protocol whose upkeep only
//! its operator can perform has a single point of failure at exactly the moment upkeep
//! matters. *Assume yours fails; the protocol must not depend on it.*
//!
//! | Instruction | Cadence (§ 9.1) | What it does |
//! |---|---|---|
//! | `crank_market_price` | on demand | Records a validated price; trips the deviation breaker |
//! | `crank_funding` | hourly ± 60 s | Advances the funding and carry indices |
//! | `crank_market_session` | every 60 s | Advances the regime state machine |
//! | `liquidate_position` | < 2 s from HF < 1 | Closes an underwater position |
//! | `auto_deleverage` | last resort | Force-closes a winner when insurance is exhausted |
//!
//! Trigger orders (TP/SL) are **not** here. They are a trader convenience rather than a risk
//! control, they are absent from Phase 4's exit criteria, and their off-chain executor is
//! Phase 7 work — shipping the on-chain half a phase early would mean orders that nothing
//! fires.

pub mod adl;
pub mod funding;
pub mod liquidate;
pub mod price;
pub mod session;

pub use adl::*;
pub use funding::*;
pub use liquidate::*;
pub use price::*;
pub use session::*;
