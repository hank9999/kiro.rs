//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};

use super::{
    handlers::{
        add_credential, add_proxy, assign_proxy_to_credential, batch_add_proxies,
        complete_social_login, complete_social_relogin, delete_credential, delete_proxy,
        force_refresh_token, get_all_credentials, get_credential_balance, get_global_proxy,
        get_load_balancing_mode, get_proxy_pool, poll_idc_login, poll_idc_relogin,
        poll_social_login, poll_social_relogin, reset_failure_count, set_credential_disabled,
        set_credential_priority, set_global_proxy, set_load_balancing_mode, set_proxy_enabled,
        start_idc_login, start_idc_relogin, start_social_login, start_social_relogin,
        update_admin_key, update_credential, update_refresh_token,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 端点
/// - `GET /credentials` - 获取所有凭据状态
/// - `POST /credentials` - 添加新凭据
/// - `DELETE /credentials/:id` - 删除凭据
/// - `PUT /credentials/:id` - 更新凭据可编辑字段（email、proxy 等）
/// - `POST /credentials/:id/disabled` - 设置凭据禁用状态
/// - `POST /credentials/:id/priority` - 设置凭据优先级
/// - `POST /credentials/:id/reset` - 重置失败计数
/// - `POST /credentials/:id/refresh` - 强制刷新 Token
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /config/load-balancing` - 获取负载均衡模式
/// - `PUT /config/load-balancing` - 设置负载均衡模式
///
/// # 认证
/// 需要 Admin API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn create_admin_router(state: AdminState) -> Router {
    // 需要 Admin API Key 认证的路由
    let authenticated = Router::new()
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route(
            "/credentials/{id}",
            delete(delete_credential).put(update_credential),
        )
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/refresh-token", put(update_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/credentials/{id}/proxy", post(assign_proxy_to_credential))
        .route("/proxy-pool", get(get_proxy_pool).post(add_proxy))
        .route("/proxy-pool/batch", post(batch_add_proxies))
        .route("/proxy-pool/{id}", delete(delete_proxy))
        .route("/proxy-pool/{id}/enabled", post(set_proxy_enabled))
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .route("/config/global-proxy", get(get_global_proxy).put(set_global_proxy))
        .route("/config/admin-key", put(update_admin_key))
        .route("/auth/idc/start", post(start_idc_login))
        .route("/auth/idc/poll/{session_id}", post(poll_idc_login))
        .route("/auth/social/start", post(start_social_login))
        .route("/auth/social/poll/{session_id}", post(poll_social_login))
        .route("/auth/social/complete/{session_id}", post(complete_social_login))
        .route("/credentials/{id}/relogin/social/start", post(start_social_relogin))
        .route("/credentials/{id}/relogin/social/poll/{session_id}", post(poll_social_relogin))
        .route("/credentials/{id}/relogin/social/complete/{session_id}", post(complete_social_relogin))
        .route("/credentials/{id}/relogin/idc/start", post(start_idc_relogin))
        .route("/credentials/{id}/relogin/idc/poll/{session_id}", post(poll_idc_relogin))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ));

    Router::new()
        .merge(authenticated)
        .with_state(state)
}
