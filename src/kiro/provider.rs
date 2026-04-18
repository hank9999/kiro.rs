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
use crate::kiro::endpoint::{EndpointErrorKind, KiroRequest, RequestContext};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;

// 注：KiroProvider 本身不再直接持有 EndpointRegistry —— 重试循环通过 ctx.endpoint 访问。
// TokenManager 单向持有 EndpointRegistry，并在 acquire_context* 构造 CallContext 时预解析。
use crate::model::config::TlsBackend;
use parking_lot::Mutex;

/// 每个凭据的最大重试次数
const MAX_RETRIES_PER_CREDENTIAL: usize = 3;

/// 总重试次数硬上限（避免无限重试）
const MAX_TOTAL_RETRIES: usize = 9;

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
}

impl KiroProvider {
    /// 创建带代理配置的 KiroProvider 实例
    ///
    /// 端点注册表通过 `TokenManager` 间接注入：Provider 的重试循环从 `ctx.endpoint`
    /// 取端点实现，无需自身持有 `EndpointRegistry`。
    ///
    /// # Arguments
    /// * `token_manager` - 多凭据 Token 管理器（已持有 endpoint_registry）
    /// * `proxy` - 全局代理配置
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
    ) -> Self {
        let tls_backend = token_manager.config().tls_backend;
        // 预热：构建全局代理对应的 Client
        let initial_client = build_client(proxy.as_ref(), 720, tls_backend)
            .expect("创建 HTTP 客户端失败");
        let mut cache = HashMap::new();
        cache.insert(proxy.clone(), initial_client);

        Self {
            token_manager,
            global_proxy: proxy,
            client_cache: Mutex::new(cache),
            tls_backend,
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

    /// 发送 MCP API 请求（WebSearch 等工具调用）
    pub async fn call_mcp(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_mcp_with_retry(request_body).await
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        // MCP 调用（WebSearch 等工具）不涉及模型选择，无需按模型过滤凭据
        let req = KiroRequest::Mcp { body: request_body };
        self.call_with_retry(&req, None, "MCP 请求").await
    }

    /// 内部方法：带重试逻辑的 API 调用
    ///
    /// 重试策略：
    /// - 每个凭据最多重试 MAX_RETRIES_PER_CREDENTIAL 次
    /// - 总重试次数 = min(凭据数量 × 每凭据重试次数, MAX_TOTAL_RETRIES)
    /// - 硬上限 9 次，避免无限重试
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let model = Self::extract_model_from_request(request_body);
        let kind_label = if is_stream {
            "流式 API 请求"
        } else {
            "非流式 API 请求"
        };
        let req = KiroRequest::GenerateAssistant {
            body: request_body,
            stream: is_stream,
            model: model.as_deref(),
        };
        self.call_with_retry(&req, model.as_deref(), kind_label).await
    }

    /// 统一的带重试循环实现
    ///
    /// # 参数
    /// - `req`: KiroRequest 变体（内部仅含引用，单次构造即可多次传给 build_request）
    /// - `model_for_acquire`: 传给 `acquire_context` 用于凭据过滤（opus 等模型）
    /// - `kind_label`: 日志与错误消息前缀（如 "MCP 请求" / "流式 API 请求" / "非流式 API 请求"）
    ///
    /// 该 helper 承载 §5 所有契约：
    /// - force_refreshed Set 语义：每凭据一次强制刷新机会
    /// - 错误分支顺序：402 → 400 → 401/403 → 瞬态 → 其他 4xx → 兜底
    /// - anyhow 错误消息格式：统一通过 `kind_label` 前缀（字节级等价于重构前
    ///   "{api_type} API 请求失败"、"MCP 请求失败"）
    /// - sleep 时机与 retry 次数
    async fn call_with_retry(
        &self,
        req: &KiroRequest<'_>,
        model_for_acquire: Option<&str>,
        kind_label: &str,
    ) -> anyhow::Result<reqwest::Response> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            let ctx = match self.token_manager.acquire_context(model_for_acquire).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let endpoint = Arc::clone(&ctx.endpoint);

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &ctx.machine_id,
                config,
            };

            let client = self.client_for(&ctx.credentials)?;
            let request = endpoint.build_request(&client, &rctx, req)?;

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "{}发送失败（尝试 {}/{}）: {}",
                        kind_label,
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
                self.token_manager.report_success(ctx.id);
                return Ok(response);
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();

            match endpoint.classify_error(status.as_u16(), &body) {
                EndpointErrorKind::MonthlyQuotaExhausted => {
                    tracing::warn!(
                        "{}失败（额度已用尽，禁用凭据并切换，尝试 {}/{}）: {} {}",
                        kind_label,
                        attempt + 1,
                        max_retries,
                        status,
                        body
                    );

                    let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                    if !has_available {
                        anyhow::bail!(
                            "{}失败（所有凭据已用尽）: {} {}",
                            kind_label,
                            status,
                            body
                        );
                    }

                    last_error = Some(anyhow::anyhow!(
                        "{}失败: {} {}",
                        kind_label,
                        status,
                        body
                    ));
                    continue;
                }
                EndpointErrorKind::BadRequest => {
                    anyhow::bail!("{}失败: {} {}", kind_label, status, body);
                }
                EndpointErrorKind::BearerTokenInvalid => {
                    tracing::warn!(
                        "{}失败（可能为凭据错误，尝试 {}/{}）: {} {}",
                        kind_label,
                        attempt + 1,
                        max_retries,
                        status,
                        body
                    );

                    // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                    if !force_refreshed.contains(&ctx.id) {
                        force_refreshed.insert(ctx.id);
                        tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                        if self.token_manager.force_refresh_token_for(ctx.id).await.is_ok() {
                            tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                            continue;
                        }
                        tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                    }

                    let has_available = self.token_manager.report_failure(ctx.id);
                    if !has_available {
                        anyhow::bail!(
                            "{}失败（所有凭据已用尽）: {} {}",
                            kind_label,
                            status,
                            body
                        );
                    }

                    last_error = Some(anyhow::anyhow!(
                        "{}失败: {} {}",
                        kind_label,
                        status,
                        body
                    ));
                    continue;
                }
                EndpointErrorKind::Transient => {
                    tracing::warn!(
                        "{}失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                        kind_label,
                        attempt + 1,
                        max_retries,
                        status,
                        body
                    );
                    last_error = Some(anyhow::anyhow!(
                        "{}失败: {} {}",
                        kind_label,
                        status,
                        body
                    ));
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
                EndpointErrorKind::ClientError => {
                    anyhow::bail!("{}失败: {} {}", kind_label, status, body);
                }
                EndpointErrorKind::Unknown => {
                    // 兜底：当作可重试的瞬态错误处理（不切换凭据）
                    tracing::warn!(
                        "{}失败（未知错误，尝试 {}/{}）: {} {}",
                        kind_label,
                        attempt + 1,
                        max_retries,
                        status,
                        body
                    );
                    last_error = Some(anyhow::anyhow!(
                        "{}失败: {} {}",
                        kind_label,
                        status,
                        body
                    ));
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            }
        }

        // 所有重试都失败
        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "{}失败：已达到最大重试次数（{}次）",
                kind_label,
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
