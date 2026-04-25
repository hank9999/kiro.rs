//! RequestExecutor：通用 send + 退避 + 故障转移
//!
//! 同一份 execute() 处理 API 与 MCP 两种端点（kind 参数化），删除旧 KiroProvider 中
//! call_api_with_retry / call_mcp_with_retry 双胞胎。
//!
//! 内部循环顺序：
//! ```text
//! let ctx = pool.acquire(model).await?;
//! let endpoint = endpoints.resolve_for(&ctx.credentials)?;
//! let url = endpoint.url(kind, &rctx);
//! let body = endpoint.transform_body(kind, body, &rctx);
//! let request = endpoint.decorate(kind, base, &rctx);
//! let response = client.send(request).await;
//! let decision = policy.decide(status, &body_text, attempt);
//! apply(decision, &mut ctx, pool); // report / refresh / retry / failover / fail
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use reqwest::Client;

use crate::config::Config;
use crate::domain::endpoint::RequestContext;
use crate::domain::error::ProviderError;
use crate::domain::retry::{DisabledReason, RetryDecision, RetryPolicy};
use crate::infra::endpoint::EndpointRegistry;
use crate::infra::http::client::{ProxyConfig, build_client};
use crate::service::credential_pool::{CallContext, CredentialPool};

use super::retry::{MAX_RETRIES_PER_CREDENTIAL, MAX_TOTAL_RETRIES, next_backoff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Api,
    Mcp,
}

/// 单次 attempt 的处置（compute_attempt_outcome 输出，executor 应用）
#[derive(Debug)]
pub enum AttemptOutcome {
    /// 报告成功，返回 response
    SucceedAndReturn,
    /// 永久失败，立即返回 ProviderError
    FailWith(ProviderError),
    /// 禁用当前凭据后继续 retry
    DisableAndContinue(DisabledReason),
    /// 失败转移到下一凭据
    FailoverContinue,
    /// 强制刷新当前凭据 token 后继续
    ForceRefreshContinue,
    /// 退避指定时间后继续
    RetryAfter(Duration),
}

/// 把 RetryDecision 映射到 AttemptOutcome（纯函数，无副作用）
///
/// `force_refreshed` 用于跟踪每凭据已 force-refresh 过的 id；如果 ForceRefresh 决策对
/// 已 refresh 过的凭据再触发，降级为 FailoverContinue。
pub fn compute_attempt_outcome(
    decision: RetryDecision,
    cred_id: u64,
    force_refreshed: &HashSet<u64>,
) -> AttemptOutcome {
    match decision {
        RetryDecision::Success => AttemptOutcome::SucceedAndReturn,
        RetryDecision::Fail(e) => AttemptOutcome::FailWith(e),
        RetryDecision::DisableCredential(reason) => AttemptOutcome::DisableAndContinue(reason),
        RetryDecision::FailoverCredential => AttemptOutcome::FailoverContinue,
        RetryDecision::Retry { backoff } => AttemptOutcome::RetryAfter(backoff),
        RetryDecision::ForceRefresh => {
            if force_refreshed.contains(&cred_id) {
                AttemptOutcome::FailoverContinue
            } else {
                AttemptOutcome::ForceRefreshContinue
            }
        }
    }
}

pub struct RequestExecutor {
    config: Arc<Config>,
    global_proxy: Option<ProxyConfig>,
    client_cache: Mutex<HashMap<Option<ProxyConfig>, Client>>,
}

impl RequestExecutor {
    pub fn new(config: Arc<Config>, global_proxy: Option<ProxyConfig>) -> Self {
        let mut cache = HashMap::new();
        // 预热默认 client
        if let Ok(c) = build_client(global_proxy.as_ref(), 720, config.net.tls_backend) {
            cache.insert(global_proxy.clone(), c);
        }
        Self {
            config,
            global_proxy,
            client_cache: Mutex::new(cache),
        }
    }

    fn client_for(&self, ctx: &CallContext) -> Result<Client, ProviderError> {
        let proxy = ctx.credentials.effective_proxy(self.global_proxy.as_ref());
        let mut cache = self.client_cache.lock();
        if let Some(c) = cache.get(&proxy) {
            return Ok(c.clone());
        }
        let new = build_client(proxy.as_ref(), 720, self.config.net.tls_backend).map_err(|e| {
            ProviderError::EndpointResolution(format!("build HTTP client failed: {e}"))
        })?;
        cache.insert(proxy, new.clone());
        Ok(new)
    }

    pub async fn execute(
        &self,
        kind: EndpointKind,
        body: &str,
        model: Option<&str>,
        pool: &CredentialPool,
        endpoints: &EndpointRegistry,
        policy: &dyn RetryPolicy,
    ) -> Result<reqwest::Response, ProviderError> {
        let total = pool.total_count();
        let max_retries = (total * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        if max_retries == 0 {
            return Err(ProviderError::AllCredentialsExhausted {
                available: 0,
                total,
            });
        }

        let mut last_error: Option<ProviderError> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            let ctx = match pool.acquire(model).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    break;
                }
            };

            let endpoint = match endpoints.resolve_for(&ctx.credentials) {
                Ok(ep) => ep,
                Err(e) => {
                    pool.report_failure(ctx.id);
                    last_error = Some(e);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &ctx.machine_id,
                config: &self.config,
            };

            let url = match kind {
                EndpointKind::Api => endpoint.api_url(&rctx),
                EndpointKind::Mcp => endpoint.mcp_url(&rctx),
            };
            let body_t = match kind {
                EndpointKind::Api => endpoint.transform_api_body(body, &rctx),
                EndpointKind::Mcp => endpoint.transform_mcp_body(body, &rctx),
            };

            let client = self.client_for(&ctx)?;
            let base = client
                .post(&url)
                .body(body_t)
                .header("content-type", "application/json")
                .header("Connection", "close");
            let request = match kind {
                EndpointKind::Api => endpoint.decorate_api(base, &rctx),
                EndpointKind::Mcp => endpoint.decorate_mcp(base, &rctx),
            };

            let response = match request.send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        max_retries,
                        "请求发送失败（网络），将退避重试: {e}"
                    );
                    last_error = Some(ProviderError::UpstreamHttp {
                        status: 0,
                        body: e.to_string(),
                    });
                    if attempt + 1 < max_retries {
                        tokio::time::sleep(next_backoff(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();
            if status.is_success() {
                pool.report_success(ctx.id);
                return Ok(response);
            }

            let body_text = response.text().await.unwrap_or_default();
            let decision = policy.decide(status, &body_text, attempt);
            let outcome = compute_attempt_outcome(decision, ctx.id, &force_refreshed);

            match outcome {
                AttemptOutcome::SucceedAndReturn => {
                    // 不可达：is_success 已在前面处理
                    pool.report_success(ctx.id);
                    return Err(ProviderError::UpstreamHttp {
                        status: status.as_u16(),
                        body: body_text,
                    });
                }
                AttemptOutcome::FailWith(e) => return Err(e),
                AttemptOutcome::DisableAndContinue(reason) => {
                    let has_available = match reason {
                        DisabledReason::QuotaExceeded => pool.report_quota_exhausted(ctx.id),
                        DisabledReason::InvalidRefreshToken => {
                            pool.report_refresh_token_invalid(ctx.id)
                        }
                        _ => pool.report_failure(ctx.id),
                    };
                    last_error = Some(ProviderError::UpstreamHttp {
                        status: status.as_u16(),
                        body: body_text,
                    });
                    if !has_available {
                        // pool.report_*  返回 true = disabled；当所有都 disabled 时下一轮 acquire 会触发自愈或 exhausted
                        // 这里继续 loop 让 acquire 决定
                    }
                    continue;
                }
                AttemptOutcome::FailoverContinue => {
                    pool.report_failure(ctx.id);
                    last_error = Some(ProviderError::UpstreamHttp {
                        status: status.as_u16(),
                        body: body_text,
                    });
                    continue;
                }
                AttemptOutcome::ForceRefreshContinue => {
                    force_refreshed.insert(ctx.id);
                    if pool.force_refresh(ctx.id).await.is_err() {
                        pool.report_failure(ctx.id);
                    }
                    last_error = Some(ProviderError::UpstreamHttp {
                        status: status.as_u16(),
                        body: body_text,
                    });
                    continue;
                }
                AttemptOutcome::RetryAfter(d) => {
                    last_error = Some(ProviderError::UpstreamHttp {
                        status: status.as_u16(),
                        body: body_text,
                    });
                    if attempt + 1 < max_retries {
                        tokio::time::sleep(d).await;
                    }
                    continue;
                }
            }
        }

        Err(last_error.unwrap_or(ProviderError::UpstreamHttp {
            status: 0,
            body: format!("max retries exhausted ({max_retries})"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::ProviderError;

    #[test]
    fn outcome_success_decision_yields_succeed() {
        let outcome = compute_attempt_outcome(RetryDecision::Success, 1, &HashSet::new());
        assert!(matches!(outcome, AttemptOutcome::SucceedAndReturn));
    }

    #[test]
    fn outcome_fail_decision_propagates_error() {
        let err = ProviderError::ContextWindowFull;
        let outcome = compute_attempt_outcome(RetryDecision::Fail(err), 1, &HashSet::new());
        assert!(matches!(
            outcome,
            AttemptOutcome::FailWith(ProviderError::ContextWindowFull)
        ));
    }

    #[test]
    fn outcome_disable_decision_yields_disable() {
        let outcome = compute_attempt_outcome(
            RetryDecision::DisableCredential(DisabledReason::QuotaExceeded),
            1,
            &HashSet::new(),
        );
        assert!(matches!(
            outcome,
            AttemptOutcome::DisableAndContinue(DisabledReason::QuotaExceeded)
        ));
    }

    #[test]
    fn outcome_failover_decision_yields_failover() {
        let outcome =
            compute_attempt_outcome(RetryDecision::FailoverCredential, 1, &HashSet::new());
        assert!(matches!(outcome, AttemptOutcome::FailoverContinue));
    }

    #[test]
    fn outcome_retry_decision_yields_retry_after() {
        let outcome = compute_attempt_outcome(
            RetryDecision::Retry {
                backoff: Duration::from_millis(123),
            },
            1,
            &HashSet::new(),
        );
        match outcome {
            AttemptOutcome::RetryAfter(d) => assert_eq!(d.as_millis(), 123),
            other => panic!("期望 RetryAfter，得到 {other:?}"),
        }
    }

    #[test]
    fn outcome_force_refresh_first_time_yields_force_refresh() {
        let outcome = compute_attempt_outcome(RetryDecision::ForceRefresh, 1, &HashSet::new());
        assert!(matches!(outcome, AttemptOutcome::ForceRefreshContinue));
    }

    #[test]
    fn outcome_force_refresh_already_done_downgrades_to_failover() {
        let mut already = HashSet::new();
        already.insert(7u64);
        let outcome = compute_attempt_outcome(RetryDecision::ForceRefresh, 7, &already);
        assert!(matches!(outcome, AttemptOutcome::FailoverContinue));
    }
}
