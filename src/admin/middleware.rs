//! Admin API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use super::proxy_pool::ProxyPool;
use super::service::AdminService;
use super::types::AdminErrorResponse;
use crate::common::auth;

/// Admin API 共享状态
#[derive(Clone)]
pub struct AdminState {
    /// Admin API 密钥
    pub admin_api_key: String,
    /// Admin 服务
    pub service: Arc<AdminService>,
    /// 代理池
    proxy_pool: Arc<ProxyPool>,
}

impl AdminState {
    pub fn new(admin_api_key: impl Into<String>, service: AdminService, proxy_pool: ProxyPool) -> Self {
        Self {
            admin_api_key: admin_api_key.into(),
            service: Arc::new(service),
            proxy_pool: Arc::new(proxy_pool),
        }
    }

    /// 获取代理池引用
    pub fn proxy_pool(&self) -> &ProxyPool {
        &self.proxy_pool
    }
}

/// Admin API 认证中间件
pub async fn admin_auth_middleware(
    State(state): State<AdminState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let api_key = auth::extract_api_key(&request);

    match api_key {
        Some(key) if auth::constant_time_eq(&key, &state.admin_api_key) => next.run(request).await,
        _ => {
            let error = AdminErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}
