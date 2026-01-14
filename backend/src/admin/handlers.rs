//! Admin API HTTP 处理器
//!
//! Input: AdminState, 请求数据
//! Output: JSON 响应
//! Pos: Admin API 请求处理层

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};

use super::{
    middleware::{AdminState, AUTH_COOKIE_NAME},
    types::{
        AddCredentialRequest, ChangePasswordRequest, ExportCredentialsRequest,
        ImportCredentialsRequest, LoginRequest, LoginResponse, SetDisabledRequest,
        SetPriorityRequest, SettingsResponse, SuccessResponse, UpdateCredentialRequest,
        UpdateSettingsRequest, UserInfoResponse,
    },
};
use crate::jwt::Claims;

// ============ 认证相关 ============

/// POST /api/admin/login
/// 用户登录
pub async fn login(
    State(state): State<AdminState>,
    jar: CookieJar,
    Json(payload): Json<LoginRequest>,
) -> impl IntoResponse {
    match state.db.verify_user(&payload.username, &payload.password) {
        Ok(Some(user_id)) => {
            // 生成 JWT token
            match state.jwt.generate_token(user_id, &payload.username) {
                Ok(token) => {
                    // 设置 Cookie
                    let cookie = Cookie::build((AUTH_COOKIE_NAME, token))
                        .path("/")
                        .http_only(true)
                        .same_site(SameSite::Lax)
                        .max_age(time::Duration::hours(24))
                        .build();

                    let jar = jar.add(cookie);

                    (
                        jar,
                        Json(LoginResponse {
                            success: true,
                            message: "登录成功".to_string(),
                            username: Some(payload.username),
                        }),
                    )
                        .into_response()
                }
                Err(e) => {
                    tracing::error!("生成 token 失败: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(LoginResponse {
                            success: false,
                            message: "服务器错误".to_string(),
                            username: None,
                        }),
                    )
                        .into_response()
                }
            }
        }
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Json(LoginResponse {
                success: false,
                message: "用户名或密码错误".to_string(),
                username: None,
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("验证用户失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(LoginResponse {
                    success: false,
                    message: "服务器错误".to_string(),
                    username: None,
                }),
            )
                .into_response()
        }
    }
}

/// POST /api/admin/logout
/// 用户登出
pub async fn logout(jar: CookieJar) -> impl IntoResponse {
    let cookie = Cookie::build((AUTH_COOKIE_NAME, ""))
        .path("/")
        .max_age(time::Duration::seconds(0))
        .build();

    let jar = jar.remove(cookie);

    (
        jar,
        Json(SuccessResponse::new("已登出")),
    )
}

/// GET /api/admin/me
/// 获取当前用户信息
pub async fn get_current_user(Extension(claims): Extension<Claims>) -> impl IntoResponse {
    Json(UserInfoResponse {
        id: claims.sub,
        username: claims.username,
    })
}

/// POST /api/admin/change-password
/// 修改密码
pub async fn change_password(
    State(state): State<AdminState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ChangePasswordRequest>,
) -> impl IntoResponse {
    match state.db.change_password(claims.sub, &payload.old_password, &payload.new_password) {
        Ok(true) => Json(SuccessResponse::new("密码修改成功")).into_response(),
        Ok(false) => (
            StatusCode::BAD_REQUEST,
            Json(SuccessResponse {
                success: false,
                message: "原密码错误".to_string(),
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("修改密码失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SuccessResponse {
                    success: false,
                    message: "服务器错误".to_string(),
                }),
            )
                .into_response()
        }
    }
}

// ============ 凭据管理 ============

/// GET /api/admin/credentials
/// 获取所有凭据状态
pub async fn get_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_all_credentials();
    Json(response)
}

/// POST /api/admin/credentials/:id/disabled
/// 设置凭据禁用状态
pub async fn set_credential_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    match state.service.set_disabled(id, payload.disabled) {
        Ok(_) => {
            let action = if payload.disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("凭据 #{} 已{}", id, action))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/priority
/// 设置凭据优先级
pub async fn set_credential_priority(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetPriorityRequest>,
) -> impl IntoResponse {
    match state.service.set_priority(id, payload.priority) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 优先级已设置为 {}",
            id, payload.priority
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/reset
/// 重置失败计数并重新启用
pub async fn reset_failure_count(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_and_enable(id) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 失败计数已重置并重新启用",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/balance
/// 获取指定凭据的余额
pub async fn get_credential_balance(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_balance(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials
/// 添加新凭据
pub async fn add_credential(
    State(state): State<AdminState>,
    Json(payload): Json<AddCredentialRequest>,
) -> impl IntoResponse {
    match state.service.add_credential(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/credentials/:id
/// 更新凭据元数据
pub async fn update_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateCredentialRequest>,
) -> impl IntoResponse {
    match state.service.update_credential(id, payload) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已更新", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/credentials/:id
/// 删除凭据
pub async fn delete_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_credential(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/refresh
/// 刷新所有凭据的余额信息
pub async fn refresh_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.refresh_all_balances().await;
    Json(response)
}

// ============ 系统设置 ============

/// GET /api/admin/settings
/// 获取系统设置
pub async fn get_settings(State(state): State<AdminState>) -> impl IntoResponse {
    let kiro_version = state.db.get_setting("kiro_version")
        .ok()
        .flatten()
        .unwrap_or_else(|| "0.8.0".to_string());
    let system_version = state.db.get_setting("system_version")
        .ok()
        .flatten()
        .unwrap_or_else(|| "darwin#24.6.0".to_string());
    let node_version = state.db.get_setting("node_version")
        .ok()
        .flatten()
        .unwrap_or_else(|| "v22.12.0".to_string());
    let min_usage_threshold = state.db.get_setting("min_usage_threshold")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(5.0);
    let count_tokens_api_url = state.db.get_setting("count_tokens_api_url")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    let count_tokens_api_key = state.db.get_setting("count_tokens_api_key")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    let count_tokens_auth_type = state.db.get_setting("count_tokens_auth_type")
        .ok()
        .flatten()
        .unwrap_or_else(|| "x-api-key".to_string());

    Json(SettingsResponse {
        kiro_version,
        system_version,
        node_version,
        min_usage_threshold,
        count_tokens_api_url,
        count_tokens_api_key,
        count_tokens_auth_type,
    })
}

/// POST /api/admin/settings
/// 更新系统设置
pub async fn update_settings(
    State(state): State<AdminState>,
    Json(payload): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    let mut updates: Vec<(&str, String)> = Vec::new();

    if let Some(ref v) = payload.kiro_version {
        updates.push(("kiro_version", v.clone()));
    }
    if let Some(ref v) = payload.system_version {
        updates.push(("system_version", v.clone()));
    }
    if let Some(ref v) = payload.node_version {
        updates.push(("node_version", v.clone()));
    }
    if let Some(v) = payload.min_usage_threshold {
        updates.push(("min_usage_threshold", v.to_string()));
    }
    if let Some(ref v) = payload.count_tokens_api_url {
        updates.push(("count_tokens_api_url", v.clone()));
    }
    if let Some(ref v) = payload.count_tokens_api_key {
        updates.push(("count_tokens_api_key", v.clone()));
    }
    if let Some(ref v) = payload.count_tokens_auth_type {
        updates.push(("count_tokens_auth_type", v.clone()));
    }

    if updates.is_empty() {
        return Json(SuccessResponse::new("没有需要更新的设置")).into_response();
    }

    let updates_ref: Vec<(&str, &str)> = updates.iter().map(|(k, v)| (*k, v.as_str())).collect();
    match state.db.set_settings(&updates_ref) {
        Ok(_) => Json(SuccessResponse::new("设置已更新")).into_response(),
        Err(e) => {
            tracing::error!("更新设置失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SuccessResponse {
                    success: false,
                    message: "更新设置失败".to_string(),
                }),
            )
                .into_response()
        }
    }
}

// ============ 导入导出 ============

/// POST /api/admin/credentials/:id/use
/// 使用此账号（设置为当前凭据）
pub async fn set_current_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.set_current(id) {
        Ok(_) => Json(SuccessResponse::new(format!("已切换到凭据 #{}", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/export
/// 导出凭据
pub async fn export_credentials(
    State(state): State<AdminState>,
    Json(payload): Json<ExportCredentialsRequest>,
) -> impl IntoResponse {
    let response = state.service.export_credentials(payload);
    Json(response)
}

/// POST /api/admin/credentials/import
/// 批量导入凭据
pub async fn import_credentials(
    State(state): State<AdminState>,
    Json(payload): Json<ImportCredentialsRequest>,
) -> impl IntoResponse {
    let items = payload.into_vec();
    let response = state.service.import_credentials(items).await;
    Json(response)
}
