//! Admin API 中间件
//!
//! Input: JWT, Database, 请求
//! Output: 认证状态、用户信息
//! Pos: Admin API 认证层

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use axum_extra::extract::CookieJar;

use super::service::AdminService;
use super::types::AdminErrorResponse;
use crate::db::Database;
use crate::jwt::{Claims, JwtManager};

/// Cookie 名称
pub const AUTH_COOKIE_NAME: &str = "kiro_auth";

/// Admin API 共享状态
#[derive(Clone)]
pub struct AdminState {
    /// 数据库
    pub db: Database,
    /// JWT 管理器
    pub jwt: JwtManager,
    /// Admin 服务
    pub service: Arc<AdminService>,
}

impl AdminState {
    pub fn new(db: Database, jwt: JwtManager, service: AdminService) -> Self {
        Self {
            db,
            jwt,
            service: Arc::new(service),
        }
    }
}

/// 从请求中提取 JWT token
fn extract_token(jar: &CookieJar, request: &Request<Body>) -> Option<String> {
    // 1. 优先从 Cookie 获取
    if let Some(cookie) = jar.get(AUTH_COOKIE_NAME) {
        return Some(cookie.value().to_string());
    }

    // 2. 从 Authorization header 获取
    if let Some(auth_header) = request.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }

    None
}

/// Admin API 认证中间件
pub async fn admin_auth_middleware(
    State(state): State<AdminState>,
    jar: CookieJar,
    request: Request<Body>,
    next: Next,
) -> Response {
    let token = extract_token(&jar, &request);

    match token {
        Some(t) => {
            match state.jwt.verify_token(&t) {
                Ok(claims) => {
                    // Token 有效，继续处理请求
                    // 可以将 claims 注入到请求扩展中
                    let mut request = request;
                    request.extensions_mut().insert(claims);
                    next.run(request).await
                }
                Err(_) => {
                    let error = AdminErrorResponse::authentication_error();
                    (StatusCode::UNAUTHORIZED, Json(error)).into_response()
                }
            }
        }
        None => {
            let error = AdminErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}

/// 从请求扩展中获取用户 Claims
pub fn get_claims(request: &Request<Body>) -> Option<&Claims> {
    request.extensions().get::<Claims>()
}
