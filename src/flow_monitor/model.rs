//! 流量记录数据模型

use serde::Serialize;

/// 流量记录
#[derive(Debug, Clone, Serialize)]
pub struct FlowRecord {
    pub id: i64,
    pub request_id: String,
    pub timestamp: String,
    pub method: String,
    pub path: String,
    pub model: String,
    pub stream: bool,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub duration_ms: i64,
    pub status_code: u16,
    pub error: Option<String>,
    pub user_id: Option<String>,
}
