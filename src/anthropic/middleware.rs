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

use super::types::ErrorResponse;

/// 凭证 ID（通过请求扩展传递）
#[derive(Clone, Copy, Debug)]
pub struct CredentialId(pub u64);

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// API 密钥
    pub api_key: String,
    /// Kiro Provider（可选，用于实际 API 调用）
    /// 内部使用 MultiTokenManager，已支持线程安全的多凭据管理
    pub kiro_provider: Option<Arc<KiroProvider>>,
    /// 是否开启非流式响应的 thinking 块提取
    pub extract_thinking: bool,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(api_key: impl Into<String>, extract_thinking: bool) -> Self {
        Self {
            api_key: api_key.into(),
            kiro_provider: None,
            extract_thinking,
        }
    }

    /// 设置 KiroProvider
    pub fn with_kiro_provider(mut self, provider: KiroProvider) -> Self {
        self.kiro_provider = Some(Arc::new(provider));
        self
    }
}

/// API Key 认证中间件
///
/// 支持两种 API key 格式：
/// - `{base_key}` - 完全匹配配置的 API key，使用系统配置的策略（priority 或 balanced 模式）
/// - `{base_key}-{credential_id}` - 直接使用指定凭证
///
/// 认证逻辑：
/// 1. 先检查是否完全匹配配置的 API key → 使用系统配置的策略
/// 2. 尝试解析凭证 ID → 使用指定凭证
/// 3. 都不匹配 → 返回 401
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    match auth::extract_api_key(&request) {
        Some(key) => {
            // 1. 先检查是否完全匹配（使用系统配置的策略）
            if auth::constant_time_eq(&key, &state.api_key) {
                return next.run(request).await;
            }

            // 2. 尝试解析凭证 ID
            if let Some(credential_id) = auth::parse_credential_id(&key, &state.api_key) {
                tracing::debug!("API key 指定使用凭证 ID: {}", credential_id);
                request.extensions_mut().insert(CredentialId(credential_id));
                return next.run(request).await;
            }

            // 3. 都不匹配，返回 401
            let error = ErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
        None => {
            let error = ErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
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
