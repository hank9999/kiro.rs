//! 代理池 API 处理器

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};

use super::{
    middleware::AdminState,
    types::{
        AddProxyRequest, BatchAddProxyRequest, ProxyPoolListResponse, ProxyTestResponse,
        SetDisabledRequest, SuccessResponse, UpdateProxyRequest,
    },
};

/// GET /api/admin/proxy-pool
pub async fn list_proxies(State(state): State<AdminState>) -> impl IntoResponse {
    let pool = state.proxy_pool();
    let proxies = pool.list();
    let available = pool.available_count();
    Json(ProxyPoolListResponse {
        total: proxies.len(),
        available,
        proxies,
    })
}

/// POST /api/admin/proxy-pool
pub async fn add_proxy(
    State(state): State<AdminState>,
    Json(payload): Json<AddProxyRequest>,
) -> impl IntoResponse {
    if payload.url.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": {"type": "invalid_request", "message": "代理 URL 不能为空"}})),
        )
            .into_response();
    }

    let pool = state.proxy_pool();
    let id = pool.add(
        payload.url.trim().to_string(),
        payload.username,
        payload.password,
        payload.label,
    );

    Json(serde_json::json!({
        "success": true,
        "message": format!("代理添加成功，ID: {}", id),
        "id": id
    }))
    .into_response()
}

/// POST /api/admin/proxy-pool/batch
pub async fn batch_add_proxies(
    State(state): State<AdminState>,
    Json(payload): Json<BatchAddProxyRequest>,
) -> impl IntoResponse {
    let pool = state.proxy_pool();
    let ids = pool.batch_add(&payload.lines);

    Json(serde_json::json!({
        "success": true,
        "message": format!("批量添加 {} 个代理", ids.len()),
        "ids": ids
    }))
}

/// PUT /api/admin/proxy-pool/:id
pub async fn update_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateProxyRequest>,
) -> impl IntoResponse {
    let pool = state.proxy_pool();
    match pool.update(id, payload.url, payload.username, payload.password, payload.label, payload.disabled) {
        Ok(_) => Json(SuccessResponse::new(format!("代理 #{} 已更新", id))).into_response(),
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": {"type": "not_found", "message": e}})),
        )
            .into_response(),
    }
}

/// DELETE /api/admin/proxy-pool/:id
pub async fn delete_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let pool = state.proxy_pool();
    match pool.delete(id) {
        Ok(_) => Json(SuccessResponse::new(format!("代理 #{} 已删除", id))).into_response(),
        Err(e) => {
            let status = if e.contains("不存在") {
                axum::http::StatusCode::NOT_FOUND
            } else {
                axum::http::StatusCode::BAD_REQUEST
            };
            (
                status,
                Json(serde_json::json!({"error": {"type": "invalid_request", "message": e}})),
            )
                .into_response()
        }
    }
}

/// POST /api/admin/proxy-pool/:id/disabled
pub async fn set_proxy_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    let pool = state.proxy_pool();
    match pool.set_disabled(id, payload.disabled) {
        Ok(_) => {
            let action = if payload.disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("代理 #{} 已{}", id, action))).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": {"type": "not_found", "message": e}})),
        )
            .into_response(),
    }
}

/// POST /api/admin/proxy-pool/:id/test
pub async fn test_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let pool = state.proxy_pool();
    let entry = match pool.get(id) {
        Some(e) => e,
        None => {
            return Json(ProxyTestResponse {
                success: false,
                message: format!("代理 #{} 不存在", id),
                latency_ms: None,
            })
            .into_response()
        }
    };

    // 构建代理客户端测试连通性
    let start = std::time::Instant::now();
    let result = test_proxy_connectivity(&entry.url, entry.username.as_deref(), entry.password.as_deref()).await;
    let latency = start.elapsed().as_millis() as u64;

    match result {
        Ok(_) => Json(ProxyTestResponse {
            success: true,
            message: format!("连接成功，延迟 {}ms", latency),
            latency_ms: Some(latency),
        })
        .into_response(),
        Err(e) => Json(ProxyTestResponse {
            success: false,
            message: format!("连接失败: {}", e),
            latency_ms: Some(latency),
        })
        .into_response(),
    }
}

/// 测试代理连通性（通过代理访问 httpbin）
async fn test_proxy_connectivity(
    proxy_url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(), String> {
    use reqwest::Proxy;

    let mut proxy = Proxy::all(proxy_url).map_err(|e| format!("代理 URL 无效: {}", e))?;

    if let (Some(user), Some(pass)) = (username, password) {
        proxy = proxy.basic_auth(user, pass);
    }

    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建客户端失败: {}", e))?;

    let resp = client
        .get("https://httpbin.org/ip")
        .send()
        .await
        .map_err(|e| format!("{}", e))?;

    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}
