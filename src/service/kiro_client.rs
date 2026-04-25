//! KiroClient：组合 RequestExecutor + CredentialPool + EndpointRegistry + RetryPolicy

use std::sync::Arc;

use crate::domain::error::ProviderError;
use crate::domain::retry::RetryPolicy;
use crate::infra::endpoint::EndpointRegistry;
use crate::infra::http::executor::{EndpointKind, RequestExecutor};

use super::credential_pool::CredentialPool;

pub struct KiroClient {
    executor: Arc<RequestExecutor>,
    pool: Arc<CredentialPool>,
    endpoints: Arc<EndpointRegistry>,
    policy: Arc<dyn RetryPolicy>,
}

impl KiroClient {
    pub fn new(
        executor: Arc<RequestExecutor>,
        pool: Arc<CredentialPool>,
        endpoints: Arc<EndpointRegistry>,
        policy: Arc<dyn RetryPolicy>,
    ) -> Self {
        Self {
            executor,
            pool,
            endpoints,
            policy,
        }
    }

    pub fn pool(&self) -> Arc<CredentialPool> {
        self.pool.clone()
    }

    pub fn endpoints(&self) -> Arc<EndpointRegistry> {
        self.endpoints.clone()
    }

    /// 非流式 API
    pub async fn call_api(
        &self,
        body: &str,
        model: Option<&str>,
    ) -> Result<reqwest::Response, ProviderError> {
        self.executor
            .execute(
                EndpointKind::Api,
                body,
                model,
                &self.pool,
                &self.endpoints,
                &*self.policy,
            )
            .await
    }

    /// 流式 API（与 call_api 共享 executor，差异仅在 caller 不缓冲响应）
    pub async fn call_api_stream(
        &self,
        body: &str,
        model: Option<&str>,
    ) -> Result<reqwest::Response, ProviderError> {
        self.executor
            .execute(
                EndpointKind::Api,
                body,
                model,
                &self.pool,
                &self.endpoints,
                &*self.policy,
            )
            .await
    }

    /// MCP API（WebSearch 等工具调用）
    pub async fn call_mcp(
        &self,
        body: &str,
        model: Option<&str>,
    ) -> Result<reqwest::Response, ProviderError> {
        self.executor
            .execute(
                EndpointKind::Mcp,
                body,
                model,
                &self.pool,
                &self.endpoints,
                &*self.policy,
            )
            .await
    }
}
