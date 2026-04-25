//! Anthropic API 路由配置（迁移自 anthropic/router.rs，使用新 KiroClient）

use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};

use crate::service::KiroClient;

use super::handlers::{count_tokens, get_models, post_messages, post_messages_cc};
use super::middleware::{AppState, auth_middleware, cors_layer};

/// 请求体最大大小限制 (50MB)
const MAX_BODY_SIZE: usize = 50 * 1024 * 1024;

/// 创建 Anthropic API 路由
///
/// # 端点
/// - `GET /v1/models`
/// - `POST /v1/messages`
/// - `POST /v1/messages/count_tokens`
/// - `POST /cc/v1/messages` (Claude Code 兼容)
/// - `POST /cc/v1/messages/count_tokens`
pub fn create_router(
    api_key: impl Into<String>,
    kiro_client: Option<Arc<KiroClient>>,
    extract_thinking: bool,
) -> Router {
    let mut state = AppState::new(api_key, extract_thinking);
    if let Some(client) = kiro_client {
        state = state.with_kiro_client(client);
    }

    let v1_routes = Router::new()
        .route("/models", get(get_models))
        .route("/messages", post(post_messages))
        .route("/messages/count_tokens", post(count_tokens))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    let cc_v1_routes = Router::new()
        .route("/messages", post(post_messages_cc))
        .route("/messages/count_tokens", post(count_tokens))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .nest("/v1", v1_routes)
        .nest("/cc/v1", cc_v1_routes)
        .layer(cors_layer())
        .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
        .with_state(state)
}
