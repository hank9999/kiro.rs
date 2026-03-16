//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};

use super::{
    handlers::{
        add_credential, delete_credential, get_all_credentials, get_credential_balance,
        get_load_balancing_mode, reset_failure_count, set_credential_disabled,
        set_credential_priority, set_load_balancing_mode,
    },
    proxy_pool_handlers::{
        add_proxy, batch_add_proxies, delete_proxy, list_proxies, set_proxy_disabled,
        test_proxy, update_proxy,
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
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /config/load-balancing` - 获取负载均衡模式
/// - `PUT /config/load-balancing` - 设置负载均衡模式
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
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        // 代理池路由
        .route(
            "/proxy-pool",
            get(list_proxies).post(add_proxy),
        )
        .route("/proxy-pool/batch", post(batch_add_proxies))
        .route("/proxy-pool/{id}", put(update_proxy).delete(delete_proxy))
        .route("/proxy-pool/{id}/disabled", post(set_proxy_disabled))
        .route("/proxy-pool/{id}/test", post(test_proxy))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
