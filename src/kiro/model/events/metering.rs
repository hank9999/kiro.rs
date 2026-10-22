//! 计费事件
//!
//! 处理 meteringEvent。载荷只有 credit 总量，没有 token 分桶。

use serde::Deserialize;

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

/// Kiro 计费事件
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeteringEvent {
    /// 计量单位。空字符串或 `credit` 按 credit 处理。
    #[serde(default)]
    pub unit: String,
    /// 单位复数。反推缓存时不使用。
    #[serde(default)]
    #[allow(dead_code)]
    pub unit_plural: String,
    /// 本次消耗量
    #[serde(default)]
    pub usage: f64,
}

impl EventPayload for MeteringEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}
