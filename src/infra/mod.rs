//! Infrastructure layer：trait 实现、I/O 适配、外部协议
//!
//! Phase 2 起逐步接入。Phase 1 / 2 期间老 `kiro/` 模块仍并存，Phase 7 移除。

#![allow(dead_code)]

pub mod endpoint;
pub mod http;
pub mod machine_id;
pub mod parser;
pub mod refresher;
pub mod selector;
pub mod storage;
