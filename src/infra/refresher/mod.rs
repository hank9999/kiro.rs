//! Token 刷新策略：Social / Idc / ApiKey
//!
//! 公共纯函数（HTTP 构造、错误判定、字段提取）暴露在本模块，refresh() 主体仅做
//! "发请求 + 调纯函数 + 装配 RefreshOutcome" 的薄壳；HTTP 端到端行为靠 Phase 8 冒烟。

#![allow(dead_code)]

pub mod api_key;
pub mod idc;
pub mod social;

pub use api_key::ApiKeyRefresher;
pub use idc::IdcRefresher;
pub use social::SocialRefresher;

use chrono::{Duration, Utc};
use http::StatusCode;
use serde::Deserialize;

use crate::config::Config;
use crate::domain::credential::Credential;
use crate::domain::error::{KiroError, RefreshError};
use crate::domain::token::RefreshOutcome;
use crate::infra::http::client::{ProxyConfig, build_client};

/// 把上游 refresh 端点的非 2xx HTTP 状态映射到 RefreshError
///
/// 400 + body 含 `"invalid_grant"` → TokenInvalid（refreshToken 永久失效）；
/// 401 → Unauthorized；403 → Forbidden；429 → RateLimited；
/// 5xx 与其他 4xx → ServerError（携带原 status）。
pub fn classify_refresh_http_error(status: StatusCode, body: &str) -> RefreshError {
    let s = status.as_u16();
    if s == 400 && body.contains("\"invalid_grant\"") {
        return RefreshError::TokenInvalid;
    }
    match s {
        401 => RefreshError::Unauthorized,
        403 => RefreshError::Forbidden,
        429 => RefreshError::RateLimited,
        _ => RefreshError::ServerError(status),
    }
}

/// 构造 Social refresh 请求体（含 refreshToken 字段）
pub fn build_social_request_body(cred: &Credential) -> Result<String, RefreshError> {
    let refresh_token = cred
        .refresh_token
        .as_deref()
        .ok_or(RefreshError::Unauthorized)?;
    let body = serde_json::json!({ "refreshToken": refresh_token });
    Ok(body.to_string())
}

/// 构造 IdC refresh 请求体（clientId / clientSecret / refreshToken / grantType）
///
/// 缺 refreshToken / clientId / clientSecret 任一字段都返回 Unauthorized。
pub fn build_idc_request_body(cred: &Credential) -> Result<String, RefreshError> {
    let refresh_token = cred
        .refresh_token
        .as_deref()
        .ok_or(RefreshError::Unauthorized)?;
    let client_id = cred
        .client_id
        .as_deref()
        .ok_or(RefreshError::Unauthorized)?;
    let client_secret = cred
        .client_secret
        .as_deref()
        .ok_or(RefreshError::Unauthorized)?;
    let body = serde_json::json!({
        "clientId": client_id,
        "clientSecret": client_secret,
        "refreshToken": refresh_token,
        "grantType": "refresh_token",
    });
    Ok(body.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    profile_arn: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// 解析 refresh 响应 JSON 为 RefreshOutcome
///
/// `expiresIn`（秒）转换为绝对时间 `expires_at`（RFC3339）。
pub fn parse_refresh_response(json: &str) -> Result<RefreshOutcome, RefreshError> {
    let raw: RawRefreshResponse =
        serde_json::from_str(json).map_err(RefreshError::MalformedResponse)?;
    let expires_at = raw
        .expires_in
        .map(|secs| (Utc::now() + Duration::seconds(secs)).to_rfc3339());
    Ok(RefreshOutcome {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        profile_arn: raw.profile_arn,
        expires_at,
    })
}

/// 根据全局配置 + 凭据级 proxy 构造 reqwest::Client
pub(crate) fn build_refresh_client(
    cred: &Credential,
    config: &Config,
) -> Result<reqwest::Client, RefreshError> {
    let global_proxy = config.proxy.proxy_url.as_deref().map(|url| {
        let mut p = ProxyConfig::new(url);
        if let (Some(u), Some(pw)) =
            (&config.proxy.proxy_username, &config.proxy.proxy_password)
        {
            p = p.with_auth(u, pw);
        }
        p
    });
    let effective_proxy = cred.effective_proxy(global_proxy.as_ref());
    build_client(effective_proxy.as_ref(), 60, config.net.tls_backend).map_err(|e| match e {
        KiroError::Network(re) => RefreshError::Network(re),
        _ => RefreshError::ServerError(StatusCode::INTERNAL_SERVER_ERROR),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_400_with_invalid_grant_is_token_invalid() {
        let body = r#"{"error":"invalid_grant","error_description":"Invalid refresh token provided"}"#;
        assert!(matches!(
            classify_refresh_http_error(StatusCode::BAD_REQUEST, body),
            RefreshError::TokenInvalid
        ));
    }

    #[test]
    fn classify_400_without_invalid_grant_is_server_error() {
        let body = r#"{"error":"some_other"}"#;
        assert!(matches!(
            classify_refresh_http_error(StatusCode::BAD_REQUEST, body),
            RefreshError::ServerError(s) if s.as_u16() == 400
        ));
    }

    #[test]
    fn classify_401_is_unauthorized() {
        assert!(matches!(
            classify_refresh_http_error(StatusCode::UNAUTHORIZED, ""),
            RefreshError::Unauthorized
        ));
    }

    #[test]
    fn classify_403_is_forbidden() {
        assert!(matches!(
            classify_refresh_http_error(StatusCode::FORBIDDEN, ""),
            RefreshError::Forbidden
        ));
    }

    #[test]
    fn classify_429_is_rate_limited() {
        assert!(matches!(
            classify_refresh_http_error(StatusCode::TOO_MANY_REQUESTS, ""),
            RefreshError::RateLimited
        ));
    }

    #[test]
    fn classify_5xx_is_server_error() {
        for s in [500u16, 502, 503, 504] {
            let status = StatusCode::from_u16(s).unwrap();
            assert!(matches!(
                classify_refresh_http_error(status, ""),
                RefreshError::ServerError(returned) if returned.as_u16() == s
            ));
        }
    }

    #[test]
    fn build_social_body_contains_refresh_token() {
        let cred = Credential {
            refresh_token: Some("rt-abc".to_string()),
            ..Default::default()
        };
        let body = build_social_request_body(&cred).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["refreshToken"], "rt-abc");
    }

    #[test]
    fn build_social_body_missing_refresh_token_returns_unauthorized() {
        let cred = Credential::default();
        assert!(matches!(
            build_social_request_body(&cred),
            Err(RefreshError::Unauthorized)
        ));
    }

    #[test]
    fn build_idc_body_contains_all_fields() {
        let cred = Credential {
            refresh_token: Some("rt".to_string()),
            client_id: Some("cid".to_string()),
            client_secret: Some("csec".to_string()),
            ..Default::default()
        };
        let body = build_idc_request_body(&cred).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["clientId"], "cid");
        assert_eq!(v["clientSecret"], "csec");
        assert_eq!(v["refreshToken"], "rt");
        assert_eq!(v["grantType"], "refresh_token");
    }

    #[test]
    fn build_idc_body_missing_client_id_returns_unauthorized() {
        let cred = Credential {
            refresh_token: Some("rt".to_string()),
            client_secret: Some("csec".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            build_idc_request_body(&cred),
            Err(RefreshError::Unauthorized)
        ));
    }

    #[test]
    fn build_idc_body_missing_client_secret_returns_unauthorized() {
        let cred = Credential {
            refresh_token: Some("rt".to_string()),
            client_id: Some("cid".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            build_idc_request_body(&cred),
            Err(RefreshError::Unauthorized)
        ));
    }

    #[test]
    fn parse_response_with_full_fields() {
        let json = r#"{
            "accessToken": "at-1",
            "refreshToken": "rt-2",
            "profileArn": "arn:aws:codewhisperer:us-east-1:123:profile/X",
            "expiresIn": 3600
        }"#;
        let outcome = parse_refresh_response(json).unwrap();
        assert_eq!(outcome.access_token, "at-1");
        assert_eq!(outcome.refresh_token.as_deref(), Some("rt-2"));
        assert_eq!(
            outcome.profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:us-east-1:123:profile/X")
        );
        assert!(outcome.expires_at.is_some());
    }

    #[test]
    fn parse_response_minimal_only_access_token() {
        let json = r#"{"accessToken":"at-only"}"#;
        let outcome = parse_refresh_response(json).unwrap();
        assert_eq!(outcome.access_token, "at-only");
        assert!(outcome.refresh_token.is_none());
        assert!(outcome.profile_arn.is_none());
        assert!(outcome.expires_at.is_none());
    }

    #[test]
    fn parse_response_missing_access_token_returns_malformed() {
        let json = r#"{"refreshToken":"rt"}"#;
        assert!(matches!(
            parse_refresh_response(json),
            Err(RefreshError::MalformedResponse(_))
        ));
    }

    #[test]
    fn parse_response_invalid_json_returns_malformed() {
        let json = "not-json";
        assert!(matches!(
            parse_refresh_response(json),
            Err(RefreshError::MalformedResponse(_))
        ));
    }
}
