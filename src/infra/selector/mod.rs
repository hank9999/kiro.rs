//! 凭据选择器：PrioritySelector + BalancedSelector
//!
//! Selector 必须同步、纯计算；disabled 过滤由 pool 在拼装 view 时完成。

#![allow(dead_code)]

pub mod balanced;
pub mod priority;

pub use balanced::BalancedSelector;
pub use priority::PrioritySelector;
