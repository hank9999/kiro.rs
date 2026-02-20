//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};

use super::{
    middleware::AdminState,
    types::{
    AddCredentialRequest, CredentialAccountInfoResponse, SetDisabledRequest,
    SetEnabledModelsRequest, SetLoadBalancingModeRequest, SetPriorityRequest,
    SetSummaryModelRequest, SummaryModelResponse, SuccessResponse, AVAILABLE_SUMMARY_MODELS,
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

/// POST /api/admin/credentials/:id/models
/// 设置凭据启用的模型列表
pub async fn set_credential_enabled_models(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetEnabledModelsRequest>,
) -> impl IntoResponse {
    match state.service.set_enabled_models(id, payload.enabled_models) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 模型开关已更新", id))).into_response(),
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

/// GET /api/admin/credentials/:id/account
/// 获取指定凭据的账号信息（套餐/用量/邮箱等）
pub async fn get_credential_account_info(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_account_info(id).await {
        Ok(response) => Json::<CredentialAccountInfoResponse>(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/stats
/// 获取指定凭据的统计详情
pub async fn get_credential_stats(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_credential_stats(id) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/stats/reset
/// 清空指定凭据统计
pub async fn reset_credential_stats(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_credential_stats(id).await {
        Ok(()) => Json(SuccessResponse::new(format!("凭据 #{} 统计已清空", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/stats/reset
/// 清空全部统计
pub async fn reset_all_stats(State(state): State<AdminState>) -> impl IntoResponse {
    match state.service.reset_all_stats().await {
        Ok(()) => Json(SuccessResponse::new("全部统计已清空")).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/credentials/:id
/// 删除指定凭据（并持久化到凭据文件）
pub async fn delete_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_credential(id).await {
        Ok(msg) => Json(SuccessResponse::new(msg)).into_response(),
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

/// GET /api/admin/settings/summary-model
/// 获取当前摘要模型设置
pub async fn get_summary_model(State(state): State<AdminState>) -> impl IntoResponse {
    let current_model = state
        .app_state
        .as_ref()
        .map(|s| s.get_summary_model())
        .unwrap_or_else(|| "claude-sonnet-4.5".to_string());

    Json(SummaryModelResponse {
        current_model,
        available_models: AVAILABLE_SUMMARY_MODELS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    })
}

/// POST /api/admin/settings/summary-model
/// 设置摘要模型
pub async fn set_summary_model(
    State(state): State<AdminState>,
    Json(payload): Json<SetSummaryModelRequest>,
) -> impl IntoResponse {
    // 验证模型是否在允许列表中
    if !AVAILABLE_SUMMARY_MODELS.contains(&payload.model.as_str()) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(super::types::AdminErrorResponse::invalid_request(format!(
                "不支持的模型: {}，可用模型: {:?}",
                payload.model, AVAILABLE_SUMMARY_MODELS
            ))),
        )
            .into_response();
    }

    // 设置模型
    if let Some(app_state) = &state.app_state {
        app_state.set_summary_model(&payload.model);
        Json(SuccessResponse::new(format!(
            "摘要模型已设置为: {}",
            payload.model
        )))
        .into_response()
    } else {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(super::types::AdminErrorResponse::internal_error(
                "AppState 未配置",
            )),
        )
            .into_response()
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
