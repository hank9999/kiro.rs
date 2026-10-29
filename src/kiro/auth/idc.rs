//! AWS SSO OIDC 设备授权登录流程
//!
//! 实现三步流程：
//! 1. 注册 OIDC 客户端（register_client）
//! 2. 发起设备授权，获取用户验证码（start_device_authorization）
//! 3. 轮询令牌端点，等待用户完成授权（poll_token）

use anyhow::Context;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::model::token_refresh::{
    CreateTokenRequest, CreateTokenResponse, OidcErrorResponse, RegisterClientRequest,
    RegisterClientResponse, StartDeviceAuthorizationRequest, StartDeviceAuthorizationResponse,
};
use crate::model::config::Config;

/// 设备授权轮询结果
#[derive(Debug)]
pub enum PollResult {
    /// 用户尚未完成授权，继续等待
    Pending,
    /// 授权成功，返回 token
    Success(CreateTokenResponse),
    /// 设备码已过期，需重新发起
    Expired,
    /// 其他错误
    Error(anyhow::Error),
}

/// AWS Builder ID / IAM Identity Center 的默认 Start URL
pub const BUILDER_ID_START_URL: &str = "https://view.awsapps.com/start";

/// Kiro IDE 使用的 OIDC 作用域
const KIRO_SCOPES: &[&str] = &[
    "sso:account:access",
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "openid",
    "profile",
    "email",
];

fn oidc_endpoint(region: &str) -> String {
    format!("https://oidc.{}.amazonaws.com", region)
}

/// 注册 OIDC 客户端
///
/// 每次发起设备授权前调用，获得 clientId 和 clientSecret。
/// 注册结果有过期时间（通常数天），但此处每次重新注册以保持简单。
pub async fn register_client(
    region: &str,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<RegisterClientResponse> {
    let url = format!("{}/client/register", oidc_endpoint(region));
    let client = build_client(proxy, 30, config.tls_backend)?;

    let body = RegisterClientRequest {
        client_name: "kiro-rs".to_string(),
        client_type: "public".to_string(),
        scopes: KIRO_SCOPES.iter().map(|s| s.to_string()).collect(),
        grant_types: vec![
            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
            "refresh_token".to_string(),
        ],
    };

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .header("host", format!("oidc.{}.amazonaws.com", region))
        .json(&body)
        .send()
        .await
        .context("注册 OIDC 客户端请求失败")?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("注册 OIDC 客户端失败 {}: {}", status, body_text);
    }

    resp.json::<RegisterClientResponse>()
        .await
        .context("解析注册响应失败")
}

/// 发起设备授权，返回供用户访问的验证码和 URL
pub async fn start_device_authorization(
    region: &str,
    start_url: &str,
    client_id: &str,
    client_secret: &str,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<StartDeviceAuthorizationResponse> {
    let url = format!("{}/device_authorization", oidc_endpoint(region));
    let client = build_client(proxy, 30, config.tls_backend)?;

    let body = StartDeviceAuthorizationRequest {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
        start_url: start_url.to_string(),
    };

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .header("host", format!("oidc.{}.amazonaws.com", region))
        .json(&body)
        .send()
        .await
        .context("发起设备授权请求失败")?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("发起设备授权失败 {}: {}", status, body_text);
    }

    resp.json::<StartDeviceAuthorizationResponse>()
        .await
        .context("解析设备授权响应失败")
}

/// 轮询一次令牌端点
///
/// 返回 `PollResult`，由调用方决定是否继续轮询。
pub async fn poll_token(
    region: &str,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> PollResult {
    let url = format!("{}/token", oidc_endpoint(region));
    let client = match build_client(proxy, 30, config.tls_backend) {
        Ok(c) => c,
        Err(e) => return PollResult::Error(e),
    };

    let body = CreateTokenRequest {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
        grant_type: "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        device_code: device_code.to_string(),
    };

    let resp = match client
        .post(&url)
        .header("content-type", "application/json")
        .header("host", format!("oidc.{}.amazonaws.com", region))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return PollResult::Error(e.into()),
    };

    let status = resp.status();

    if status.is_success() {
        return match resp.json::<CreateTokenResponse>().await {
            Ok(token) => PollResult::Success(token),
            Err(e) => PollResult::Error(e.into()),
        };
    }

    let body_text = match resp.text().await {
        Ok(t) => t,
        Err(e) => return PollResult::Error(e.into()),
    };

    // 解析标准 OIDC 错误码
    if let Ok(err_resp) = serde_json::from_str::<OidcErrorResponse>(&body_text) {
        match err_resp.error.as_str() {
            "authorization_pending" => return PollResult::Pending,
            "slow_down" => return PollResult::Pending,
            "expired_token" => return PollResult::Expired,
            "access_denied" => {
                return PollResult::Error(anyhow::anyhow!(
                    "用户拒绝了授权请求"
                ))
            }
            _ => {}
        }
    }

    PollResult::Error(anyhow::anyhow!(
        "轮询令牌失败 {}: {}",
        status,
        body_text
    ))
}
