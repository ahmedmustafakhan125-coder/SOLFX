//! Instruction handlers, grouped by who is allowed to call them.
//!
//! The grouping is the authority model made visible: `admin` needs the multisig, `guardian`
//! can only restrict, `user` needs the account owner's signature, and `keeper` is
//! permissionless.

pub mod admin;
pub mod guardian;
pub mod keeper;
pub mod user;

pub use admin::*;
pub use guardian::*;
pub use keeper::*;
pub use user::*;
