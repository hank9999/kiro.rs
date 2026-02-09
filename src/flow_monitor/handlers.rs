//! Flow Monitor API 处理器

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use super::router::FlowMonitorState;
use super::types::FlowQuery;

/// GET /api/admin/flows
pub async fn get_flows(
    State(state): State<FlowMonitorState>,
    Query(query): Query<FlowQuery>,
) -> impl IntoResponse {
    // 校验时间格式
    if let Some(ref t) = query.start_time {
        if chrono::DateTime::parse_from_rfc3339(t).is_err() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {"type": "invalid_request_error", "message": format!("无效的 startTime 格式，需要 RFC3339 格式: {}", t)}
                })),
            ).into_response();
        }
    }
    if let Some(ref t) = query.end_time {
        if chrono::DateTime::parse_from_rfc3339(t).is_err() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {"type": "invalid_request_error", "message": format!("无效的 endTime 格式，需要 RFC3339 格式: {}", t)}
                })),
            ).into_response();
        }
    }
    match state.monitor.query(query).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => {
            tracing::error!("查询流量记录失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {"type": "internal_error", "message": format!("查询失败: {}", e)}
                })),
            )
                .into_response()
        }
    }
}

/// GET /api/admin/flows/stats
pub async fn get_flow_stats(
    State(state): State<FlowMonitorState>,
) -> impl IntoResponse {
    match state.monitor.get_stats().await {
        Ok(response) => Json(response).into_response(),
        Err(e) => {
            tracing::error!("获取流量统计失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {"type": "internal_error", "message": format!("统计失败: {}", e)}
                })),
            )
                .into_response()
        }
    }
}

/// DELETE /api/admin/flows 查询参数
#[derive(serde::Deserialize)]
pub struct ClearFlowsQuery {
    pub before: Option<String>,
}

/// DELETE /api/admin/flows
pub async fn clear_flows(
    State(state): State<FlowMonitorState>,
    Query(query): Query<ClearFlowsQuery>,
) -> impl IntoResponse {
    let before = query.before;

    // 校验 before 时间格式
    if let Some(ref before_str) = before {
        if chrono::DateTime::parse_from_rfc3339(before_str).is_err() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {"type": "invalid_request_error", "message": format!("无效的时间格式，需要 RFC3339 格式: {}", before_str)}
                })),
            ).into_response();
        }
    }

    match state.monitor.clear(before).await {
        Ok(count) => Json(serde_json::json!({
            "success": true,
            "message": format!("已清除 {} 条记录", count)
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("清空流量记录失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {"type": "internal_error", "message": format!("清空失败: {}", e)}
                })),
            )
                .into_response()
        }
    }
}
