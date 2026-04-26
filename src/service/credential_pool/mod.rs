//! 凭据池：组合 store + state + stats + selector + refresher
//!
//! Phase 2.11-2.12 实现 state + stats；Phase 2.13 实现 store；Phase 2.14 实现 pool 门面。

#![allow(dead_code)]

pub mod admin;
pub mod pool;
pub mod state;
pub mod stats;
pub mod stats_persister;
pub mod store;

pub use admin::AdminPoolError;
pub use pool::{CallContext, CredentialPool};
pub use state::CredentialState;
pub use stats::{CredentialStats, EntryStats};
pub use store::CredentialStore;
