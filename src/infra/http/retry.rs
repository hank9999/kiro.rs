//! DefaultRetryPolicy：状态码 → RetryDecision 映射 + 指数退避

use std::time::Duration;

use http::StatusCode;

use crate::domain::error::ProviderError;
use crate::domain::retry::{DisabledReason, RetryDecision, RetryPolicy};

/// 单凭据连续失败次数上限
pub const MAX_RETRIES_PER_CREDENTIAL: usize = 3;
/// 单请求总重试次数上限
pub const MAX_TOTAL_RETRIES: usize = 9;

/// 指数退避：200ms × 2^attempt，上限 2s，± 25% jitter
pub fn next_backoff(attempt: usize) -> Duration {
    const BASE_MS: u64 = 200;
    const MAX_MS: u64 = 2_000;
    let exp = BASE_MS.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
    let backoff = exp.min(MAX_MS);
    let jitter_max = (backoff / 4).max(1);
    let jitter = fastrand::u64(0..=jitter_max);
    Duration::from_millis(backoff.saturating_add(jitter))
}

#[derive(Default)]
pub struct DefaultRetryPolicy;

impl DefaultRetryPolicy {
    pub fn new() -> Self {
        Self
    }
}

impl RetryPolicy for DefaultRetryPolicy {
    fn decide(&self, status: StatusCode, body: &str, attempt: usize) -> RetryDecision {
        let s = status.as_u16();

        if status.is_success() {
            return RetryDecision::Success;
        }

        // 402 + MONTHLY_REQUEST_COUNT → 永久禁用（QuotaExceeded）
        if s == 402 && body.contains("MONTHLY_REQUEST_COUNT") {
            return RetryDecision::DisableCredential(DisabledReason::QuotaExceeded);
        }

        // 401/403 + bearer token invalid → 强制刷新（每凭据仅一次）
        if matches!(s, 401 | 403)
            && body.contains("The bearer token included in the request is invalid")
        {
            return RetryDecision::ForceRefresh;
        }

        // 401/403 普通 → 失败转移
        if matches!(s, 401 | 403) {
            return RetryDecision::FailoverCredential;
        }

        // 408/429/5xx → 退避重试
        if matches!(s, 408 | 429) || status.is_server_error() {
            return RetryDecision::Retry {
                backoff: next_backoff(attempt),
            };
        }

        // 其他 4xx（含 400）→ 直接失败
        if status.is_client_error() {
            return RetryDecision::Fail(ProviderError::UpstreamHttp {
                status: s,
                body: body.to_string(),
            });
        }

        // 兜底
        RetryDecision::Fail(ProviderError::UpstreamHttp {
            status: s,
            body: body.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> DefaultRetryPolicy {
        DefaultRetryPolicy::new()
    }

    #[test]
    fn decide_200_is_success() {
        assert!(matches!(
            policy().decide(StatusCode::OK, "", 0),
            RetryDecision::Success
        ));
    }

    #[test]
    fn decide_400_is_fail() {
        let d = policy().decide(StatusCode::BAD_REQUEST, "bad request body", 0);
        match d {
            RetryDecision::Fail(ProviderError::UpstreamHttp { status, .. }) => {
                assert_eq!(status, 400);
            }
            other => panic!("期望 Fail，得到 {other:?}"),
        }
    }

    #[test]
    fn decide_401_with_bearer_invalid_is_force_refresh() {
        let body = "The bearer token included in the request is invalid";
        assert!(matches!(
            policy().decide(StatusCode::UNAUTHORIZED, body, 0),
            RetryDecision::ForceRefresh
        ));
    }

    #[test]
    fn decide_403_with_bearer_invalid_is_force_refresh() {
        let body = "The bearer token included in the request is invalid";
        assert!(matches!(
            policy().decide(StatusCode::FORBIDDEN, body, 0),
            RetryDecision::ForceRefresh
        ));
    }

    #[test]
    fn decide_401_generic_is_failover() {
        assert!(matches!(
            policy().decide(StatusCode::UNAUTHORIZED, "unauthorized", 0),
            RetryDecision::FailoverCredential
        ));
    }

    #[test]
    fn decide_403_generic_is_failover() {
        assert!(matches!(
            policy().decide(StatusCode::FORBIDDEN, "forbidden", 0),
            RetryDecision::FailoverCredential
        ));
    }

    #[test]
    fn decide_402_with_monthly_request_count_is_disable_quota() {
        let body = r#"{"reason":"MONTHLY_REQUEST_COUNT"}"#;
        let status = StatusCode::from_u16(402).unwrap();
        assert!(matches!(
            policy().decide(status, body, 0),
            RetryDecision::DisableCredential(DisabledReason::QuotaExceeded)
        ));
    }

    #[test]
    fn decide_408_429_5xx_is_retry() {
        for s in [408u16, 429, 500, 502, 503, 504] {
            let status = StatusCode::from_u16(s).unwrap();
            assert!(
                matches!(
                    policy().decide(status, "", 0),
                    RetryDecision::Retry { .. }
                ),
                "status {s} 应返回 Retry"
            );
        }
    }

    #[test]
    fn decide_other_4xx_is_fail() {
        for s in [404u16, 405, 410] {
            let status = StatusCode::from_u16(s).unwrap();
            match policy().decide(status, "", 0) {
                RetryDecision::Fail(_) => {}
                other => panic!("status {s} 应返回 Fail，得到 {other:?}"),
            }
        }
    }

    #[test]
    fn next_backoff_grows_exponentially_capped_at_2s() {
        // attempt=0 → ~200ms（含 jitter 上限到 250ms）
        let d0 = next_backoff(0);
        assert!(d0.as_millis() >= 200 && d0.as_millis() <= 250);
        // attempt=1 → ~400ms（含 jitter 上限到 500ms）
        let d1 = next_backoff(1);
        assert!(d1.as_millis() >= 400 && d1.as_millis() <= 500);
        // attempt=2 → ~800ms
        let d2 = next_backoff(2);
        assert!(d2.as_millis() >= 800 && d2.as_millis() <= 1000);
        // attempt=10 → 上限 2000ms + 25% jitter → 上限 2500ms
        let d10 = next_backoff(10);
        assert!(d10.as_millis() >= 2000 && d10.as_millis() <= 2500);
        // attempt=20 同样上限
        let d20 = next_backoff(20);
        assert!(d20.as_millis() <= 2500);
    }
}
