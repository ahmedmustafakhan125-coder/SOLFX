pub mod initialize_market;
pub mod initialize_protocol;
pub mod protocol_admin;
pub mod set_market_oracle;
pub mod update_market;

// Glob re-exports are required, not stylistic: `#[derive(Accounts)]` generates a hidden
// `__client_accounts_*` module next to each struct, and `#[program]` resolves it through
// this namespace. Naming it explicitly would couple us to Anchor's internal naming.
//
// The consequence is that no two modules here may export the same symbol, which is why the
// two initialisation handlers are `init_protocol` and `init_market` rather than both
// `handler`.
pub use initialize_market::*;
pub use initialize_protocol::*;
pub use protocol_admin::*;
pub use set_market_oracle::*;
pub use update_market::*;
