//! Flow Monitor API 路由

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};

use crate::common::auth;

use super::handlers::{clear_flows, get_flow_stats, get_flows};
use super::store::FlowMonitor;

/// Flow Monitor API 状态
#[derive(Clone)]
pub struct FlowMonitorState {
    pub admin_api_key: String,
    pub monitor: Arc<FlowMonitor>,
}

/// Flow Monitor 认证中间件
async fn flow_auth_middleware(
    State(state): State<FlowMonitorState>,
    request: Request<Body>,
    next: Next,
) -> axum::response::Response {
    match auth::extract_api_key(&request) {
        Some(key) if auth::constant_time_eq(&key, &state.admin_api_key) => {
            next.run(request).await
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {"type": "authentication_error", "message": "Invalid API key"}
            })),
        )
            .into_response(),
    }
}

/// 创建 Flow Monitor API 路由
///
/// 返回 Router<()>，可直接 nest 到主应用
pub fn create_flow_monitor_router(
    admin_api_key: impl Into<String>,
    monitor: Arc<FlowMonitor>,
) -> Router {
    let state = FlowMonitorState {
        admin_api_key: admin_api_key.into(),
        monitor,
    };

    Router::new()
        .route("/flows", get(get_flows).delete(clear_flows))
        .route("/flows/stats", get(get_flow_stats))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            flow_auth_middleware,
        ))
        .with_state(state)
}
