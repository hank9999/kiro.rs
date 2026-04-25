//! 凭据池：组合 store + state + stats + selector + refresher
//!
//! Phase 2.11-2.12 实现 state + stats；Phase 2.13 实现 store；Phase 2.14 实现 pool 门面。

#![allow(dead_code)]

pub mod pool;
pub mod state;
pub mod stats;
pub mod store;

pub use pool::{CallContext, CredentialPool, MODE_BALANCED, MODE_PRIORITY};
pub use state::{CredentialState, EntryState, MAX_FAILURES_PER_CREDENTIAL};
pub use stats::{CredentialStats, EntryStats};
pub use store::{CredentialStore, ValidationIssue, ValidationKind};
