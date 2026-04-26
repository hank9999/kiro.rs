//! RetryPolicy + RetryDecision（占位，Phase 3 实现 DefaultRetryPolicy）

#![allow(dead_code)]

use std::time::Duration;

use http::StatusCode;

use crate::domain::error::ProviderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledReason {
    /// 用户通过 admin API 主动禁用
    Manual,
    /// 上游 API 调用失败累计达到阈值（可由 heal 自愈）
    TooManyFailures,
    /// 刷新 Token 失败累计达到阈值（确定性失败，不参与自愈）
    TooManyRefreshFailures,
    QuotaExceeded,
    InvalidRefreshToken,
    InvalidConfig,
}

#[derive(Debug)]
pub enum RetryDecision {
    Retry { backoff: Duration },
    FailoverCredential,
    ForceRefresh,
    DisableCredential(DisabledReason),
    Fail(ProviderError),
    Success,
}

pub trait RetryPolicy: Send + Sync {
    fn decide(&self, status: StatusCode, body: &str, attempt: usize) -> RetryDecision;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 编译期断言：DisabledReason 必须包含 Manual + TooManyRefreshFailures 两个变体
    #[test]
    fn disabled_reason_includes_manual_and_too_many_refresh_failures() {
        let manual: DisabledReason = DisabledReason::Manual;
        let too_many_refresh: DisabledReason = DisabledReason::TooManyRefreshFailures;
        // 通过 pattern match 强制变体可见
        let names: Vec<&'static str> = [manual, too_many_refresh]
            .into_iter()
            .map(|r| match r {
                DisabledReason::Manual => "Manual",
                DisabledReason::TooManyRefreshFailures => "TooManyRefreshFailures",
                DisabledReason::TooManyFailures => "TooManyFailures",
                DisabledReason::QuotaExceeded => "QuotaExceeded",
                DisabledReason::InvalidRefreshToken => "InvalidRefreshToken",
                DisabledReason::InvalidConfig => "InvalidConfig",
            })
            .collect();
        assert_eq!(names, vec!["Manual", "TooManyRefreshFailures"]);
    }
}
