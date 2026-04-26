//! 协议层：Anthropic ↔ Kiro 转换、流式归约、推送策略、token 估算
//!
//! 子模块直接通过 `crate::service::conversation::<sub>::<sym>` 访问；不在此层 re-export。

#![allow(dead_code)]

pub mod converter;
pub mod delivery;
pub mod reducer;
pub mod thinking;
pub mod tokens;
pub mod tools;
pub mod websearch;
