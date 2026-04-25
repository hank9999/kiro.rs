//! HTTP infrastructure：reqwest 客户端工厂、ProxyConfig、RequestExecutor、RetryPolicy
//!
//! Phase 2 引入 client；Phase 3 引入 executor + retry。

#![allow(dead_code)]

pub mod client;
