//! Admin API 路由配置

use std::sync::Arc;

use axum::{
    Router, middleware,
    routing::{delete, get, post},
};

use super::{
    auth_handlers,
    captcha::CaptchaStore,
    handlers::{
        add_credential, delete_credential, get_all_credentials, get_credential_balance,
        get_load_balancing_mode, reset_failure_count, set_credential_disabled,
        set_credential_priority, set_load_balancing_mode,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 端点
/// - `GET /auth/captcha` - 获取 CAPTCHA 图片（无需认证）
/// - `POST /auth/login` - 登录获取 JWT token（无需认证）
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
/// - Auth 端点：无需认证
/// - Admin 端点：需要 JWT token（通过 `Authorization: Bearer <token>` header）
pub fn create_admin_router(state: AdminState, captcha_store: Arc<CaptchaStore>) -> Router {
    // 未认证的 auth 路由
    let auth_routes = Router::new()
        .route("/auth/captcha", get(auth_handlers::get_captcha))
        .route("/auth/login", post(auth_handlers::login))
        .with_state((state.admin_api_key.clone(), captcha_store));

    // 需要认证的 admin 路由
    let admin_routes = Router::new()
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state);

    // 合并路由
    Router::new()
        .merge(auth_routes)
        .merge(admin_routes)
}
