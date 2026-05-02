//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试
//! 支持按凭据级 endpoint 切换不同 Kiro API 端点

use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::endpoint::{KiroEndpoint, RequestContext};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;
use crate::model::config::TlsBackend;
use parking_lot::Mutex;

/// 总重试次数硬上限（避免无限重试）
const MAX_TOTAL_RETRIES: usize = 30;

fn should_disable_credential_on_status(status: u16) -> bool {
    status == 403
}

fn should_rotate_without_disabling_status(status: u16) -> bool {
    status == 429
}

pub(crate) fn is_invalid_model_id_error(status: u16, body: &str) -> bool {
    (status == 400 || status == 502)
        && (body.contains("INVALID_MODEL_ID")
            || body.contains("Invalid model. Please select a different model"))
}

fn total_attempt_limit(total_credentials: usize) -> usize {
    total_credentials.clamp(1, MAX_TOTAL_RETRIES)
}

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 支持多凭据故障转移和重试机制
/// 按凭据 `endpoint` 字段选择 [`KiroEndpoint`] 实现
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    /// 全局代理配置（用于凭据无自定义代理时的回退）
    global_proxy: Option<ProxyConfig>,
    /// Client 缓存：key = effective proxy config, value = reqwest::Client
    /// 不同代理配置的凭据使用不同的 Client，共享相同代理的凭据复用 Client
    client_cache: Mutex<HashMap<Option<ProxyConfig>, Client>>,
    /// TLS 后端配置
    tls_backend: TlsBackend,
    /// 端点实现注册表（key: endpoint 名称）
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    /// 默认端点名称（凭据未指定 endpoint 时使用）
    default_endpoint: String,
}

impl KiroProvider {
    /// 创建带代理配置和端点注册表的 KiroProvider 实例
    ///
    /// # Arguments
    /// * `token_manager` - 多凭据 Token 管理器
    /// * `proxy` - 全局代理配置
    /// * `endpoints` - 端点名 → 实现的注册表（至少包含 `default_endpoint` 对应条目）
    /// * `default_endpoint` - 凭据未显式指定 endpoint 时使用的名称
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default_endpoint: String,
    ) -> Self {
        assert!(
            endpoints.contains_key(&default_endpoint),
            "默认端点 {} 未在 endpoints 注册表中",
            default_endpoint
        );
        let tls_backend = token_manager.config().tls_backend;
        // 预热：构建全局代理对应的 Client
        let initial_client =
            build_client(proxy.as_ref(), 720, tls_backend).expect("创建 HTTP 客户端失败");
        let mut cache = HashMap::new();
        cache.insert(proxy.clone(), initial_client);

        Self {
            token_manager,
            global_proxy: proxy,
            client_cache: Mutex::new(cache),
            tls_backend,
            endpoints,
            default_endpoint,
        }
    }

    /// 根据凭据的代理配置获取（或创建并缓存）对应的 reqwest::Client
    fn client_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Client> {
        let effective = credentials.effective_proxy(self.global_proxy.as_ref());
        let mut cache = self.client_cache.lock();
        if let Some(client) = cache.get(&effective) {
            return Ok(client.clone());
        }
        let client = build_client(effective.as_ref(), 720, self.tls_backend)?;
        cache.insert(effective, client.clone());
        Ok(client)
    }

    /// 根据凭据选择 endpoint 实现
    fn endpoint_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Arc<dyn KiroEndpoint>> {
        let name = credentials
            .endpoint
            .as_deref()
            .unwrap_or(&self.default_endpoint);
        self.endpoints
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知端点: {}", name))
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移（见 [`Self::call_api_with_retry`]）
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_api_with_retry(request_body, false).await
    }

    /// 发送流式 API 请求
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_api_with_retry(request_body, true).await
    }

    pub fn premium_probe_target_for(&self, model: &str, stream: bool) -> Option<String> {
        let probe = &self.token_manager.config().premium_model_probe;
        if !probe.should_probe_model(model, stream) {
            return None;
        }
        if !self.token_manager.has_premium_probe_candidate() {
            return None;
        }
        if fastrand::f64() > probe.probability_clamped() {
            return None;
        }
        Some(probe.target_model.clone())
    }

    /// 发送一次高级模型探针请求。探针只使用未校验凭据；成功后移入高级凭证库，
    /// 确定性 INVALID_MODEL_ID 则标记为不支持并回退原始请求。
    pub async fn call_api_with_premium_probe(
        &self,
        probe_body: &str,
        fallback_body: &str,
        source_model: &str,
        target_model: &str,
    ) -> anyhow::Result<reqwest::Response> {
        let proof = crate::kiro::token_manager::PremiumVaultProof {
            source_model: source_model.to_string(),
            target_model: target_model.to_string(),
            status: 0,
        };

        let ctx = match self.token_manager.acquire_premium_probe_context().await {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::debug!("高级模型探针无可用未校验凭据，回退普通请求: {}", e);
                return self.call_api(fallback_body).await;
            }
        };

        let config = self.token_manager.config();
        let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);
        let endpoint = match self.endpoint_for(&ctx.credentials) {
            Ok(e) => e,
            Err(e) => {
                self.token_manager.report_failure(ctx.id);
                tracing::warn!("高级模型探针端点解析失败，回退普通请求: {}", e);
                return self.call_api(fallback_body).await;
            }
        };

        let rctx = RequestContext {
            credentials: &ctx.credentials,
            token: &ctx.token,
            machine_id: &machine_id,
            config,
        };

        let url = endpoint.api_url(&rctx);
        let body = endpoint.transform_api_body(probe_body, &rctx);
        let base = self
            .client_for(&ctx.credentials)?
            .post(&url)
            .body(body)
            .header("content-type", "application/json");
        let request = endpoint.decorate_api(base, &rctx);

        let response = match request.send().await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("高级模型探针发送失败，保持未校验并回退普通请求: {}", e);
                return self.call_api(fallback_body).await;
            }
        };

        let status = response.status();
        if status.is_success() {
            let mut proof = proof;
            proof.status = status.as_u16();
            if let Err(e) = self
                .token_manager
                .move_credential_to_premium_vault(ctx.id, &proof)
            {
                tracing::warn!(
                    "高级模型探针成功但凭据 #{} 转存高级库失败，保留普通池: {}",
                    ctx.id,
                    e
                );
            }
            let mut response = response;
            ctx.attach_lease_to_response(&mut response);
            return Ok(response);
        }

        let status_code = status.as_u16();
        let body = response.text().await.unwrap_or_default();
        if is_invalid_model_id_error(status_code, &body) {
            self.token_manager.mark_premium_model_access(
                ctx.id,
                false,
                &proof,
                Some("INVALID_MODEL_ID"),
            )?;
        } else if status_code == 402 && endpoint.is_monthly_request_limit(&body) {
            self.token_manager.report_quota_exhausted(ctx.id);
        } else if should_disable_credential_on_status(status_code) {
            self.token_manager.report_immediate_failure(ctx.id);
        } else if should_rotate_without_disabling_status(status_code) {
            self.token_manager.report_retryable_status_failure(ctx.id);
        } else if status_code == 401 {
            self.token_manager.report_failure(ctx.id);
        }

        tracing::warn!(
            "高级模型探针失败（status={}），已按分类处理并回退普通请求: {}",
            status_code,
            body
        );
        self.call_api(fallback_body).await
    }

    /// 发送 MCP API 请求（WebSearch 等工具调用）
    pub async fn call_mcp(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_mcp_with_retry(request_body).await
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let max_retries = total_attempt_limit(self.token_manager.total_count());
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            // MCP 调用（WebSearch 等工具）不涉及模型选择，无需按模型过滤凭据
            let ctx = match self.token_manager.acquire_context(None).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    // endpoint 解析失败：记为失败，换下一张凭据
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.mcp_url(&rctx);
            let body = endpoint.transform_mcp_body(request_body, &rctx);

            let base = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json");
            let request = endpoint.decorate_mcp(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "MCP 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                let mut response = response;
                ctx.attach_lease_to_response(&mut response);
                self.token_manager.report_success(ctx.id);
                return Ok(response);
            }

            // 失败响应
            let body = response.text().await.unwrap_or_default();

            // 402 额度用尽
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            if should_disable_credential_on_status(status.as_u16()) {
                tracing::warn!(
                    "MCP 请求失败（403，立即禁用当前凭据并切换，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                let has_available = self.token_manager.report_immediate_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            if should_rotate_without_disabling_status(status.as_u16()) {
                tracing::warn!(
                    "MCP 请求失败（{}，不禁用当前凭据，切换到下一张，尝试 {}/{}）: {}",
                    status,
                    attempt + 1,
                    max_retries,
                    body
                );
                let has_available = self.token_manager.report_retryable_status_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（无可切换凭据）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 401 仍视为凭据问题，保留一次强制刷新机会
            if status.as_u16() == 401 {
                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 瞬态错误
            if status.as_u16() == 408 || status.is_server_error() {
                tracing::warn!(
                    "MCP 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx
            if status.is_client_error() {
                if status.as_u16() == 400 {
                    tracing::warn!(
                        "MCP 请求失败（400，请求格式错误，不重试不切换凭据）: {}; request_summary={}",
                        body,
                        Self::kiro_request_diagnostic_summary(request_body)
                    );
                }
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 兜底
            last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("MCP 请求失败：已达到最大重试次数（{}次）", max_retries)
        }))
    }

    /// 内部方法：带重试逻辑的 API 调用
    ///
    /// 重试策略：
    /// - `403`：立即禁用当前凭据并切换
    /// - `400/429`：不禁用当前凭据，短冷却后切换下一张
    /// - 总尝试次数 = min(凭据数量, 30)
    /// - 硬上限 30 次，避免无限重试
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let max_retries = total_attempt_limit(self.token_manager.total_count());
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();
        let api_type = if is_stream { "流式" } else { "非流式" };

        // 尝试从请求体中提取模型信息
        let model = Self::extract_model_from_request(request_body);

        for attempt in 0..max_retries {
            // 获取调用上下文（绑定 index、credentials、token）
            let ctx = match self.token_manager.acquire_context(model.as_deref()).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.api_url(&rctx);
            let body = endpoint.transform_api_body(request_body, &rctx);

            let base = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json");
            let request = endpoint.decorate_api(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "API 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    // 网络错误通常是上游/链路瞬态问题，不应导致"禁用凭据"或"切换凭据"
                    // （否则一段时间网络抖动会把所有凭据都误禁用，需要重启才能恢复）
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                let mut response = response;
                ctx.attach_lease_to_response(&mut response);
                self.token_manager.report_success(ctx.id);
                return Ok(response);
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                tracing::warn!(
                    "API 请求失败（额度已用尽，禁用凭据并切换，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                continue;
            }

            if should_disable_credential_on_status(status.as_u16()) {
                tracing::warn!(
                    "API 请求失败（403，立即禁用当前凭据并切换，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                let has_available = self.token_manager.report_immediate_failure(ctx.id);
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                continue;
            }

            if should_rotate_without_disabling_status(status.as_u16()) {
                tracing::warn!(
                    "API 请求失败（{}，不禁用当前凭据，切换到下一张，尝试 {}/{}）: {}",
                    status,
                    attempt + 1,
                    max_retries,
                    body
                );

                let has_available = self.token_manager.report_retryable_status_failure(ctx.id);
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（无可切换凭据）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                continue;
            }

            // 401 仍视为凭据问题，保留一次强制刷新机会
            if status.as_u16() == 401 {
                tracing::warn!(
                    "API 请求失败（可能为凭据错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                continue;
            }

            // 408/5xx - 瞬态上游错误：重试但不禁用或切换凭据
            if status.as_u16() == 408 || status.is_server_error() {
                tracing::warn!(
                    "API 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx - 通常为请求/配置问题：直接返回，不计入凭据失败
            if status.is_client_error() {
                if status.as_u16() == 400 {
                    tracing::warn!(
                        "{} API 请求失败（400，请求格式错误，不重试不切换凭据）: {}; request_summary={}",
                        api_type,
                        body,
                        Self::kiro_request_diagnostic_summary(request_body)
                    );
                }
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 兜底：当作可重试的瞬态错误处理（不切换凭据）
            tracing::warn!(
                "API 请求失败（未知错误，尝试 {}/{}）: {} {}",
                attempt + 1,
                max_retries,
                status,
                body
            );
            last_error = Some(anyhow::anyhow!(
                "{} API 请求失败: {} {}",
                api_type,
                status,
                body
            ));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        // 所有重试都失败
        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "{} API 请求失败：已达到最大重试次数（{}次）",
                api_type,
                max_retries
            )
        }))
    }

    /// 从请求体中提取模型信息
    ///
    /// 尝试解析 JSON 请求体，提取 conversationState.currentMessage.userInputMessage.modelId
    fn extract_model_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;

        json.get("conversationState")?
            .get("currentMessage")?
            .get("userInputMessage")?
            .get("modelId")?
            .as_str()
            .map(|s| s.to_string())
    }

    fn kiro_request_diagnostic_summary(request_body: &str) -> String {
        use serde_json::Value;

        let Ok(json) = serde_json::from_str::<Value>(request_body) else {
            return format!("parse_error=true body_bytes={}", request_body.len());
        };

        let state = json.get("conversationState").unwrap_or(&Value::Null);
        let history = state
            .get("history")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let current_user = state
            .get("currentMessage")
            .and_then(|v| v.get("userInputMessage"))
            .unwrap_or(&Value::Null);
        let current_context = current_user
            .get("userInputMessageContext")
            .unwrap_or(&Value::Null);

        let model = current_user
            .get("modelId")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let current_content_chars = current_user
            .get("content")
            .and_then(Value::as_str)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let current_tools = current_context
            .get("tools")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let current_tool_results = current_context
            .get("toolResults")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let current_images = current_user
            .get("images")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);

        let mut history_tool_use_ids = HashSet::new();
        let mut history_tool_results = 0usize;
        let mut empty_history_user_messages = 0usize;
        let mut empty_history_assistant_messages = 0usize;

        for message in history {
            if let Some(user_message) = message.get("userInputMessage") {
                let content = user_message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if content.trim().is_empty() {
                    empty_history_user_messages += 1;
                }

                history_tool_results += user_message
                    .get("userInputMessageContext")
                    .and_then(|v| v.get("toolResults"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
            }

            if let Some(assistant_message) = message.get("assistantResponseMessage") {
                let content = assistant_message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let tool_uses = assistant_message
                    .get("toolUses")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);

                if content.trim().is_empty() && tool_uses.is_empty() {
                    empty_history_assistant_messages += 1;
                }

                for tool_use in tool_uses {
                    if let Some(tool_use_id) = tool_use.get("toolUseId").and_then(Value::as_str) {
                        history_tool_use_ids.insert(tool_use_id.to_string());
                    }
                }
            }
        }

        let orphan_current_tool_results = current_context
            .get("toolResults")
            .and_then(Value::as_array)
            .map(|tool_results| {
                tool_results
                    .iter()
                    .filter(|result| {
                        result
                            .get("toolUseId")
                            .and_then(Value::as_str)
                            .is_some_and(|id| !history_tool_use_ids.contains(id))
                    })
                    .count()
            })
            .unwrap_or(0);

        let schema_anomalies = current_context
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .map(|tool| {
                        tool.get("toolSpecification")
                            .and_then(|v| v.get("inputSchema"))
                            .and_then(|v| v.get("json"))
                            .map(Self::count_json_schema_anomalies)
                            .unwrap_or(0)
                    })
                    .sum::<usize>()
            })
            .unwrap_or(0);

        format!(
            "model={} history_messages={} current_content_chars={} current_content_empty={} current_images={} current_tools={} current_tool_results={} history_tool_uses={} history_tool_results={} orphan_current_tool_results={} empty_history_user_messages={} empty_history_assistant_messages={} schema_anomalies={} body_bytes={}",
            model,
            history.len(),
            current_content_chars,
            current_content_chars == 0,
            current_images,
            current_tools,
            current_tool_results,
            history_tool_use_ids.len(),
            history_tool_results,
            orphan_current_tool_results,
            empty_history_user_messages,
            empty_history_assistant_messages,
            schema_anomalies,
            request_body.len()
        )
    }

    fn count_json_schema_anomalies(value: &serde_json::Value) -> usize {
        use serde_json::Value;

        match value {
            Value::Object(obj) => {
                let mut count = 0;

                if let Some(schema_type) = obj.get("type") {
                    if !schema_type.as_str().is_some_and(|s| !s.is_empty()) {
                        count += 1;
                    }
                }

                if let Some(properties) = obj.get("properties") {
                    if !properties.is_object() {
                        count += 1;
                    }
                }

                if let Some(required) = obj.get("required") {
                    let valid_required = required
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string));
                    if !valid_required {
                        count += 1;
                    }
                }

                if let Some(additional_properties) = obj.get("additionalProperties") {
                    if !(additional_properties.is_boolean() || additional_properties.is_object()) {
                        count += 1;
                    }
                }

                count
                    + obj
                        .values()
                        .map(Self::count_json_schema_anomalies)
                        .sum::<usize>()
            }
            Value::Array(items) => items.iter().map(Self::count_json_schema_anomalies).sum(),
            _ => 0,
        }
    }

    fn retry_delay(attempt: usize) -> Duration {
        // 指数退避 + 少量抖动，避免上游抖动时放大故障
        const BASE_MS: u64 = 200;
        const MAX_MS: u64 = 2_000;
        let exp = BASE_MS.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
        let backoff = exp.min(MAX_MS);
        let jitter_max = (backoff / 4).max(1);
        let jitter = fastrand::u64(0..=jitter_max);
        Duration::from_millis(backoff.saturating_add(jitter))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KiroProvider, MAX_TOTAL_RETRIES, is_invalid_model_id_error,
        should_disable_credential_on_status, should_rotate_without_disabling_status,
        total_attempt_limit,
    };

    #[test]
    fn test_total_attempt_limit_caps_at_30() {
        assert_eq!(total_attempt_limit(0), 1);
        assert_eq!(total_attempt_limit(2), 2);
        assert_eq!(total_attempt_limit(30), 30);
        assert_eq!(MAX_TOTAL_RETRIES, 30);
        assert_eq!(total_attempt_limit(40), 30);
    }

    #[test]
    fn test_status_handling_rules() {
        assert!(should_disable_credential_on_status(403));
        assert!(!should_disable_credential_on_status(400));
        assert!(!should_disable_credential_on_status(429));
        assert!(!should_disable_credential_on_status(401));

        assert!(!should_rotate_without_disabling_status(400));
        assert!(should_rotate_without_disabling_status(429));
        assert!(!should_rotate_without_disabling_status(403));
        assert!(!should_rotate_without_disabling_status(401));
        assert!(!should_rotate_without_disabling_status(402));
        assert!(!should_rotate_without_disabling_status(500));
    }

    #[test]
    fn test_invalid_model_id_detection_handles_wrapped_502_body() {
        let body = r#"{"error":{"type":"api_error","message":"上游 API 调用失败: 非流式 API 请求失败: 400 Bad Request {\"message\":\"Invalid model. Please select a different model to continue.\",\"reason\":\"INVALID_MODEL_ID\"}"}}"#;
        assert!(is_invalid_model_id_error(502, body));
    }

    #[test]
    fn test_invalid_model_id_detection_does_not_match_transient_errors() {
        assert!(!is_invalid_model_id_error(
            500,
            "upstream temporarily unavailable"
        ));
        assert!(!is_invalid_model_id_error(429, "rate limited"));
    }

    #[test]
    fn test_kiro_request_diagnostic_summary_redacts_content() {
        let request_body = serde_json::json!({
            "conversationState": {
                "conversationId": "conv-secret",
                "history": [
                    {
                        "assistantResponseMessage": {
                            "content": "assistant secret text",
                            "toolUses": [
                                {"toolUseId": "tool-1", "name": "Read", "input": {"path": "secret.txt"}}
                            ]
                        }
                    },
                    {
                        "userInputMessage": {
                            "content": "user secret text",
                            "modelId": "claude-sonnet-4.5",
                            "userInputMessageContext": {
                                "toolResults": [
                                    {"toolUseId": "tool-1", "content": [{"text": "tool secret"}], "status": "success"}
                                ]
                            }
                        }
                    }
                ],
                "currentMessage": {
                    "userInputMessage": {
                        "content": "current secret text",
                        "modelId": "claude-sonnet-4.5",
                        "userInputMessageContext": {
                            "tools": [
                                {
                                    "toolSpecification": {
                                        "name": "BrokenTool",
                                        "inputSchema": {"json": {"type": "object", "required": null}}
                                    }
                                }
                            ],
                            "toolResults": [
                                {"toolUseId": "orphan", "content": [{"text": "orphan secret"}], "status": "success"}
                            ]
                        }
                    }
                }
            }
        })
        .to_string();

        let summary = KiroProvider::kiro_request_diagnostic_summary(&request_body);

        assert!(summary.contains("model=claude-sonnet-4.5"));
        assert!(summary.contains("history_messages=2"));
        assert!(summary.contains("current_tools=1"));
        assert!(summary.contains("current_tool_results=1"));
        assert!(summary.contains("history_tool_uses=1"));
        assert!(summary.contains("history_tool_results=1"));
        assert!(summary.contains("orphan_current_tool_results=1"));
        assert!(summary.contains("schema_anomalies=1"));
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("conv-secret"));
    }
}
