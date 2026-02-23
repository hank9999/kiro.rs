//! Admin 认证相关处理器
//!
//! 提供登录和 CAPTCHA 验证端点

use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::Json,
};
use base64::{engine::general_purpose, Engine as _};

use super::{
    captcha::{CaptchaError, CaptchaStore},
    jwt,
    types::{AdminErrorResponse, CaptchaResponse, LoginRequest, LoginResponse},
};
use crate::common::auth;

/// GET /api/auth/captcha
///
/// 生成 CAPTCHA 图像
pub async fn get_captcha(
    State((_, store)): State<(String, Arc<CaptchaStore>)>,
) -> Result<Json<CaptchaResponse>, (StatusCode, Json<AdminErrorResponse>)> {
    let (token, png_bytes) = store.generate();

    // 将 PNG 编码为 base64 data URL
    let base64_image = general_purpose::STANDARD.encode(&png_bytes);
    let data_url = format!("data:image/png;base64,{}", base64_image);

    Ok(Json(CaptchaResponse {
        token,
        image: data_url,
    }))
}

/// POST /api/auth/login
///
/// 验证 API Key 和 CAPTCHA，返回 JWT Token
pub async fn login(
    State((admin_key, store)): State<(String, Arc<CaptchaStore>)>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<AdminErrorResponse>)> {
    let ip = addr.ip().to_string();

    // 1. 检查速率限制
    if let Err(e) = store.check_rate_limit(&ip) {
        return Err(handle_captcha_error(e));
    }

    // 2. 验证 CAPTCHA
    if let Err(e) = store.verify(&payload.captcha_token, &payload.captcha_answer) {
        return Err(handle_captcha_error(e));
    }

    // 3. 验证 API Key (使用常量时间比较防止时序攻击)
    if !auth::constant_time_eq(&payload.api_key, &admin_key) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(AdminErrorResponse::new(
                "invalid_credentials",
                "Invalid credentials",
            )),
        ));
    }

    // 4. 生成 JWT Token
    let (token, expires_in) = jwt::generate_token(&admin_key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(AdminErrorResponse::internal_error(format!(
                "Failed to generate token: {}",
                e
            ))),
        )
    })?;

    // 登录成功，返回 token
    Ok(Json(LoginResponse {
        success: true,
        message: "Login successful".to_string(),
        token,
        expires_in,
    }))
}

/// 处理 CAPTCHA 错误
fn handle_captcha_error(error: CaptchaError) -> (StatusCode, Json<AdminErrorResponse>) {
    match error {
        CaptchaError::TooManyAttempts => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(AdminErrorResponse::new(
                "rate_limit_exceeded",
                "Too many login attempts. Please try again later.",
            )),
        ),
        CaptchaError::TokenNotFound => (
            StatusCode::BAD_REQUEST,
            Json(AdminErrorResponse::new(
                "invalid_captcha",
                "Invalid or expired CAPTCHA token",
            )),
        ),
        CaptchaError::AnswerIncorrect => (
            StatusCode::BAD_REQUEST,
            Json(AdminErrorResponse::new(
                "invalid_credentials",
                "Invalid credentials",
            )),
        ),
    }
}
