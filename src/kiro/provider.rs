//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试

use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;
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
    pub fn with_proxy(token_manager: Arc<MultiTokenManager>, proxy: Option<ProxyConfig>) -> Self {
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

    /// 获取凭据级 API 基础 URL
    fn base_url_for(&self, credentials: &KiroCredentials) -> String {
        let config = self.token_manager.config();
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            credentials.effective_api_region(&config)
        )
    }

    /// 获取凭据级 MCP API URL
    fn mcp_url_for(&self, credentials: &KiroCredentials) -> String {
        let config = self.token_manager.config();
        format!(
            "https://q.{}.amazonaws.com/mcp",
            credentials.effective_api_region(&config)
        )
    }

    /// 获取凭据级 API 基础域名
    fn base_domain_for(&self, credentials: &KiroCredentials) -> String {
        let config = self.token_manager.config();
        format!(
            "q.{}.amazonaws.com",
            credentials.effective_api_region(&config)
        )
    }

    /// 从请求体中提取模型信息
    ///
    /// 尝试解析 JSON 请求体，提取 conversationState.currentMessage.userInputMessage.modelId
    fn extract_model_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;

        // 尝试提取 conversationState.currentMessage.userInputMessage.modelId
        json.get("conversationState")?
            .get("currentMessage")?
            .get("userInputMessage")?
            .get("modelId")?
            .as_str()
            .map(|s| s.to_string())
    }

    /// 将凭据的 profile_arn 注入到请求体 JSON 中
    fn inject_profile_arn(request_body: &str, profile_arn: &Option<String>) -> String {
        if let Some(arn) = profile_arn {
            if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(request_body) {
                json["profileArn"] = serde_json::Value::String(arn.clone());
                if let Ok(body) = serde_json::to_string(&json) {
                    return body;
                }
            }
        }
        request_body.to_string()
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
        self.call_api_with_retry(request_body, false).await
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
        self.call_api_with_retry(request_body, true).await
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
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            // 获取调用上下文
            // MCP 调用（WebSearch 等工具）不涉及模型选择，无需按模型过滤凭据
            let ctx = match self.token_manager.acquire_context(None).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = match machine_id::generate_from_credentials(&ctx.credentials, &config) {
                Some(id) => id,
                None => {
                    last_error = Some(anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"));
                    continue;
                }
            };

            let url = self.mcp_url_for(&ctx.credentials);
            let x_amz_user_agent = format!("aws-sdk-js/1.0.34 KiroIDE-{}-{}", config.kiro_version, machine_id);
            let user_agent = format!(
                "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{}-{}",
                config.system_version, config.node_version, config.kiro_version, machine_id
            );

            // 发送请求
            let mut request = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(request_body.to_string())
                .header("content-type", "application/json");

            // MCP 请求需要携带 profile ARN（如果凭据中存在）
            if let Some(ref arn) = ctx.credentials.profile_arn {
                request = request.header("x-amzn-kiro-profile-arn", arn);
            }

            let response = match request
                .header("x-amz-user-agent", &x_amz_user_agent)
                .header("user-agent", &user_agent)
                .header("host", &self.base_domain_for(&ctx.credentials))
                .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
                .header("amz-sdk-request", "attempt=1; max=3")
                .header("Authorization", format!("Bearer {}", ctx.token))
                .header("Connection", "close")
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
                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if Self::is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
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
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
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
            let machine_id = match machine_id::generate_from_credentials(&ctx.credentials, &config) {
                Some(id) => id,
                None => {
                    last_error = Some(anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"));
                    continue;
                }
            };

            let url = self.base_url_for(&ctx.credentials);
            let x_amz_user_agent = format!("aws-sdk-js/1.0.34 KiroIDE-{}-{}", config.kiro_version, machine_id);
            let user_agent = format!(
                "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{}-{}",
                config.system_version, config.node_version, config.kiro_version, machine_id
            );

            // 注入实际凭据的 profile_arn 到请求体
            let body = Self::inject_profile_arn(request_body, &ctx.credentials.profile_arn);

            // 发送请求
            let response = match self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("x-amzn-codewhisperer-optout", "true")
                .header("x-amzn-kiro-agent-mode", "vibe")
                .header("x-amz-user-agent", &x_amz_user_agent)
                .header("user-agent", &user_agent)
                .header("host", &self.base_domain_for(&ctx.credentials))
                .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
                .header("amz-sdk-request", "attempt=1; max=3")
                .header("Authorization", format!("Bearer {}", ctx.token))
                .header("Connection", "close")
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
                return Ok(response);
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

                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if Self::is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
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

    /// 检查响应体是否包含 bearer token 失效的特征消息
    ///
    /// 当上游已使 accessToken 失效但本地 expiresAt 未到期时，
    /// API 会返回 401/403 并携带此特征消息。
    fn is_bearer_token_invalid(body: &str) -> bool {
        body.contains("The bearer token included in the request is invalid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_inject_profile_arn_with_some() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let arn = Some("arn:aws:codewhisperer:us-east-1:123:profile/ABC".to_string());
        let result = KiroProvider::inject_profile_arn(body, &arn);
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            json["profileArn"],
            "arn:aws:codewhisperer:us-east-1:123:profile/ABC"
        );
        // 原有字段保留
        assert_eq!(json["conversationState"]["conversationId"], "c1");
    }

    #[test]
    fn test_inject_profile_arn_with_none() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let result = KiroProvider::inject_profile_arn(body, &None);
        // 不注入 profileArn，原样返回
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(json.get("profileArn").is_none());
        assert_eq!(json["conversationState"]["conversationId"], "c1");
    }

    #[test]
    fn test_inject_profile_arn_overwrites_existing() {
        let body = r#"{"conversationState":{},"profileArn":"old-arn"}"#;
        let arn = Some("new-arn".to_string());
        let result = KiroProvider::inject_profile_arn(body, &arn);
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["profileArn"], "new-arn");
    }

    #[test]
    fn test_inject_profile_arn_invalid_json() {
        let body = "not-valid-json";
        let arn = Some("arn:test".to_string());
        let result = KiroProvider::inject_profile_arn(body, &arn);
        // 解析失败时原样返回
        assert_eq!(result, "not-valid-json");
    }
}
