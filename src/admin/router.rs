//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post},
};

use super::{
    handlers::{
        add_credential, clear_immediate_failure_disabled, delete_credential,
        delete_premium_credential, export_premium_credentials, force_refresh_token,
        get_all_credentials, get_credential_balance, get_load_balancing_mode,
        get_premium_credentials, get_runtime_metrics, reset_all_credentials, reset_failure_count,
        restore_premium_credential, set_credential_disabled, set_credential_priority,
        set_load_balancing_mode,
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
/// - `POST /credentials/reset-all` - 启动所有账号并重置失败计数
/// - `POST /credentials/clear-immediate-failures` - 批量清除 ImmediateFailure 已禁用凭据
/// - `POST /credentials/:id/refresh` - 强制刷新 Token
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /config/load-balancing` - 获取负载均衡模式
/// - `PUT /config/load-balancing` - 设置负载均衡模式
/// - `GET /runtime/metrics` - 获取运行时轻量指标
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
        .route("/credentials/reset-all", post(reset_all_credentials))
        .route(
            "/credentials/clear-immediate-failures",
            post(clear_immediate_failure_disabled),
        )
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/premium-credentials", get(get_premium_credentials))
        .route(
            "/premium-credentials/export",
            post(export_premium_credentials),
        )
        .route(
            "/premium-credentials/{id}/restore",
            post(restore_premium_credential),
        )
        .route(
            "/premium-credentials/{id}",
            delete(delete_premium_credential),
        )
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .route("/runtime/metrics", get(get_runtime_metrics))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
