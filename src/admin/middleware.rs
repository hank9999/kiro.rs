//! Admin API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use super::jwt;
use super::service::AdminService;
use super::types::AdminErrorResponse;

/// Admin API 共享状态
#[derive(Clone)]
pub struct AdminState {
    /// Admin API 密钥
    pub admin_api_key: String,
    /// Admin 服务
    pub service: Arc<AdminService>,
}

impl AdminState {
    pub fn new(admin_api_key: impl Into<String>, service: AdminService) -> Self {
        Self {
            admin_api_key: admin_api_key.into(),
            service: Arc::new(service),
        }
    }
}

/// Admin API 认证中间件
pub async fn admin_auth_middleware(
    State(state): State<AdminState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(token) => match jwt::verify_token(token, &state.admin_api_key) {
            Ok(_) => next.run(request).await,
            Err(_) => {
                let error = AdminErrorResponse::new(
                    "authentication_error",
                    "Invalid or expired token",
                );
                (StatusCode::UNAUTHORIZED, Json(error)).into_response()
            }
        },
        None => {
            let error = AdminErrorResponse::new(
                "authentication_error",
                "Missing authorization token",
            );
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}
