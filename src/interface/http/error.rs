//! KiroError → axum::Response 的统一映射
//!
//! 取代 handlers.rs 中基于 `err_str.contains` 的字符串扫描错误识别，
//! 通过结构化 enum 匹配把 KiroError 转换为 Anthropic 兼容的错误响应。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};

use crate::domain::error::{KiroError, ProviderError, RefreshError};
use crate::interface::http::anthropic::dto::ErrorResponse;

/// 上游 body 在错误响应中携带的最大字节数（按 char_boundary 安全截断）。
const UPSTREAM_BODY_MAX_BYTES: usize = 512;

/// 把 KiroError 映射为 Anthropic 兼容的错误响应。
///
/// 字段名 `error.type` / `error.message` 与 master 历史响应保持一致。
/// 状态码与 `KiroError::http_status_hint` 同源。
///
/// 对每个 arm 同步写入 tracing：
/// - 客户端可重试/可纠正的 4xx → `warn!`
/// - 服务端 / 上游 / 默认兜底 → `error!`
pub fn kiro_error_response(err: &KiroError) -> Response {
    let (status, error_type, message) = match err {
        KiroError::Provider(ProviderError::ContextWindowFull) => {
            tracing::warn!(error = %err, "上游拒绝请求：上下文窗口已满（不应重试）");
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "Context window is full. Reduce conversation history, system prompt, or tools."
                    .to_string(),
            )
        }
        KiroError::Provider(ProviderError::InputTooLong) => {
            tracing::warn!(error = %err, "上游拒绝请求：输入过长（不应重试）");
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "Input is too long. Reduce the size of your messages.".to_string(),
            )
        }
        KiroError::Provider(ProviderError::BadRequest(msg)) => {
            tracing::warn!(error = %err, "请求被上游标记为 BadRequest");
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                msg.clone(),
            )
        }
        KiroError::Provider(ProviderError::EndpointResolution(msg)) => {
            tracing::error!(error = %err, "endpoint 解析失败");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                msg.clone(),
            )
        }
        KiroError::Provider(ProviderError::AllCredentialsExhausted { available, total }) => {
            tracing::error!(error = %err, available, total, "所有凭据都已耗尽");
            (
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("All {total} credentials exhausted ({available} available)"),
            )
        }
        KiroError::Provider(ProviderError::UpstreamHttp { status, body }) => {
            tracing::error!(error = %err, upstream_status = status, "上游返回错误状态码");
            (
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!(
                    "upstream HTTP {status}: {}",
                    truncate_utf8(body, UPSTREAM_BODY_MAX_BYTES)
                ),
            )
        }
        KiroError::Refresh(RefreshError::TokenInvalid) => {
            tracing::error!(error = %err, "refresh token 无效");
            (
                StatusCode::BAD_GATEWAY,
                "api_error",
                "Refresh token invalid".to_string(),
            )
        }
        // 默认 arm 覆盖 KiroError::{Endpoint, Network, Config, Storage, Decode} 等内部错误。
        // 内部错误的 Display 可能含文件路径 / URL 等运维信息，统一替换为 generic message，
        // 完整 error 链仅落入 tracing::error! 由运维查阅。
        _ => {
            tracing::error!(error = %err, kind = err.kind(), "Kiro API 调用失败（默认 arm）");
            (
                StatusCode::BAD_GATEWAY,
                "api_error",
                "Internal upstream error".to_string(),
            )
        }
    };

    (status, Json(ErrorResponse::new(error_type, message))).into_response()
}

/// 在 char boundary 上截断字符串，避免破坏多字节字符。
fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...(truncated)", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    async fn body_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn kiro_error_into_response_context_window_full_400_with_invalid_request_error_type() {
        let err: KiroError = ProviderError::ContextWindowFull.into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Context window is full"),
            "got: {json}"
        );
    }

    #[tokio::test]
    async fn kiro_error_into_response_input_too_long_400() {
        let err: KiroError = ProviderError::InputTooLong.into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Input is too long"),
            "got: {json}"
        );
    }

    #[tokio::test]
    async fn kiro_error_into_response_all_credentials_exhausted_502_with_api_error_type() {
        let err: KiroError = ProviderError::AllCredentialsExhausted {
            available: 1,
            total: 4,
        }
        .into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "api_error");
        let msg = json["error"]["message"].as_str().unwrap();
        assert!(msg.contains('1') && msg.contains('4'), "got: {msg}");
    }

    #[tokio::test]
    async fn kiro_error_into_response_endpoint_resolution_503() {
        let err: KiroError = ProviderError::EndpointResolution("ide endpoint missing".into()).into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"]["type"], "api_error");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("ide endpoint missing"),
            "got: {json}"
        );
    }

    #[tokio::test]
    async fn kiro_error_into_response_bad_request_400_with_message() {
        let err: KiroError = ProviderError::BadRequest("invalid model XYZ".into()).into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
        assert_eq!(json["error"]["message"], "invalid model XYZ");
    }

    #[tokio::test]
    async fn kiro_error_into_response_upstream_http_502() {
        let err: KiroError = ProviderError::UpstreamHttp {
            status: 503,
            body: "upstream is down".into(),
        }
        .into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "api_error");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("upstream is down"),
            "got: {json}"
        );
    }

    #[tokio::test]
    async fn kiro_error_into_response_upstream_http_truncates_long_body() {
        let body = "x".repeat(2048);
        let err: KiroError = ProviderError::UpstreamHttp {
            status: 502,
            body: body.clone(),
        }
        .into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        let msg = json["error"]["message"].as_str().unwrap();
        // 截断到 512 字节加少量包装；原 body 2048，结果 message 长度应远小于 2048
        assert!(msg.len() < body.len(), "expect truncation; got len={}", msg.len());
    }

    #[tokio::test]
    async fn kiro_error_into_response_refresh_token_invalid_502() {
        let err: KiroError = RefreshError::TokenInvalid.into();
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "api_error");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Refresh token invalid"),
            "got: {json}"
        );
    }

    #[tokio::test]
    async fn kiro_error_into_response_default_502() {
        let err = KiroError::Endpoint("unexpected".into());
        let (status, json) = body_json(kiro_error_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "api_error");
        // 默认 arm 应使用通用 message 而非 err.to_string()，避免内部信息泄漏。
        assert_eq!(json["error"]["message"], "Internal upstream error");
    }

    #[test]
    fn truncate_utf8_returns_empty_for_empty_input() {
        assert_eq!(truncate_utf8("", 0), "");
        assert_eq!(truncate_utf8("", 100), "");
    }

    #[test]
    fn truncate_utf8_returns_truncation_marker_when_max_bytes_zero() {
        assert_eq!(truncate_utf8("hello", 0), "...(truncated)");
    }

    #[test]
    fn truncate_utf8_returns_full_when_within_limit() {
        assert_eq!(truncate_utf8("hello", 5), "hello");
        assert_eq!(truncate_utf8("hello", 100), "hello");
    }

    #[test]
    fn truncate_utf8_falls_back_to_char_boundary_for_multibyte() {
        // "中文" 是 2 个字符，每个 3 字节，共 6 字节
        // max_bytes=4 落在第二个字符 "文" 的 2/3 字节处，应回退到 boundary=3
        let result = truncate_utf8("中文", 4);
        assert_eq!(result, "中...(truncated)");

        // max_bytes=2 落在第一个字符 "中" 的 1/3 字节处，应回退到 boundary=0
        let result = truncate_utf8("中文", 2);
        assert_eq!(result, "...(truncated)");
    }
}
