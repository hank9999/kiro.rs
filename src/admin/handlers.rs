//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};

use super::{
    middleware::AdminState,
    types::{
        AddCredentialRequest, SetDisabledRequest, SetLoadBalancingModeRequest, SetPriorityRequest,
        SuccessResponse, UpdateEmailConfigRequest, EmailConfigResponse,
    },
};

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

/// GET /api/admin/config/load-balancing
/// 获取负载均衡模式
pub async fn get_load_balancing_mode(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_load_balancing_mode();
    Json(response)
}

/// PUT /api/admin/config/load-balancing
/// 设置负载均衡模式
pub async fn set_load_balancing_mode(
    State(state): State<AdminState>,
    Json(payload): Json<SetLoadBalancingModeRequest>,
) -> impl IntoResponse {
    match state.service.set_load_balancing_mode(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/email/test
/// 发送测试邮件
pub async fn send_test_email(State(state): State<AdminState>) -> impl IntoResponse {
    match state.notifier.send_test_email().await {
        Ok(_) => Json(SuccessResponse::new("测试邮件已发送")).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SuccessResponse {
                success: false,
                message: format!("{}", e),
            }),
        )
            .into_response(),
    }
}

/// GET /api/admin/email/config
/// 获取邮件配置
pub async fn get_email_config(State(state): State<AdminState>) -> impl IntoResponse {
    match state.notifier.get_config() {
        Some(config) => Json(EmailConfigResponse {
            configured: true,
            smtp_host: Some(config.smtp_host),
            smtp_port: Some(config.smtp_port),
            smtp_username: Some(config.smtp_username),
            from_address: Some(config.from_address),
            recipients: Some(config.recipients),
        }),
        None => Json(EmailConfigResponse {
            configured: false,
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            from_address: None,
            recipients: None,
        }),
    }
}

/// PUT /api/admin/email/config
/// 更新邮件配置
pub async fn update_email_config(
    State(state): State<AdminState>,
    Json(payload): Json<UpdateEmailConfigRequest>,
) -> impl IntoResponse {
    use crate::model::config::EmailConfig;

    let email_config = EmailConfig {
        smtp_host: payload.smtp_host,
        smtp_port: payload.smtp_port,
        smtp_username: payload.smtp_username,
        smtp_password: payload.smtp_password,
        from_address: payload.from_address,
        recipients: payload.recipients,
    };

    // 更新运行时配置
    state.notifier.update_config(Some(email_config.clone()));

    // 持久化到 config.json
    if let Some(config_path) = &state.config_path {
        match persist_email_config(config_path, Some(&email_config)) {
            Ok(_) => {}
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(SuccessResponse {
                        success: false,
                        message: format!("配置已生效但持久化失败: {}", e),
                    }),
                )
                    .into_response();
            }
        }
    }

    Json(SuccessResponse::new("邮件配置已更新")).into_response()
}

/// DELETE /api/admin/email/config
/// 删除邮件配置
pub async fn delete_email_config(State(state): State<AdminState>) -> impl IntoResponse {
    // 清除运行时配置
    state.notifier.update_config(None);

    // 持久化到 config.json
    if let Some(config_path) = &state.config_path {
        match persist_email_config(config_path, None) {
            Ok(_) => {}
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(SuccessResponse {
                        success: false,
                        message: format!("配置已清除但持久化失败: {}", e),
                    }),
                )
                    .into_response();
            }
        }
    }

    Json(SuccessResponse::new("邮件配置已删除")).into_response()
}

/// 持久化邮件配置到 config.json
fn persist_email_config(
    config_path: &std::path::Path,
    email_config: Option<&crate::model::config::EmailConfig>,
) -> anyhow::Result<()> {
    use crate::model::config::Config;

    let mut config = Config::load(config_path)?;
    config.email = email_config.cloned();
    config.save()?;
    tracing::info!("邮件配置已持久化到: {}", config_path.display());
    Ok(())
}
