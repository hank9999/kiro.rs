//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试

use reqwest::Client;
use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, HeaderMap, HeaderValue};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use crate::common::truncate_str_safe;
use crate::http_client::{build_client, build_stream_client, ProxyConfig};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::{CallContext, MultiTokenManager};
use crate::stats::StatsStore;

/// 全局标志：是否需要在下次请求时输出详细日志
static VERBOSE_NEXT_REQUEST: AtomicBool = AtomicBool::new(false);

/// 全局诊断请求计数器
static DIAGNOSTIC_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// 每个凭据的最大重试次数
const MAX_RETRIES_PER_CREDENTIAL: usize = 3;

/// 总重试次数硬上限（避免无限重试）
const MAX_TOTAL_RETRIES: usize = 9;

/// 诊断日志构建器
struct DiagnosticLog {
    request_id: u64,
    sections: Vec<String>,
    start_time: std::time::Instant,
}

impl DiagnosticLog {
    fn new() -> Self {
        Self {
            request_id: DIAGNOSTIC_REQUEST_ID.fetch_add(1, Ordering::SeqCst),
            sections: Vec::new(),
            start_time: std::time::Instant::now(),
        }
    }

    fn add_section(&mut self, title: &str, content: String) {
        self.sections.push(format!(
            "┌─── {} ───\n{}\n└───────────────────────────────────────",
            title, content
        ));
    }

    fn add_step(&mut self, step: &str, details: String) {
        let elapsed = self.start_time.elapsed();
        self.sections.push(format!(
            "[+{:>6.2}ms] {} | {}",
            elapsed.as_secs_f64() * 1000.0,
            step,
            details
        ));
    }

    fn output(&self) {
        let separator = "═".repeat(80);
        let header = format!(
            "\n{}\n  诊断日志 #{}  |  时间: {}  |  总耗时: {:.2}ms\n{}",
            separator,
            self.request_id,
            chrono::Utc::now().to_rfc3339(),
            self.start_time.elapsed().as_secs_f64() * 1000.0,
            separator
        );

        let body = self.sections.join("\n\n");

        tracing::error!(
            "{}\n\n{}\n\n{}",
            header,
            body,
            separator
        );
    }
}

/// 格式化凭据信息用于诊断
fn format_credential_info(ctx: &CallContext, config_region: &str) -> String {
    let cred = &ctx.credentials;
    
    // Token 分析
    let token_info = if ctx.token.is_empty() {
        "Token: <空>".to_string()
    } else {
        let token_len = ctx.token.len();
        let token_preview = if token_len > 100 {
            format!(
                "{}...{}",
                truncate_str_safe(&ctx.token, 50),
                truncate_str_safe(&ctx.token[ctx.token.len().saturating_sub(20)..], 20)
            )
        } else {
            ctx.token.clone()
        };
        
        // 尝试解析 JWT token 的 payload（如果是 JWT 格式）
        let jwt_info = if ctx.token.contains('.') {
            let parts: Vec<&str> = ctx.token.split('.').collect();
            if parts.len() == 3 {
                // 尝试 base64 解码 payload
                match base64_decode_jwt_payload(parts[1]) {
                    Some(payload) => format!("\n    JWT Payload (decoded): {}", payload),
                    None => "\n    JWT Payload: <无法解码>".to_string(),
                }
            } else {
                "".to_string()
            }
        } else {
            "\n    Token 格式: 非 JWT".to_string()
        };
        
        format!(
            "Token 长度: {} 字符\n    Token 预览: {}\n    Token 完整值: {}{}",
            token_len,
            token_preview,
            ctx.token,
            jwt_info
        )
    };

    // expires_at 分析
    let expiry_info = match &cred.expires_at {
        Some(exp) => {
            match chrono::DateTime::parse_from_rfc3339(exp) {
                Ok(dt) => {
                    let now = chrono::Utc::now();
                    let diff = dt.signed_duration_since(now);
                    let status = if diff.num_seconds() < 0 {
                        format!("已过期 {} 秒", -diff.num_seconds())
                    } else if diff.num_minutes() < 5 {
                        format!("即将过期，剩余 {} 秒", diff.num_seconds())
                    } else {
                        format!("有效，剩余 {} 分钟", diff.num_minutes())
                    };
                    format!("expires_at: {} ({})", exp, status)
                }
                Err(_) => format!("expires_at: {} (无法解析)", exp),
            }
        }
        None => "expires_at: <未设置>".to_string(),
    };

    format!(
        "凭据 ID: {}\n\
         认证方式: {:?}\n\
         凭据 region: {:?}\n\
         config region: {}\n\
         {}\n\
         profile_arn: {:?}\n\
         client_id: {}\n\
         client_secret: {}\n\
         refresh_token: {}\n\
         account_email: {:?}\n\
         user_id: {:?}\n\
         machine_id: {:?}\n\
         priority: {}\n\
         enabled_models: {:?}\n\
         {}",
        ctx.id,
        cred.auth_method,
        cred.region,
        config_region,
        expiry_info,
        cred.profile_arn,
        cred.client_id.as_ref().map(|s| format!("{}...({} chars)", &s[..s.len().min(10)], s.len())).unwrap_or_else(|| "<无>".to_string()),
        cred.client_secret.as_ref().map(|s| format!("{}...({} chars)", &s[..s.len().min(5)], s.len())).unwrap_or_else(|| "<无>".to_string()),
        cred.refresh_token.as_ref().map(|s| format!("{}...({} chars)", &s[..s.len().min(20)], s.len())).unwrap_or_else(|| "<无>".to_string()),
        cred.account_email,
        cred.user_id,
        cred.machine_id,
        cred.priority,
        cred.enabled_models,
        token_info
    )
}

/// 尝试 base64 解码 JWT payload
fn base64_decode_jwt_payload(payload: &str) -> Option<String> {
    // JWT 使用 base64url 编码，需要处理 padding
    let mut payload = payload.replace('-', "+").replace('_', "/");
    let padding = (4 - payload.len() % 4) % 4;
    payload.push_str(&"=".repeat(padding));
    
    match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &payload) {
        Ok(bytes) => String::from_utf8(bytes).ok(),
        Err(_) => None,
    }
}

/// 格式化请求头用于诊断
fn format_headers(headers: &HeaderMap) -> String {
    let mut lines = Vec::new();
    for (name, value) in headers.iter() {
        let value_str = value.to_str().unwrap_or("<binary>");
        // 对敏感头进行部分脱敏（使用安全截断）
        let display_value = if name.as_str().eq_ignore_ascii_case("authorization") {
            if value_str.len() > 50 {
                let start = truncate_str_safe(value_str, 30);
                let end_start = value_str.len().saturating_sub(10);
                let end = if end_start < value_str.len() {
                    truncate_str_safe(&value_str[end_start..], 10)
                } else {
                    ""
                };
                format!("{}...{} ({} chars)", start, end, value_str.len())
            } else {
                value_str.to_string()
            }
        } else {
            value_str.to_string()
        };
        lines.push(format!("    {}: {}", name, display_value));
    }
    lines.join("\n")
}

/// 格式化请求体用于诊断
fn format_request_body(body: &str) -> String {
    // 尝试格式化 JSON
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        // 提取关键字段
        let mut key_fields = Vec::new();
        
        if let Some(obj) = json.as_object() {
            for key in ["conversationState", "profileArn", "source", "currentMessage"] {
                if let Some(val) = obj.get(key) {
                    let val_str = serde_json::to_string(val).unwrap_or_else(|_| "<序列化失败>".to_string());
                    let truncated = if val_str.len() > 500 {
                        format!("{}... ({} chars)", truncate_str_safe(&val_str, 500), val_str.len())
                    } else {
                        val_str
                    };
                    key_fields.push(format!("    {}: {}", key, truncated));
                }
            }
        }
        
        let key_fields_str = if key_fields.is_empty() {
            "    <无关键字段>".to_string()
        } else {
            key_fields.join("\n")
        };
        
        let full_body = if body.len() > 3000 {
            format!("{}... (truncated, total {} bytes)", truncate_str_safe(body, 3000), body.len())
        } else {
            body.to_string()
        };
        
        format!(
            "关键字段:\n{}\n\n完整请求体 ({} bytes):\n{}",
            key_fields_str,
            body.len(),
            full_body
        )
    } else {
        // 非 JSON 格式
        if body.len() > 3000 {
            format!("(非 JSON, {} bytes): {}... (truncated)", body.len(), truncate_str_safe(body, 3000))
        } else {
            format!("(非 JSON, {} bytes): {}", body.len(), body)
        }
    }
}

/// 记录完整的诊断日志
fn log_full_diagnostic(
    diag: &mut DiagnosticLog,
    phase: &str,
    url: &str,
    headers: &HeaderMap,
    request_body: &str,
    ctx: &CallContext,
    config: &crate::model::config::Config,
) {
    diag.add_section("阶段", phase.to_string());
    
    diag.add_section("配置信息", format!(
        "config.region: {}\n\
         config.kiro_version: {}\n\
         config.system_version: {}\n\
         config.node_version: {}\n\
         config.tls_backend: {:?}",
        config.region,
        config.kiro_version,
        config.system_version,
        config.node_version,
        config.tls_backend
    ));
    
    diag.add_section("凭据信息", format_credential_info(ctx, &config.region));
    
    diag.add_section("请求 URL", url.to_string());
    
    diag.add_section("请求头", format_headers(headers));
    
    diag.add_section("请求体", format_request_body(request_body));
}

/// 记录响应诊断
fn log_response_diagnostic(
    diag: &mut DiagnosticLog,
    status: reqwest::StatusCode,
    response_body: &str,
) {
    diag.add_section("响应状态", format!(
        "HTTP 状态码: {} ({})\n\
         是否成功: {}\n\
         是否客户端错误: {}\n\
         是否服务端错误: {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown"),
        status.is_success(),
        status.is_client_error(),
        status.is_server_error()
    ));
    
    // 尝试解析响应体
    let response_analysis = if let Ok(json) = serde_json::from_str::<serde_json::Value>(response_body) {
        let mut analysis = Vec::new();
        
        if let Some(obj) = json.as_object() {
            if let Some(msg) = obj.get("message") {
                analysis.push(format!("message: {}", msg));
            }
            if let Some(reason) = obj.get("reason") {
                analysis.push(format!("reason: {}", reason));
            }
            if let Some(error) = obj.get("error") {
                analysis.push(format!("error: {}", error));
            }
        }
        
        if analysis.is_empty() {
            "无特殊错误字段".to_string()
        } else {
            analysis.join("\n")
        }
    } else {
        "响应体非 JSON 格式".to_string()
    };
    
    let body_display = if response_body.len() > 2000 {
        format!("{}... (truncated, {} bytes)", truncate_str_safe(response_body, 2000), response_body.len())
    } else {
        response_body.to_string()
    };
    
    diag.add_section("响应分析", response_analysis);
    diag.add_section("响应体", body_display);
}

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

    /// 获取 API 基础 URL（固定使用 config.region）
    pub fn base_url(&self) -> String {
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            self.token_manager.config().region
        )
    }

    /// 获取 API 基础 URL（固定使用 config.region，忽略凭据级 region）
    fn base_url_for_credential(&self, _credentials: &KiroCredentials) -> String {
        self.base_url()
    }

    /// 获取 MCP API URL（固定使用 config.region）
    pub fn mcp_url(&self) -> String {
        format!(
            "https://q.{}.amazonaws.com/mcp",
            self.token_manager.config().region
        )
    }

    /// 获取 MCP API URL（固定使用 config.region，忽略凭据级 region）
    fn mcp_url_for_credential(&self, _credentials: &KiroCredentials) -> String {
        self.mcp_url()
    }

    /// 获取 API 基础域名（固定使用 config.region）
    pub fn base_domain(&self) -> String {
        format!("q.{}.amazonaws.com", self.token_manager.config().region)
    }

    /// 获取 API 基础域名（固定使用 config.region，忽略凭据级 region）
    fn base_domain_for_credential(&self, _credentials: &KiroCredentials) -> String {
        self.base_domain()
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
            HeaderValue::from_str(&self.base_domain_for_credential(&ctx.credentials))
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
        headers.insert("host", HeaderValue::from_str(&self.base_domain_for_credential(&ctx.credentials)).unwrap());
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

            let url = self.mcp_url_for_credential(&ctx.credentials);
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
            let ctx = match self.token_manager.acquire_context_for_model(model).await {
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

            let url = self.base_url_for_credential(&ctx.credentials);
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

            // 检查是否需要输出详细日志（上次请求遇到 401/403 后设置）
            let should_log_verbose = VERBOSE_NEXT_REQUEST.swap(false, Ordering::SeqCst);
            let mut diag = if should_log_verbose {
                let mut d = DiagnosticLog::new();
                d.add_step("开始请求", format!(
                    "尝试 {}/{}, 凭据 #{}, 模型: {:?}, API 类型: {}",
                    attempt + 1, max_retries, ctx.id, model, api_type
                ));
                log_full_diagnostic(
                    &mut d,
                    "请求准备完成",
                    &url,
                    &headers,
                    &final_request_body,
                    &ctx,
                    self.token_manager.config(),
                );
                Some(d)
            } else {
                None
            };

            // 发送请求
            let client = if is_stream {
                &self.stream_client
            } else {
                &self.client
            };

            if let Some(ref mut d) = diag {
                d.add_step("发送请求", format!("POST {}", url));
            }

            let response = match client
                .post(&url)
                .headers(headers.clone())
                .body(final_request_body.clone())
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    if let Some(ref mut d) = diag {
                        d.add_step("请求失败", format!("网络错误: {}", e));
                        d.output();
                    }

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

            if let Some(ref mut d) = diag {
                d.add_step("收到响应", format!("HTTP {}", status));
            }

            // 成功响应
            if status.is_success() {
                if let Some(ref mut d) = diag {
                    d.add_step("请求成功", "API 调用成功".to_string());
                    d.output();
                }
                self.token_manager.report_success(ctx.id);
                return Ok((ctx.id, response));
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();

            if let Some(ref mut d) = diag {
                log_response_diagnostic(d, status, &body);
            }

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && Self::is_monthly_request_limit(&body) {
                if let Some(ref mut d) = diag {
                    d.add_step("错误类型", "402 额度用尽 (MONTHLY_REQUEST_COUNT)".to_string());
                    d.output();
                }

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
                // 特殊处理：内容长度超限错误
                if Self::is_content_length_exceeded(&body) {
                    if let Some(ref mut d) = diag {
                        d.add_step("错误类型", "400 内容长度超限 (CONTENT_LENGTH_EXCEEDS_THRESHOLD)".to_string());
                        d.output();
                    }

                    tracing::warn!(
                        "内容长度超过限制，将返回特殊错误以触发 Claude Code 压缩: {}",
                        body
                    );

                    if let Some(stats) = &self.stats {
                        stats.record_error(
                            ctx.id,
                            model,
                            truncate_error("内容长度超限 (CONTENT_LENGTH_EXCEEDS_THRESHOLD)".to_string()),
                        );
                    }

                    // 返回特殊错误标记，让上层识别并返回 stop_reason=max_tokens
                    anyhow::bail!("ContentLengthExceeded: Input is too long (CONTENT_LENGTH_EXCEEDS_THRESHOLD)");
                }

                if let Some(ref mut d) = diag {
                    d.add_step("错误类型", "400 Bad Request".to_string());
                    d.output();
                }

                // 其他 400 错误正常处理
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
                // 如果当前没有诊断日志，创建一个完整的
                if diag.is_none() {
                    let mut d = DiagnosticLog::new();
                    d.add_step("401/403 错误触发诊断", format!(
                        "尝试 {}/{}, 凭据 #{}, HTTP {}",
                        attempt + 1, max_retries, ctx.id, status
                    ));
                    log_full_diagnostic(
                        &mut d,
                        &format!("401/403 错误 - 尝试 {}/{}", attempt + 1, max_retries),
                        &url,
                        &headers,
                        &final_request_body,
                        &ctx,
                        self.token_manager.config(),
                    );
                    log_response_diagnostic(&mut d, status, &body);
                    d.add_step("后续操作", "设置 VERBOSE_NEXT_REQUEST 标志，下次请求将输出完整诊断".to_string());
                    d.output();
                } else if let Some(ref mut d) = diag {
                    d.add_step("错误类型", format!("{} 凭据/权限错误", status.as_u16()));
                    d.output();
                }

                // 设置标志，让下次请求输出详细日志
                VERBOSE_NEXT_REQUEST.store(true, Ordering::SeqCst);

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

    /// 检查是否为内容长度超限错误
    ///
    /// 识别 Kiro API 返回的 CONTENT_LENGTH_EXCEEDS_THRESHOLD 错误
    fn is_content_length_exceeded(body: &str) -> bool {
        if body.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") {
            return true;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return false;
        };

        if value
            .get("reason")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "CONTENT_LENGTH_EXCEEDS_THRESHOLD")
        {
            return true;
        }

        value
            .pointer("/error/reason")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "CONTENT_LENGTH_EXCEEDS_THRESHOLD")
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

        // 如果 IdC 凭据没有 profileArn，输出警告
        // 这是一个常见的配置错误，会导致 403 错误
        let Some(arn) = profile_arn else {
            tracing::warn!(
                "IdC 凭据缺少 profileArn 配置！\n\
                 这可能导致 403 错误（bearer token invalid）。\n\
                 请在凭据文件中为 IdC 凭据配置 profileArn 字段。\n\
                 profileArn 格式: arn:aws:codewhisperer:<region>:<account-id>:profile/<profile-id>\n\
                 可以从 Kiro IDE 的凭据中获取此值。"
            );
            return Ok(request_body.to_string());
        };

        // 解析请求体为 JSON
        let mut json: serde_json::Value = serde_json::from_str(request_body)
            .map_err(|e| anyhow::anyhow!("解析请求体 JSON 失败: {}", e))?;

        // 检查原始请求体中的 profileArn
        let original_arn = json.get("profileArn").and_then(|v| v.as_str()).map(|s| s.to_string());
        
        // 注入凭据的 profileArn
        if let Some(obj) = json.as_object_mut() {
            obj.insert("profileArn".to_string(), serde_json::Value::String(arn.clone()));
        }

        // 如果原始 profileArn 和凭据的 profileArn 不同，记录日志
        if let Some(orig) = original_arn {
            if orig != *arn {
                tracing::debug!(
                    "IdC profileArn 替换: {} -> {}",
                    orig,
                    arn
                );
            }
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

    #[test]
    fn test_is_content_length_exceeded_detects_reason() {
        let body = r#"{"message":"Input is too long.","reason":"CONTENT_LENGTH_EXCEEDS_THRESHOLD"}"#;
        assert!(KiroProvider::is_content_length_exceeded(body));
    }

    #[test]
    fn test_is_content_length_exceeded_nested_reason() {
        let body = r#"{"error":{"reason":"CONTENT_LENGTH_EXCEEDS_THRESHOLD"}}"#;
        assert!(KiroProvider::is_content_length_exceeded(body));
    }

    #[test]
    fn test_is_content_length_exceeded_contains_string() {
        let body = "Error: CONTENT_LENGTH_EXCEEDS_THRESHOLD - Input is too long";
        assert!(KiroProvider::is_content_length_exceeded(body));
    }

    #[test]
    fn test_is_content_length_exceeded_false() {
        let body = r#"{"message":"Other error","reason":"INVALID_REQUEST"}"#;
        assert!(!KiroProvider::is_content_length_exceeded(body));
    }

    #[test]
    fn test_is_content_length_exceeded_invalid_json() {
        let body = "not a json";
        assert!(!KiroProvider::is_content_length_exceeded(body));
    }

    /// 集成测试：使用 #18 号凭证发送 HELLO 请求
    ///
    /// 运行方式：
    /// ```
    /// cargo test --package kiro-rs test_hello_request_with_credential_18 -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn test_hello_request_with_credential_18() {
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, UserInputMessage,
        };
        use crate::kiro::model::requests::kiro::KiroRequest;
        use crate::kiro::model::events::Event;
        use crate::kiro::parser::decoder::EventStreamDecoder;
        use futures::StreamExt;

        // 初始化日志
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        // 从上级目录读取凭据文件
        let creds_path = std::path::Path::new("../credentials.json");
        if !creds_path.exists() {
            println!("跳过测试：凭据文件不存在 {:?}", creds_path);
            return;
        }

        let creds_content = match std::fs::read_to_string(creds_path) {
            Ok(c) => c,
            Err(e) => {
                println!("读取凭据文件失败: {}", e);
                return;
            }
        };

        let creds: Vec<KiroCredentials> = match serde_json::from_str(&creds_content) {
            Ok(c) => c,
            Err(e) => {
                println!("解析凭据文件失败: {}", e);
                return;
            }
        };

        // 查找 #18 号凭证（id = 18）
        let cred = match creds.iter().find(|c| c.id == Some(18)) {
            Some(c) => c.clone(),
            None => {
                println!("未找到 #18 号凭证");
                return;
            }
        };

        println!("使用凭证 #18:");
        println!("  auth_method: {:?}", cred.auth_method);
        println!("  has_profile_arn: {}", cred.profile_arn.is_some());
        println!("  has_access_token: {}", cred.access_token.is_some());
        println!("  expires_at: {:?}", cred.expires_at);
        println!("  region: {:?}", cred.region);
        println!("  has_client_id: {}", cred.client_id.is_some());
        println!("  has_client_secret: {}", cred.client_secret.is_some());

        // 加载配置
        let config = match Config::load("config.json") {
            Ok(c) => c,
            Err(e) => {
                println!("加载配置失败: {}", e);
                Config::default()
            }
        };
        println!("API 区域: {}", config.region);

        // 创建 MultiTokenManager（只使用 #18 号凭证）
        let tm = match MultiTokenManager::new(config, vec![cred], None, None, false) {
            Ok(tm) => Arc::new(tm),
            Err(e) => {
                println!("创建 TokenManager 失败: {}", e);
                return;
            }
        };

        // 创建 KiroProvider
        let provider = match KiroProvider::new(tm) {
            Ok(p) => p,
            Err(e) => {
                println!("创建 KiroProvider 失败: {}", e);
                return;
            }
        };

        // 构建 HELLO 请求
        let conversation_id = format!("test-{}", uuid::Uuid::new_v4());
        let state = ConversationState::new(&conversation_id)
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("Hello! Please respond with a short greeting.", "claude-sonnet-4.5")
                    .with_origin("AI_EDITOR"),
            ));

        let request = KiroRequest {
            conversation_state: state,
            profile_arn: None,
        };
        let request_body = match serde_json::to_string(&request) {
            Ok(j) => j,
            Err(e) => {
                println!("序列化请求失败: {}", e);
                return;
            }
        };

        println!("\n请求体:\n{}", request_body);
        println!("\n开始调用流式 API...\n");
        println!("{}", "=".repeat(60));

        // 调用流式 API
        let response = match provider.call_api_stream(&request_body).await {
            Ok(r) => r,
            Err(e) => {
                println!("API 调用失败: {}", e);
                return;
            }
        };

        println!("HTTP 状态: {}", response.status());

        // 获取字节流
        let mut stream = response.bytes_stream();
        let mut decoder = EventStreamDecoder::new();
        let mut total_bytes = 0usize;
        let mut response_text = String::new();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    total_bytes += chunk.len();

                    if let Err(e) = decoder.feed(&chunk) {
                        eprintln!("[缓冲区错误] {}", e);
                        continue;
                    }

                    for result in decoder.decode_iter() {
                        match result {
                            Ok(frame) => {
                                match Event::from_frame(frame) {
                                    Ok(event) => {
                                        match &event {
                                            Event::AssistantResponse(ar) => {
                                                print!("{}", ar.content);
                                                response_text.push_str(&ar.content);
                                            }
                                            Event::ContextUsage(cu) => {
                                                println!("\n[上下文用量] {:.2}%", cu.context_usage_percentage);
                                            }
                                            _ => {}
                                        }
                                    }
                                    Err(e) => eprintln!("[解析错误] {}", e),
                                }
                            }
                            Err(e) => eprintln!("[帧解析错误] {}", e),
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[网络错误] {}", e);
                    break;
                }
            }
        }

        println!("\n{}", "=".repeat(60));
        println!("流式响应结束");
        println!("共接收 {} 字节，解码 {} 帧", total_bytes, decoder.frames_decoded());
        println!("响应内容长度: {} 字符", response_text.len());

        assert!(!response_text.is_empty(), "响应内容不应为空");
    }
}
