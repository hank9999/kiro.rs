//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};

use super::{
    handlers::{
        add_api_key, add_credential, delete_api_key, delete_credential, force_refresh_token,
        generate_api_key, get_all_credentials, get_api_keys, get_available_models,
        get_credential_balance, get_load_balancing_mode, get_recent_logs, get_request_activity,
        reset_failure_count, set_credential_disabled, set_credential_priority,
        set_load_balancing_mode, update_api_key,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 端点
/// - `GET /credentials` - 获取所有凭据状态
/// - `POST /credentials` - 添加新凭据
/// - `DELETE /credentials/:id` - 删除凭据
/// - `POST /credentials/:id/disabled` - 设置凭据禁用状态
/// - `POST /credentials/:id/priority` - 设置凭据优先级
/// - `POST /credentials/:id/reset` - 重置失败计数
/// - `POST /credentials/:id/refresh` - 强制刷新 Token
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /config/load-balancing` - 获取负载均衡模式
/// - `PUT /config/load-balancing` - 设置负载均衡模式
/// - `GET /api-keys` - 获取所有 API Keys
/// - `POST /api-keys` - 添加新 API Key
/// - `POST /api-keys/generate` - 生成随机 API Key
/// - `PUT /api-keys/:id` - 更新 API Key
/// - `DELETE /api-keys/:id` - 删除 API Key
///
/// # 认证
/// 需要 Admin API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn create_admin_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/models", get(get_available_models))
        .route("/activity", get(get_request_activity))
        .route("/logs", get(get_recent_logs))
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .route("/api-keys", get(get_api_keys).post(add_api_key))
        .route("/api-keys/generate", post(generate_api_key))
        .route("/api-keys/{id}", put(update_api_key).delete(delete_api_key))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
