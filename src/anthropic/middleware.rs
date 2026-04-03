//! Anthropic API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use crate::common::auth;
use crate::kiro::provider::KiroProvider;
use crate::monitoring::{RequestMetadata, RequestMonitor};

use super::types::ErrorResponse;

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// API 密钥
    pub api_key: String,
    /// Kiro Provider（可选，用于实际 API 调用）
    /// 内部使用 MultiTokenManager，已支持线程安全的多凭据管理
    pub kiro_provider: Option<Arc<KiroProvider>>,
    /// Profile ARN（可选，用于请求）
    pub profile_arn: Option<String>,
    /// 请求监控器
    pub request_monitor: RequestMonitor,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(api_key: impl Into<String>, request_monitor: RequestMonitor) -> Self {
        Self {
            api_key: api_key.into(),
            kiro_provider: None,
            profile_arn: None,
            request_monitor,
        }
    }

    /// 设置 KiroProvider
    pub fn with_kiro_provider(mut self, provider: KiroProvider) -> Self {
        self.kiro_provider = Some(Arc::new(provider));
        self
    }

    /// 设置 Profile ARN
    pub fn with_profile_arn(mut self, arn: impl Into<String>) -> Self {
        self.profile_arn = Some(arn.into());
        self
    }
}

/// API Key 认证 + 请求记录中间件
pub async fn auth_and_monitor_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let tracker = state.request_monitor.start(RequestMetadata::new(
        request.method().to_string(),
        request.uri().path().to_string(),
    ));

    match auth::extract_api_key(&request) {
        Some(key) if auth::constant_time_eq(&key, &state.api_key) => {
            let response = next.run(request).await;
            tracker.finish(response.status().as_u16(), None);
            response
        }
        _ => {
            let error = ErrorResponse::authentication_error();
            let response = (StatusCode::UNAUTHORIZED, Json(error)).into_response();
            tracker.finish(
                StatusCode::UNAUTHORIZED.as_u16(),
                Some("Authentication failed".to_string()),
            );
            response
        }
    }
}

/// CORS 中间件层
///
/// **安全说明**：当前配置允许所有来源（Any），这是为了支持公开 API 服务。
/// 如果需要更严格的安全控制，请根据实际需求配置具体的允许来源、方法和头信息。
///
/// # 配置说明
/// - `allow_origin(Any)`: 允许任何来源的请求
/// - `allow_methods(Any)`: 允许任何 HTTP 方法
/// - `allow_headers(Any)`: 允许任何请求头
pub fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};

    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
