//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};

use super::{
    middleware::AdminState,
    types::{
        ActivityQuery, AddApiKeyRequest, AddCredentialRequest, GenerateApiKeyRequest, LogsQuery,
        ProxyPoolDto, SetDisabledRequest, SetLoadBalancingModeRequest, SetPriorityRequest,
        SuccessResponse, TestProxyPoolRequest, UpdateApiKeyRequest, UpdateCredentialProxyRequest,
    },
};

/// GET /api/admin/credentials
/// 获取所有凭据状态
pub async fn get_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_all_credentials();
    Json(response)
}

/// GET /api/admin/models
/// 获取当前服务暴露的模型列表
pub async fn get_available_models(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_available_models())
}

/// GET /api/admin/activity
/// 获取最近请求活动
pub async fn get_request_activity(
    State(state): State<AdminState>,
    Query(query): Query<ActivityQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    Json(state.service.get_request_activity(limit))
}

/// GET /api/admin/logs
/// 获取最近日志
pub async fn get_recent_logs(
    State(state): State<AdminState>,
    Query(query): Query<LogsQuery>,
) -> impl IntoResponse {
    let lines = query.lines.unwrap_or(120).clamp(1, 500);
    Json(state.service.get_recent_logs(lines))
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

/// POST /api/admin/credentials/:id/refresh
/// 强制刷新凭据 Token
pub async fn force_refresh_token(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.force_refresh_token(id).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} Token 已强制刷新",
            id
        )))
        .into_response(),
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

// ============ API Key 管理处理器 ============

/// GET /api/admin/api-keys
/// 获取所有 API Keys
pub async fn get_api_keys(State(state): State<AdminState>) -> impl IntoResponse {
    match state.service.get_api_keys() {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/api-keys
/// 添加新的 API Key
pub async fn add_api_key(
    State(state): State<AdminState>,
    Json(payload): Json<AddApiKeyRequest>,
) -> impl IntoResponse {
    match state.service.add_api_key(payload) {
        Ok(key_info) => {
            // 重新加载 AppState 的 Keys
            if let Err(e) = state.app_state.reload_keys() {
                tracing::error!("重新加载 API Keys 失败: {}", e);
            }
            Json(key_info).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/api-keys/generate
/// 生成随机 API Key
pub async fn generate_api_key(
    State(state): State<AdminState>,
    Json(payload): Json<GenerateApiKeyRequest>,
) -> impl IntoResponse {
    match state.service.generate_api_key(payload) {
        Ok(response) => {
            // 重新加载 AppState 的 Keys
            if let Err(e) = state.app_state.reload_keys() {
                tracing::error!("重新加载 API Keys 失败: {}", e);
            }
            Json(response).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/api-keys/:id
/// 更新 API Key
pub async fn update_api_key(
    State(state): State<AdminState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateApiKeyRequest>,
) -> impl IntoResponse {
    match state.service.update_api_key(&id, payload) {
        Ok(_) => {
            // 重新加载 AppState 的 Keys
            if let Err(e) = state.app_state.reload_keys() {
                tracing::error!("重新加载 API Keys 失败: {}", e);
            }
            Json(SuccessResponse::new(format!("API Key {} 已更新", id))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/api-keys/:id
/// 删除 API Key
pub async fn delete_api_key(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.service.delete_api_key(&id) {
        Ok(_) => {
            // 重新加载 AppState 的 Keys
            if let Err(e) = state.app_state.reload_keys() {
                tracing::error!("重新加载 API Keys 失败: {}", e);
            }
            Json(SuccessResponse::new(format!("API Key {} 已删除", id))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

// ============ 代理池管理处理器 ============

/// GET /api/admin/proxy-pool
/// 获取当前代理池配置和运行时状态
pub async fn get_proxy_pool(State(state): State<AdminState>) -> impl IntoResponse {
    match state.service.get_proxy_pool() {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/proxy-pool
/// 更新代理池配置（持久化并热更新）
pub async fn update_proxy_pool(
    State(state): State<AdminState>,
    Json(payload): Json<ProxyPoolDto>,
) -> impl IntoResponse {
    match state.service.update_proxy_pool(payload) {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxy-pool/test
/// 测试代理池连通性
pub async fn test_proxy_pool(
    State(state): State<AdminState>,
    Json(payload): Json<TestProxyPoolRequest>,
) -> impl IntoResponse {
    match state.service.test_proxy_pool(payload).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/credentials/:id/proxy
/// 更新单个凭据的代理配置
pub async fn update_credential_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateCredentialProxyRequest>,
) -> impl IntoResponse {
    match state.service.update_credential_proxy(id, payload).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 代理已更新",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}
