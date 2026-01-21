//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试

use reqwest::Client;
use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, HeaderMap, HeaderValue};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use crate::http_client::{build_client, build_stream_client, ProxyConfig};
use crate::kiro::machine_id;
use crate::kiro::token_manager::{CallContext, MultiTokenManager};
use crate::stats::StatsStore;

#[cfg(test)]
use crate::kiro::model::credentials::KiroCredentials;

/// 每个凭据的最大重试次数
const MAX_RETRIES_PER_CREDENTIAL: usize = 3;

/// 总重试次数硬上限（避免无限重试）
const MAX_TOTAL_RETRIES: usize = 9;

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 支持多凭据故障转移和重试机制
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    client: Client,
    stream_client: Client,
    stats: Option<Arc<StatsStore>>,
}

impl KiroProvider {
    /// 创建新的 KiroProvider 实例
    pub fn new(token_manager: Arc<MultiTokenManager>) -> anyhow::Result<Self> {
        Self::with_proxy(token_manager, None)
    }

    /// 创建带代理配置的 KiroProvider 实例
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
    ) -> anyhow::Result<Self> {
        // 非流式请求：设置总超时，避免无限挂起
        let client = build_client(proxy.as_ref(), 720, token_manager.config().tls_backend)?; // 12 分钟

        // 流式请求：关闭总超时，避免长响应被客户端整体 deadline 中断
        let stream_client = build_stream_client(proxy.as_ref())?;

        Ok(Self {
            token_manager,
            client,
            stream_client,
            stats: None,
        })
    }

    /// 给 Provider 绑定统计存储（用于记录调用次数/用量/错误）。
    pub fn with_stats(mut self, stats: Arc<StatsStore>) -> Self {
        self.stats = Some(stats);
        self
    }

    pub fn stats_store(&self) -> Option<Arc<StatsStore>> {
        self.stats.clone()
    }

    /// 获取 token_manager 的引用
    pub fn token_manager(&self) -> &MultiTokenManager {
        &self.token_manager
    }

    /// 获取 API 基础 URL
    pub fn base_url(&self) -> String {
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            self.token_manager.config().region
        )
    }

    /// 获取 MCP API URL
    pub fn mcp_url(&self) -> String {
        format!(
            "https://q.{}.amazonaws.com/mcp",
            self.token_manager.config().region
        )
    }

    /// 获取 API 基础域名
    pub fn base_domain(&self) -> String {
        format!("q.{}.amazonaws.com", self.token_manager.config().region)
    }

    /// 构建请求头
    ///
    /// # Arguments
    /// * `ctx` - API 调用上下文，包含凭据和 token
    fn build_headers(&self, ctx: &CallContext) -> anyhow::Result<HeaderMap> {
        let config = self.token_manager.config();

        let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config)
            .ok_or_else(|| anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"))?;

        let kiro_version = &config.kiro_version;
        let os_name = &config.system_version;
        let node_version = &config.node_version;

        let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{}-{}", kiro_version, machine_id);

        let user_agent = format!(
            "aws-sdk-js/1.0.27 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.27 m/E KiroIDE-{}-{}",
            os_name, node_version, kiro_version, machine_id
        );

        let mut headers = HeaderMap::new();

        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-amzn-codewhisperer-optout",
            HeaderValue::from_static("true"),
        );
        headers.insert("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"));
        headers.insert(
            "x-amz-user-agent",
            HeaderValue::from_str(&x_amz_user_agent)
                .map_err(|e| anyhow::anyhow!("x-amz-user-agent header 无效: {}", e))?,
        );
        headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_str(&user_agent)
                .map_err(|e| anyhow::anyhow!("User-Agent header 无效: {}", e))?,
        );
        headers.insert(
            HOST,
            HeaderValue::from_str(&self.base_domain())
                .map_err(|e| anyhow::anyhow!("Host header 无效: {}", e))?,
        );
        headers.insert(
            "amz-sdk-invocation-id",
            HeaderValue::from_str(&Uuid::new_v4().to_string())
                .map_err(|e| anyhow::anyhow!("amz-sdk-invocation-id header 无效: {}", e))?,
        );
        headers.insert(
            "amz-sdk-request",
            HeaderValue::from_static("attempt=1; max=3"),
        );
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", ctx.token))
                .map_err(|e| anyhow::anyhow!("Authorization header 无效: {}", e))?,
        );
        headers.insert(CONNECTION, HeaderValue::from_static("close"));

        Ok(headers)
    }

    /// 构建 MCP 请求头
    fn build_mcp_headers(&self, ctx: &CallContext) -> anyhow::Result<HeaderMap> {
        let config = self.token_manager.config();

        let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config)
            .ok_or_else(|| anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"))?;

        let kiro_version = &config.kiro_version;
        let os_name = &config.system_version;
        let node_version = &config.node_version;

        let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{}-{}", kiro_version, machine_id);

        let user_agent = format!(
            "aws-sdk-js/1.0.27 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.27 m/E KiroIDE-{}-{}",
            os_name, node_version, kiro_version, machine_id
        );

        let mut headers = HeaderMap::new();

        // 按照严格顺序添加请求头
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert(
            "x-amz-user-agent",
            HeaderValue::from_str(&x_amz_user_agent).unwrap(),
        );
        headers.insert("user-agent", HeaderValue::from_str(&user_agent).unwrap());
        headers.insert("host", HeaderValue::from_str(&self.base_domain()).unwrap());
        headers.insert(
            "amz-sdk-invocation-id",
            HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap(),
        );
        headers.insert(
            "amz-sdk-request",
            HeaderValue::from_static("attempt=1; max=3"),
        );
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", ctx.token)).unwrap(),
        );
        headers.insert("Connection", HeaderValue::from_static("close"));

        Ok(headers)
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移：
    /// - 400 Bad Request: 直接返回错误，不计入凭据失败
    /// - 401/403: 视为凭据/权限问题，计入失败次数并允许故障转移
    /// - 402 MONTHLY_REQUEST_COUNT: 视为额度用尽，禁用凭据并切换
    /// - 429/5xx/网络等瞬态错误: 重试但不禁用或切换凭据（避免误把所有凭据锁死）
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的请求体字符串
    ///
    /// # Returns
    /// 返回原始的 HTTP Response，不做解析
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let (_id, resp) = self.call_api_with_retry(request_body, false, None).await?;
        Ok(resp)
    }

    /// 发送非流式 API 请求，并返回最终使用的 credential_id。
    pub async fn call_api_with_credential_id(
        &self,
        request_body: &str,
        model: Option<&str>,
    ) -> anyhow::Result<(u64, reqwest::Response)> {
        self.call_api_with_retry(request_body, false, model).await
    }

    /// 发送流式 API 请求
    ///
    /// 支持多凭据故障转移：
    /// - 400 Bad Request: 直接返回错误，不计入凭据失败
    /// - 401/403: 视为凭据/权限问题，计入失败次数并允许故障转移
    /// - 402 MONTHLY_REQUEST_COUNT: 视为额度用尽，禁用凭据并切换
    /// - 429/5xx/网络等瞬态错误: 重试但不禁用或切换凭据（避免误把所有凭据锁死）
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的请求体字符串
    ///
    /// # Returns
    /// 返回原始的 HTTP Response，调用方负责处理流式数据
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let (_id, resp) = self.call_api_with_retry(request_body, true, None).await?;
        Ok(resp)
    }

    /// 发送流式 API 请求，并返回最终使用的 credential_id。
    pub async fn call_api_stream_with_credential_id(
        &self,
        request_body: &str,
        model: Option<&str>,
    ) -> anyhow::Result<(u64, reqwest::Response)> {
        self.call_api_with_retry(request_body, true, model).await
    }

    /// 发送 MCP API 请求
    ///
    /// 用于 WebSearch 等工具调用
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的 MCP 请求体字符串
    ///
    /// # Returns
    /// 返回原始的 HTTP Response
    pub async fn call_mcp(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_mcp_with_retry(request_body).await
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..max_retries {
            // 获取调用上下文
            let ctx = match self.token_manager.acquire_context().await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let url = self.mcp_url();
            let headers = match self.build_mcp_headers(&ctx) {
                Ok(h) => h,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            // 发送请求
            let response = match self
                .client
                .post(&url)
                .headers(headers)
                .body(request_body.to_string())
                .send()
                .await
            {
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
                self.token_manager.report_success(ctx.id);
                return Ok(response);
            }

            // 失败响应
            let body = response.text().await.unwrap_or_default();

            // 402 额度用尽
            if status.as_u16() == 402 && Self::is_monthly_request_limit(&body) {
                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 400 Bad Request
            if status.as_u16() == 400 {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 401/403 凭据问题
            if matches!(status.as_u16(), 401 | 403) {
                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 瞬态错误
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
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
    /// - 每个凭据最多重试 MAX_RETRIES_PER_CREDENTIAL 次
    /// - 总重试次数 = min(凭据数量 × 每凭据重试次数, MAX_TOTAL_RETRIES)
    /// - 硬上限 9 次，避免无限重试
    ///
    /// 注意：此方法会自动将当前凭据的 profileArn 注入到请求体中，
    /// 确保 IdC 凭据能够正确使用其对应的 profileArn。
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
        model: Option<&str>,
    ) -> anyhow::Result<(u64, reqwest::Response)> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let api_type = if is_stream { "流式" } else { "非流式" };

        for attempt in 0..max_retries {
            // 获取调用上下文（绑定 index、credentials、token）
            let ctx = match self.token_manager.acquire_context().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "获取调用上下文失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );

                    if let Some(stats) = &self.stats {
                        stats.record_error(0, model, truncate_error(e.to_string()));
                    }

                    last_error = Some(e);
                    sleep(Self::retry_delay(attempt)).await;
                    continue;
                }
            };

            // 动态注入凭据的 profileArn 到请求体（仅对 IdC 凭据）
            // 这确保了 IdC 凭据能够使用其对应的 profileArn
            // Social 凭据保持原有模式，不做特殊处理
            let final_request_body = match Self::inject_profile_arn_for_idc(
                request_body,
                &ctx.credentials.profile_arn,
                &ctx.credentials.auth_method,
            ) {
                Ok(body) => body,
                Err(e) => {
                    tracing::warn!(
                        "注入 profileArn 失败（尝试 {}/{}，credential_id={}）: {}",
                        attempt + 1,
                        max_retries,
                        ctx.id,
                        e
                    );
                    // 注入失败时使用原始请求体
                    request_body.to_string()
                }
            };

            let url = self.base_url();
            let headers = match self.build_headers(&ctx) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(
                        "构建请求头失败（尝试 {}/{}，credential_id={}）: {}",
                        attempt + 1,
                        max_retries,
                        ctx.id,
                        e
                    );

                    if let Some(stats) = &self.stats {
                        stats.record_error(ctx.id, model, truncate_error(e.to_string()));
                    }

                    last_error = Some(e);
                    sleep(Self::retry_delay(attempt)).await;
                    continue;
                }
            };

            if let Some(stats) = &self.stats {
                stats.record_attempt(ctx.id, model);
            }

            // 发送请求
            let client = if is_stream {
                &self.stream_client
            } else {
                &self.client
            };

            let response = match client
                .post(&url)
                .headers(headers)
                .body(final_request_body)
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "API 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );

                    if let Some(stats) = &self.stats {
                        stats.record_error(ctx.id, model, truncate_error(e.to_string()));
                    }

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
                self.token_manager.report_success(ctx.id);
                return Ok((ctx.id, response));
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && Self::is_monthly_request_limit(&body) {
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

            // 400 Bad Request - 请求问题，重试/切换凭据无意义
            if status.as_u16() == 400 {
                if let Some(stats) = &self.stats {
                    stats.record_error(
                        ctx.id,
                        model,
                        truncate_error(format!("{} API 请求失败: {} {}", api_type, status, body)),
                    );
                }
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 401/403 - 更可能是凭据/权限问题：计入失败并允许故障转移
            if matches!(status.as_u16(), 401 | 403) {
                tracing::warn!(
                    "API 请求失败（可能为凭据错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                if let Some(stats) = &self.stats {
                    stats.record_error(
                        ctx.id,
                        model,
                        truncate_error(format!("{} API 请求失败: {} {}", api_type, status, body)),
                    );
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

            // 429/408/5xx - 瞬态上游错误：重试但不禁用或切换凭据
            // （避免 429 high traffic / 502 high load 等瞬态错误把所有凭据锁死）
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                tracing::warn!(
                    "API 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                if let Some(stats) = &self.stats {
                    stats.record_error(
                        ctx.id,
                        model,
                        truncate_error(format!("{} API 请求失败: {} {}", api_type, status, body)),
                    );
                }

                last_error = Some(anyhow::anyhow!("{} API 请求失败: {} {}", api_type, status, body));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx - 通常为请求/配置问题：直接返回，不计入凭据失败
            if status.is_client_error() {
                if let Some(stats) = &self.stats {
                    stats.record_error(
                        ctx.id,
                        model,
                        truncate_error(format!("{} API 请求失败: {} {}", api_type, status, body)),
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

            if let Some(stats) = &self.stats {
                stats.record_error(
                    ctx.id,
                    model,
                    truncate_error(format!("{} API 请求失败: {} {}", api_type, status, body)),
                );
            }

            last_error = Some(anyhow::anyhow!("{} API 请求失败: {} {}", api_type, status, body));
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

    fn is_monthly_request_limit(body: &str) -> bool {
        if body.contains("MONTHLY_REQUEST_COUNT") {
            return true;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return false;
        };

        if value
            .get("reason")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
        {
            return true;
        }

        value
            .pointer("/error/reason")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
    }

    /// 将凭据的 profileArn 注入到请求体中（仅对 IdC 凭据）
    ///
    /// 对于 IdC 凭据，每个凭据可能有不同的 profileArn，
    /// 此方法确保请求体中使用的是当前凭据对应的 profileArn。
    ///
    /// 对于 Social 凭据，保持原有模式，不做特殊处理。
    fn inject_profile_arn_for_idc(
        request_body: &str,
        profile_arn: &Option<String>,
        auth_method: &Option<String>,
    ) -> anyhow::Result<String> {
        // 仅对 IdC 凭据做特殊处理
        let is_idc = auth_method
            .as_ref()
            .map(|m| m.to_lowercase() == "idc")
            .unwrap_or(false);

        if !is_idc {
            // Social 凭据保持原有模式
            return Ok(request_body.to_string());
        }

        // 如果 IdC 凭据没有 profileArn，直接返回原始请求体
        let Some(arn) = profile_arn else {
            return Ok(request_body.to_string());
        };

        // 解析请求体为 JSON
        let mut json: serde_json::Value = serde_json::from_str(request_body)
            .map_err(|e| anyhow::anyhow!("解析请求体 JSON 失败: {}", e))?;

        // 注入 profileArn
        if let Some(obj) = json.as_object_mut() {
            obj.insert("profileArn".to_string(), serde_json::Value::String(arn.clone()));
        }

        // 序列化回字符串
        serde_json::to_string(&json)
            .map_err(|e| anyhow::anyhow!("序列化请求体 JSON 失败: {}", e))
    }
}

fn truncate_error(s: String) -> String {
    const MAX_CHARS: usize = 2000;
    if s.chars().count() <= MAX_CHARS {
        return s;
    }
    let mut out: String = s.chars().take(MAX_CHARS).collect();
    out.push_str("...(truncated)");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::model::credentials::KiroCredentials;
    use crate::kiro::token_manager::CallContext;
    use crate::model::config::Config;

    fn must_ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{:?}", e),
        }
    }

    fn create_test_provider(config: Config, credentials: KiroCredentials) -> KiroProvider {
        let tm = must_ok(MultiTokenManager::new(config, vec![credentials], None, None, false));
        must_ok(KiroProvider::new(Arc::new(tm)))
    }

    #[test]
    fn test_base_url() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let provider = create_test_provider(config, credentials);
        assert!(provider.base_url().contains("amazonaws.com"));
        assert!(provider.base_url().contains("generateAssistantResponse"));
    }

    #[test]
    fn test_base_domain() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        let credentials = KiroCredentials::default();
        let provider = create_test_provider(config, credentials);
        assert_eq!(provider.base_domain(), "q.us-east-1.amazonaws.com");
    }

    #[test]
    fn test_build_headers() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.8.0".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.profile_arn = Some("arn:aws:sso::123456789:profile/test".to_string());
        credentials.refresh_token = Some("a".repeat(150));

        let provider = create_test_provider(config, credentials.clone());
        let ctx = CallContext {
            id: 1,
            credentials,
            token: "test_token".to_string(),
        };
        let headers = must_ok(provider.build_headers(&ctx));

        assert_eq!(
            headers.get(CONTENT_TYPE).map(|v| v.as_bytes()),
            Some("application/json".as_bytes())
        );
        assert_eq!(
            headers.get("x-amzn-codewhisperer-optout").map(|v| v.as_bytes()),
            Some("true".as_bytes())
        );
        assert_eq!(
            headers.get("x-amzn-kiro-agent-mode").map(|v| v.as_bytes()),
            Some("vibe".as_bytes())
        );

        let auth = match headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
            Some(s) => s,
            None => "",
        };
        assert!(auth.starts_with("Bearer "));

        assert_eq!(
            headers.get(CONNECTION).map(|v| v.as_bytes()),
            Some("close".as_bytes())
        );
    }

    #[test]
    fn test_is_monthly_request_limit_detects_reason() {
        let body = r#"{"message":"You have reached the limit.","reason":"MONTHLY_REQUEST_COUNT"}"#;
        assert!(KiroProvider::is_monthly_request_limit(body));
    }

    #[test]
    fn test_is_monthly_request_limit_nested_reason() {
        let body = r#"{"error":{"reason":"MONTHLY_REQUEST_COUNT"}}"#;
        assert!(KiroProvider::is_monthly_request_limit(body));
    }

    #[test]
    fn test_is_monthly_request_limit_false() {
        let body = r#"{"message":"nope","reason":"DAILY_REQUEST_COUNT"}"#;
        assert!(!KiroProvider::is_monthly_request_limit(body));
    }
}
