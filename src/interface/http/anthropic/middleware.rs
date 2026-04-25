//! Anthropic API 中间件（迁移自 anthropic/middleware.rs）

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use crate::interface::http::auth;
use crate::service::KiroClient;

use super::dto::ErrorResponse;

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// API 密钥
    pub api_key: String,
    /// KiroClient（可选，用于实际 API 调用）
    pub kiro_client: Option<Arc<KiroClient>>,
    /// 是否开启非流式响应的 thinking 块提取
    pub extract_thinking: bool,
}

impl AppState {
    pub fn new(api_key: impl Into<String>, extract_thinking: bool) -> Self {
        Self {
            api_key: api_key.into(),
            kiro_client: None,
            extract_thinking,
        }
    }

    pub fn with_kiro_client(mut self, client: Arc<KiroClient>) -> Self {
        self.kiro_client = Some(client);
        self
    }
}

/// API Key 认证中间件
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match auth::extract_api_key(&request) {
        Some(key) if auth::constant_time_eq(&key, &state.api_key) => next.run(request).await,
        _ => {
            let error = ErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}

/// CORS 中间件层
pub fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
