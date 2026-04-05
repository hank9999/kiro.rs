//! Anthropic API 中间件

use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use std::sync::RwLock;

use axum::{
    body::Body,
    extract::{ConnectInfo, OriginalUri, State},
    http::{
        Request, StatusCode,
        header::{self, HeaderValue},
    },
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use tracing::Instrument;

use crate::common::auth;
use crate::kiro::provider::KiroProvider;
use crate::monitoring::{RequestMetadata, RequestMetadataHandle, RequestMonitor};

use super::types::ErrorResponse;

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// API 密钥列表（支持多Key认证）
    pub api_keys: Arc<RwLock<Vec<String>>>,
    /// 配置文件路径（用于重新加载）
    pub config_path: PathBuf,
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
    pub fn new(
        api_keys: Vec<String>,
        config_path: PathBuf,
        request_monitor: RequestMonitor,
    ) -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(api_keys)),
            config_path,
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

    /// 重新加载 API Keys（从配置文件）
    pub fn reload_keys(&self) -> anyhow::Result<()> {
        use crate::model::config::Config;
        let config = Config::load(&self.config_path)?;
        let new_keys = config.collect_valid_keys();

        let mut keys = self.api_keys.write().unwrap();
        *keys = new_keys;

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct RequestContext {
    pub request_id: String,
    pub internal_id: u64,
    metadata: RequestMetadataHandle,
}

impl RequestContext {
    fn new(internal_id: u64, request_id: String, metadata: RequestMetadataHandle) -> Self {
        Self {
            request_id,
            internal_id,
            metadata,
        }
    }

    pub fn annotate_request(
        &self,
        model: impl Into<String>,
        message_count: Option<usize>,
        stream: Option<bool>,
    ) {
        let model = model.into();
        tracing::debug!(
            request_id = %self.request_id,
            internal_request_id = self.internal_id,
            model = %model,
            message_count = ?message_count,
            stream = ?stream,
            "Updated request activity metadata"
        );
        self.metadata.annotate_request(model, message_count, stream);
    }
}

/// API Key 认证 + 请求记录中间件
pub async fn auth_and_monitor_middleware(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let metadata = build_request_metadata(&request);
    let tracker = state.request_monitor.start(metadata.clone());
    let request_id = tracker.request_id().to_string();
    let request_span = tracing::info_span!(
        "api_request",
        request_id = %request_id,
        method = %request.method(),
        path = %metadata.path(),
        endpoint = %metadata.endpoint(),
        client_ip = %metadata.client_ip().unwrap_or("-"),
        client_request_id = %metadata.client_request_id().unwrap_or("-"),
    );
    let request_context =
        RequestContext::new(tracker.id(), request_id.clone(), tracker.metadata_handle());
    request.extensions_mut().insert(request_context);

    // 验证 API Key（支持多Key）
    let request_key = auth::extract_api_key(&request);
    let is_valid = if let Some(ref key) = request_key {
        let keys = state.api_keys.read().unwrap();
        keys.iter().any(|k| auth::constant_time_eq(k, key))
    } else {
        false
    };

    if is_valid {
        let mut response = next.run(request).instrument(request_span.clone()).await;
        insert_request_id_header(&mut response, &request_id);
        tracker.finish(response.status().as_u16(), None);
        response
    } else {
        let error = ErrorResponse::authentication_error();
        let mut response = (StatusCode::UNAUTHORIZED, Json(error)).into_response();
        insert_request_id_header(&mut response, &request_id);
        tracker.finish(
            StatusCode::UNAUTHORIZED.as_u16(),
            Some("Authentication failed".to_string()),
        );
        response
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

fn build_request_metadata(request: &Request<Body>) -> RequestMetadata {
    RequestMetadata::new(request.method().to_string(), original_path(request))
        .with_client_ip(socket_ip(request))
        .with_forwarded_for(header_value(request, "x-forwarded-for"))
        .with_real_ip(
            header_value(request, "x-real-ip")
                .or_else(|| header_value(request, "cf-connecting-ip")),
        )
        .with_forwarded_proto(header_value(request, "x-forwarded-proto"))
        .with_user_agent(header_value(request, header::USER_AGENT.as_str()))
        .with_referer(header_value(request, header::REFERER.as_str()))
        .with_origin(header_value(request, header::ORIGIN.as_str()))
        .with_transfer_encoding(header_value(request, header::TRANSFER_ENCODING.as_str()))
        .with_content_length(content_length(request))
        .with_client_request_id(
            header_value(request, "x-request-id")
                .or_else(|| header_value(request, "request-id"))
                .or_else(|| header_value(request, "x-correlation-id")),
        )
}

fn original_path(request: &Request<Body>) -> String {
    request
        .extensions()
        .get::<OriginalUri>()
        .map(|uri| uri.0.path().to_string())
        .unwrap_or_else(|| request.uri().path().to_string())
}

fn socket_ip(request: &Request<Body>) -> Option<String> {
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|addr| addr.0.ip().to_string())
}

fn header_value(request: &Request<Body>, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| truncate(value, 256))
}

fn content_length(request: &Request<Body>) -> Option<u64> {
    request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn insert_request_id_header(response: &mut Response, request_id: &str) {
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response
            .headers_mut()
            .insert(header::HeaderName::from_static("x-request-id"), value);
    }
}

fn truncate(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }

    let truncated = value.chars().take(max_len).collect::<String>();
    format!("{truncated}...")
}
