//! Admin API 路由配置
//!
//! Input: handlers, middleware
//! Output: Router
//! Pos: Admin API 路由定义

use axum::{
    Router, middleware,
    routing::{delete, get, post},
};

use super::{
    handlers::{
        add_credential, change_password, delete_credential, export_credentials,
        get_all_credentials, get_credential_balance, get_current_user, get_settings,
        import_credentials, login, logout, refresh_all_credentials, reset_failure_count,
        set_credential_disabled, set_credential_priority, set_current_credential,
        update_credential, update_settings,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 公开端点（无需认证）
/// - `POST /login` - 用户登录
/// - `POST /logout` - 用户登出
///
/// # 需要认证的端点
/// - `GET /me` - 获取当前用户信息
/// - `POST /change-password` - 修改密码
/// - `GET /credentials` - 获取所有凭据状态
/// - `POST /credentials` - 添加新凭据
/// - `DELETE /credentials/:id` - 删除凭据
/// - `POST /credentials/:id/disabled` - 设置凭据禁用状态
/// - `POST /credentials/:id/priority` - 设置凭据优先级
/// - `POST /credentials/:id/reset` - 重置失败计数
/// - `POST /credentials/:id/use` - 使用此账号
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `POST /credentials/refresh` - 刷新所有凭据余额
/// - `POST /credentials/export` - 导出凭据
/// - `POST /credentials/import` - 批量导入凭据
/// - `GET /settings` - 获取系统设置
/// - `POST /settings` - 更新系统设置
pub fn create_admin_router(state: AdminState) -> Router {
    // 公开路由（无需认证）
    let public_routes = Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .with_state(state.clone());

    // 需要认证的路由
    let protected_routes = Router::new()
        .route("/me", get(get_current_user))
        .route("/change-password", post(change_password))
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/credentials/refresh", post(refresh_all_credentials))
        .route("/credentials/export", post(export_credentials))
        .route("/credentials/import", post(import_credentials))
        .route("/credentials/{id}", delete(delete_credential).put(update_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/use", post(set_current_credential))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/settings", get(get_settings).post(update_settings))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state);

    // 合并路由
    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
}
