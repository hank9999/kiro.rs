//! Flow Monitor API 请求/响应类型

use serde::{Deserialize, Serialize};

/// 查询过滤器
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

/// 分页响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowListResponse {
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub records: Vec<FlowRecordResponse>,
}

/// 单条记录响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowRecordResponse {
    pub id: i64,
    pub request_id: String,
    pub timestamp: String,
    pub path: String,
    pub model: String,
    pub stream: bool,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: i64,
    pub status_code: u16,
    pub error: Option<String>,
    pub user_id: Option<String>,
}

/// 统计响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowStatsResponse {
    pub total_requests: u64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub avg_duration_ms: f64,
    pub error_count: u64,
    pub error_rate: f64,
    pub models: Vec<ModelStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStats {
    pub model: String,
    pub count: u64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub avg_duration_ms: f64,
}
