//! On-chain account layouts.
//!
//! Every struct here carries `_reserved` padding and is **append-only**: Solana
//! deserialises by byte offset, so reordering or retyping a field corrupts every account of
//! that type already on chain (`ARCHITECTURE.md` § 5.6). New fields consume reserved bytes.

pub mod lp_pool;
pub mod market;
pub mod position;
pub mod protocol;
pub mod user_account;

pub use lp_pool::{InsuranceFund, LpPool, LpWithdrawRequest};
pub use market::{FeedKind, Market, MarketStatus, PriceSource, QuoteConversionKind};
pub use position::{Direction, Position};
pub use protocol::Protocol;
pub use user_account::UserAccount;
