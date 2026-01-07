//! Anthropic API 路由配置

use axum::{
    Router, middleware,
    routing::{get, post},
};

use crate::kiro::provider::KiroProvider;

use super::{
    handlers::{count_tokens, get_models, post_messages},
    middleware::{AppState, auth_middleware, cors_layer},
};

/// 创建 Anthropic API 路由
///
/// # 端点
/// - `GET /v1/models` - 获取可用模型列表
/// - `POST /v1/messages` - 创建消息（对话）
/// - `POST /v1/messages/count_tokens` - 计算 token 数量
///
/// # 认证
/// 所有 `/v1` 路径需要 API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
///
/// # 参数
/// - `api_key`: API 密钥，用于验证客户端请求
/// - `kiro_provider`: 可选的 KiroProvider，用于调用上游 API

/// 创建带有 KiroProvider 的 Anthropic API 路由
pub fn create_router_with_provider(
    api_key: impl Into<String>,
    kiro_provider: Option<KiroProvider>,
    profile_arn: Option<String>,
) -> Router {
    let mut state = AppState::new(api_key);
    if let Some(provider) = kiro_provider {
        state = state.with_kiro_provider(provider);
    }
    if let Some(arn) = profile_arn {
        state = state.with_profile_arn(arn);
    }

    // 需要认证的 /v1 路由
    let v1_routes = Router::new()
        .route("/models", get(get_models))
        .route("/messages", post(post_messages))
        .route("/messages/count_tokens", post(count_tokens))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        .nest("/v1", v1_routes)
        .layer(cors_layer())
        .with_state(state)
}
