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

use crate::http_client::{ProxyConfig, ProxyPool, build_client};
use crate::kiro::endpoint::{IdeEndpoint, KiroEndpoint, RequestContext};
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
/// 按凭据 `endpoint` 字段选择 [`KiroEndpoint`] 实现
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    /// 全局代理配置（用于凭据无自定义代理且无代理池时的回退）
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
    fn log_bad_request_details(
        api_type: &str,
        attempt: usize,
        max_retries: usize,
        status: reqwest::StatusCode,
        request_body: &str,
        response_body: &str,
    ) {
        tracing::error!(
            api_type = %api_type,
            attempt = attempt + 1,
            max_retries = max_retries,
            status = %status,
            "Kiro 上游返回 400 Bad Request，完整请求/响应如下。\n==== Request Body ====\n{}\n==== Response Body ====\n{}",
            request_body,
            response_body
        );
    }

    /// 构建只含默认 IDE 端点的注册表（用于简化构造器）
    fn default_endpoints() -> HashMap<String, Arc<dyn KiroEndpoint>> {
        let mut map: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        let ide = IdeEndpoint::new();
        map.insert(ide.name().to_string(), Arc::new(ide));
        map
    }

    /// 创建新的 KiroProvider 实例（默认 IDE 端点，无代理）
    pub fn new(token_manager: Arc<MultiTokenManager>) -> Self {
        Self::with_proxy_and_pool(
            token_manager,
            None,
            None,
            Self::default_endpoints(),
            "ide".to_string(),
        )
    }

    /// 创建带代理配置的 KiroProvider 实例（默认 IDE 端点）
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default_endpoint: String,
    ) -> Self {
        Self::with_proxy_and_pool(token_manager, proxy, None, endpoints, default_endpoint)
    }

    /// 创建带代理配置、代理池和端点注册表的 KiroProvider 实例
    ///
    /// # Arguments
    /// * `token_manager` - 多凭据 Token 管理器
    /// * `proxy` - 全局代理配置
    /// * `proxy_pool` - 可选代理池（用于 IP 轮询，会被塞入 `token_manager`）
    /// * `endpoints` - 端点名 → 实现的注册表（至少包含 `default_endpoint` 对应条目）
    /// * `default_endpoint` - 凭据未显式指定 endpoint 时使用的名称
    pub fn with_proxy_and_pool(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
        proxy_pool: Option<Arc<ProxyPool>>,
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

        // 预热：为代理池中每个代理预构建 Client
        if let Some(pool) = proxy_pool.as_ref() {
            for entry in pool.entries() {
                let key = Some(entry.clone());
                cache.entry(key.clone()).or_insert_with(|| {
                    build_client(Some(entry), 720, tls_backend).expect("创建代理池 Client 失败")
                });
            }
            // 把代理池塞进 token_manager，供 API 调用与 Token 刷新共用
            token_manager.replace_proxy_pool(Some(pool.clone()));
        }

        Self {
            token_manager,
            global_proxy: proxy,
            client_cache: Mutex::new(cache),
            tls_backend,
            endpoints,
            default_endpoint,
        }
    }

    /// 计算凭据当次调用的有效代理
    ///
    /// 优先级：凭据级代理 > 代理池（pick） > 全局单代理 > 无代理
    fn effective_proxy_for(&self, credentials: &KiroCredentials) -> Option<ProxyConfig> {
        // 1. 显式 direct 或凭据自带代理，直接遵循
        if let Some(url) = credentials.proxy_url.as_deref() {
            if url.eq_ignore_ascii_case(KiroCredentials::PROXY_DIRECT) {
                return None;
            }
            // 凭据自定义代理覆盖池与全局
            return credentials.effective_proxy(self.global_proxy.as_ref());
        }

        // 2. 无显式凭据代理，优先走代理池（动态读取，支持热更新）
        if let Some(pool) = self.token_manager.proxy_pool_snapshot() {
            if let Some(picked) = pool.pick(credentials.id) {
                return Some(picked);
            }
        }

        // 3. 回退到全局单代理
        self.global_proxy.clone()
    }

    /// 根据凭据的代理配置获取（或创建并缓存）对应的 reqwest::Client
    fn client_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Client> {
        let (client, _proxy) = self.client_and_proxy_for(credentials)?;
        Ok(client)
    }

    /// 构建 client 并返回本次调用所使用的代理配置
    ///
    /// 返回的 `Option<ProxyConfig>` 用于失败后标记代理冷却（429/限流场景）
    fn client_and_proxy_for(
        &self,
        credentials: &KiroCredentials,
    ) -> anyhow::Result<(Client, Option<ProxyConfig>)> {
        let effective = self.effective_proxy_for(credentials);

        // 请求级 debug 日志：显示本次请求使用哪个代理
        match effective.as_ref() {
            Some(p) => tracing::debug!(
                credential_id = credentials.id,
                proxy = %mask_proxy_url(&p.url),
                "请求将通过代理发出"
            ),
            None => tracing::debug!(
                credential_id = credentials.id,
                proxy = "direct",
                "请求将直连（无代理）"
            ),
        }

        let mut cache = self.client_cache.lock();
        if let Some(client) = cache.get(&effective) {
            return Ok((client.clone(), effective));
        }
        let client = build_client(effective.as_ref(), 720, self.tls_backend)?;
        cache.insert(effective.clone(), client.clone());
        Ok((client, effective))
    }

    /// 将本次请求使用的代理标记为限流冷却（仅当该代理属于当前代理池时）
    ///
    /// 调用时机：上游返回 429 / 503 或底层网络错误。
    /// 若代理不属于池（如凭据级代理或全局单代理），静默忽略。
    fn mark_proxy_cooldown(&self, proxy: Option<&ProxyConfig>, reason: &str) {
        let Some(p) = proxy else { return };
        let Some(pool) = self.token_manager.proxy_pool_snapshot() else {
            return;
        };
        if pool.mark_rate_limited(p) {
            tracing::warn!(
                proxy = %mask_proxy_url(&p.url),
                cooldown_secs = pool.default_cooldown().as_secs(),
                reason = %reason,
                "代理被标记为限流，进入冷却期"
            );
        }
    }

    /// 获取 token_manager 的引用
    pub fn token_manager(&self) -> &MultiTokenManager {
        &self.token_manager
    }

    /// 获取 API 基础 URL（使用 config 级 api_region）
    pub fn base_url(&self) -> String {
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            self.token_manager.config().effective_api_region()
        )
    }

    /// 获取 MCP API URL（使用 config 级 api_region）
    pub fn mcp_url(&self) -> String {
        format!(
            "https://q.{}.amazonaws.com/mcp",
            self.token_manager.config().effective_api_region()
        )
    }

    /// 获取 API 基础域名（使用 config 级 api_region）
    pub fn base_domain(&self) -> String {
        format!(
            "q.{}.amazonaws.com",
            self.token_manager.config().effective_api_region()
        )
    }

    /// 根据凭据选择 endpoint 实现
    fn endpoint_for(
        &self,
        credentials: &KiroCredentials,
    ) -> anyhow::Result<Arc<dyn KiroEndpoint>> {
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

    /// 发送 MCP API 请求（WebSearch 等工具调用）
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

            // 构建 client 同时拿到本次使用的代理配置（用于失败后标记冷却）
            let (http_client, current_proxy) = self.client_and_proxy_for(&ctx.credentials)?;

            let base = http_client
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
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
                    // 网络错误通常意味着代理链路不稳定，标记冷却以便下次重试走别的代理
                    self.mark_proxy_cooldown(current_proxy.as_ref(), "network-error");
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::short_retry_delay()).await;
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
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 400 Bad Request
            if status.as_u16() == 400 {
                Self::log_bad_request_details("MCP", attempt, max_retries, status, &request_body, &body);
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 401/403 凭据问题
            if matches!(status.as_u16(), 401 | 403) {
                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
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
                // 429/503 → 标记代理冷却，下一次重试自动换代理
                let should_cooldown = matches!(status.as_u16(), 429 | 503);
                if should_cooldown {
                    self.mark_proxy_cooldown(
                        current_proxy.as_ref(),
                        &format!("upstream-{}", status.as_u16()),
                    );
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                if attempt + 1 < max_retries {
                    // 冷却场景：已换代理，用短 sleep（100ms）；其他瞬态错误走指数退避
                    let delay = if should_cooldown {
                        Self::short_retry_delay()
                    } else {
                        Self::retry_delay(attempt)
                    };
                    sleep(delay).await;
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

            // 构建 client 同时拿到本次使用的代理配置（用于失败后标记冷却）
            let (http_client, current_proxy) = self.client_and_proxy_for(&ctx.credentials)?;

            let base = http_client
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
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
                    // 网络错误：标记代理冷却（下次 pick 会跳过该代理）
                    // 不切换凭据（网络错误不是凭据问题）
                    self.mark_proxy_cooldown(current_proxy.as_ref(), "network-error");
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::short_retry_delay()).await;
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

            // 400 Bad Request - 请求问题，重试/切换凭据无意义
            if status.as_u16() == 400 {
                Self::log_bad_request_details(
                    api_type,
                    attempt,
                    max_retries,
                    status,
                    &request_body,
                    &body,
                );
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
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
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
                // 429/503 → 标记代理冷却，下一次重试自动换代理
                let should_cooldown = matches!(status.as_u16(), 429 | 503);
                if should_cooldown {
                    self.mark_proxy_cooldown(
                        current_proxy.as_ref(),
                        &format!("upstream-{}", status.as_u16()),
                    );
                }
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                if attempt + 1 < max_retries {
                    // 冷却场景：已换代理，用短 sleep（100ms）即可
                    // 其他瞬态错误：走指数退避
                    let delay = if should_cooldown {
                        Self::short_retry_delay()
                    } else {
                        Self::retry_delay(attempt)
                    };
                    sleep(delay).await;
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

    /// 短重试间隔：用于 429/503/网络错误场景
    ///
    /// 因为已经通过代理冷却切换到了新代理，无需长时间等待；
    /// 加少量抖动避免多个请求同时打向新代理
    fn short_retry_delay() -> Duration {
        const BASE_MS: u64 = 100;
        let jitter = fastrand::u64(0..=40);
        Duration::from_millis(BASE_MS + jitter)
    }
}

/// 屏蔽代理 URL 中的用户名密码后再记录到日志
///
/// 例如 `socks5h://user:pass@host:10001` → `socks5h://***@host:10001`
fn mask_proxy_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if let Some(at) = rest.find('@') {
            let host = &rest[at + 1..];
            return format!("{}***@{}", scheme, host);
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_proxy_url_with_auth() {
        assert_eq!(
            mask_proxy_url("socks5h://user:pass@host.example:10001"),
            "socks5h://***@host.example:10001"
        );
    }

    #[test]
    fn test_mask_proxy_url_without_auth() {
        assert_eq!(
            mask_proxy_url("http://host.example:8080"),
            "http://host.example:8080"
        );
    }
}
