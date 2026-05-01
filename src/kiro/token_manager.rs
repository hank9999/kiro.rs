//! Token 管理模块
//!
//! 负责 Token 过期检测和刷新，支持 Social 和 IdC 认证方式
//! 支持多凭据 (MultiTokenManager) 管理

use anyhow::bail;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as TokioMutex;

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration as StdDuration, Instant};

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::token_refresh::{
    IdcRefreshRequest, IdcRefreshResponse, RefreshRequest, RefreshResponse,
};
use crate::kiro::model::usage_limits::UsageLimitsResponse;
use crate::model::config::Config;

/// 检查 Token 是否在指定时间内过期
pub(crate) fn is_token_expiring_within(
    credentials: &KiroCredentials,
    minutes: i64,
) -> Option<bool> {
    credentials
        .expires_at
        .as_ref()
        .and_then(|expires_at| DateTime::parse_from_rfc3339(expires_at).ok())
        .map(|expires| expires <= Utc::now() + Duration::minutes(minutes))
}

/// 检查 Token 是否已过期（提前 5 分钟判断）
pub(crate) fn is_token_expired(credentials: &KiroCredentials) -> bool {
    is_token_expiring_within(credentials, 5).unwrap_or(true)
}

/// 检查 Token 是否即将过期（10分钟内）
pub(crate) fn is_token_expiring_soon(credentials: &KiroCredentials) -> bool {
    is_token_expiring_within(credentials, 10).unwrap_or(false)
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// 生成 API Key 脱敏展示(前 4 + ... + 后 4,长度不足或非 ASCII 回退 ***)
fn mask_api_key(key: &str) -> String {
    if key.is_ascii() && key.len() > 16 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "***".to_string()
    }
}

fn now_epoch_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

/// 验证 refreshToken 的基本有效性
pub(crate) fn validate_refresh_token(credentials: &KiroCredentials) -> anyhow::Result<()> {
    let refresh_token = credentials
        .refresh_token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("缺少 refreshToken"))?;

    if refresh_token.is_empty() {
        bail!("refreshToken 为空");
    }

    if refresh_token.len() < 100 || refresh_token.ends_with("...") || refresh_token.contains("...")
    {
        bail!(
            "refreshToken 已被截断（长度: {} 字符）。\n\
             这通常是 Kiro IDE 为了防止凭证被第三方工具使用而故意截断的。",
            refresh_token.len()
        );
    }

    Ok(())
}

/// Refresh Token 永久失效错误
///
/// 当服务端返回 400 + `invalid_grant` 时，表示 refreshToken 已被撤销或过期，
/// 不应重试，需立即禁用对应凭据。
#[derive(Debug)]
pub(crate) struct RefreshTokenInvalidError {
    pub message: String,
}

impl fmt::Display for RefreshTokenInvalidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RefreshTokenInvalidError {}

/// 刷新 Token
pub(crate) async fn refresh_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    // API Key 凭据不支持 Token 刷新：底层契约级拦截
    // 其他调用点（try_ensure_token / 活跃路径 / add_credential）在调用前已显式分流 API Key；
    // 仅 force_refresh_token_for 未分流，此处 bail 让错误自然传播为 400 BAD_REQUEST。
    if credentials.is_api_key_credential() {
        bail!("API Key 凭据不支持刷新 Token");
    }

    validate_refresh_token(credentials)?;

    // 根据 auth_method 选择刷新方式
    // 如果未指定 auth_method，根据是否有 clientId/clientSecret 自动判断
    let auth_method = credentials.auth_method.as_deref().unwrap_or_else(|| {
        if credentials.client_id.is_some() && credentials.client_secret.is_some() {
            "idc"
        } else {
            "social"
        }
    });

    if auth_method.eq_ignore_ascii_case("idc")
        || auth_method.eq_ignore_ascii_case("builder-id")
        || auth_method.eq_ignore_ascii_case("iam")
    {
        refresh_idc_token(credentials, config, proxy).await
    } else {
        refresh_social_token(credentials, config, proxy).await
    }
}

/// 刷新 Social Token
async fn refresh_social_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    tracing::info!("正在刷新 Social Token...");

    let refresh_token = credentials.refresh_token.as_ref().unwrap();
    // 优先级：凭据.auth_region > 凭据.region > config.auth_region > config.region
    let region = credentials.effective_auth_region(config);

    let refresh_url = format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken", region);
    let refresh_domain = format!("prod.{}.auth.desktop.kiro.dev", region);
    let machine_id = machine_id::generate_from_credentials(credentials, config);
    let kiro_version = &config.kiro_version;

    let client = build_client(proxy, 60, config.tls_backend)?;
    let body = RefreshRequest {
        refresh_token: refresh_token.to_string(),
    };

    let response = client
        .post(&refresh_url)
        .header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            format!("KiroIDE-{}-{}", kiro_version, machine_id),
        )
        .header("Accept-Encoding", "gzip, compress, deflate, br")
        .header("host", &refresh_domain)
        .header("Connection", "close")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();

        // 400 + invalid_grant + Invalid refresh token provided → refreshToken 永久失效
        if status.as_u16() == 400
            && body_text.contains("\"invalid_grant\"")
            && body_text.contains("Invalid refresh token provided")
        {
            return Err(RefreshTokenInvalidError {
                message: format!("Social refreshToken 已失效 (invalid_grant): {}", body_text),
            }
            .into());
        }

        let error_msg = match status.as_u16() {
            401 => "OAuth 凭证已过期或无效，需要重新认证",
            403 => "权限不足，无法刷新 Token",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS OAuth 服务暂时不可用",
            _ => "Token 刷新失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let data: RefreshResponse = response.json().await?;

    let mut new_credentials = credentials.clone();
    new_credentials.access_token = Some(data.access_token);

    if let Some(new_refresh_token) = data.refresh_token {
        new_credentials.refresh_token = Some(new_refresh_token);
    }

    if let Some(profile_arn) = data.profile_arn {
        new_credentials.profile_arn = Some(profile_arn);
    }

    if let Some(expires_in) = data.expires_in {
        let expires_at = Utc::now() + Duration::seconds(expires_in);
        new_credentials.expires_at = Some(expires_at.to_rfc3339());
    }

    Ok(new_credentials)
}

/// 刷新 IdC Token (AWS SSO OIDC)
async fn refresh_idc_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    tracing::info!("正在刷新 IdC Token...");

    let refresh_token = credentials.refresh_token.as_ref().unwrap();
    let client_id = credentials
        .client_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("IdC 刷新需要 clientId"))?;
    let client_secret = credentials
        .client_secret
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("IdC 刷新需要 clientSecret"))?;

    // 优先级：凭据.auth_region > 凭据.region > config.auth_region > config.region
    let region = credentials.effective_auth_region(config);
    let refresh_url = format!("https://oidc.{}.amazonaws.com/token", region);
    let os_name = &config.system_version;
    let node_version = &config.node_version;

    let x_amz_user_agent = "aws-sdk-js/3.980.0 KiroIDE";
    let user_agent = format!(
        "aws-sdk-js/3.980.0 ua/2.1 os/{} lang/js md/nodejs#{} api/sso-oidc#3.980.0 m/E KiroIDE",
        os_name, node_version
    );

    let client = build_client(proxy, 60, config.tls_backend)?;
    let body = IdcRefreshRequest {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
        refresh_token: refresh_token.to_string(),
        grant_type: "refresh_token".to_string(),
    };

    let response = client
        .post(&refresh_url)
        .header("content-type", "application/json")
        .header("x-amz-user-agent", x_amz_user_agent)
        .header("user-agent", &user_agent)
        .header("host", format!("oidc.{}.amazonaws.com", region))
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=4")
        .header("Connection", "close")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();

        // 400 + invalid_grant + Invalid refresh token provided → refreshToken 永久失效
        if status.as_u16() == 400
            && body_text.contains("\"invalid_grant\"")
            && body_text.contains("Invalid refresh token provided")
        {
            return Err(RefreshTokenInvalidError {
                message: format!("IdC refreshToken 已失效 (invalid_grant): {}", body_text),
            }
            .into());
        }

        let error_msg = match status.as_u16() {
            401 => "IdC 凭证已过期或无效，需要重新认证",
            403 => "权限不足，无法刷新 Token",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS OIDC 服务暂时不可用",
            _ => "IdC Token 刷新失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let data: IdcRefreshResponse = response.json().await?;

    let mut new_credentials = credentials.clone();
    new_credentials.access_token = Some(data.access_token);

    if let Some(new_refresh_token) = data.refresh_token {
        new_credentials.refresh_token = Some(new_refresh_token);
    }

    if let Some(expires_in) = data.expires_in {
        let expires_at = Utc::now() + Duration::seconds(expires_in);
        new_credentials.expires_at = Some(expires_at.to_rfc3339());
    }

    // 同步更新 profile_arn（如果 IdC 响应中包含）
    if let Some(profile_arn) = data.profile_arn {
        new_credentials.profile_arn = Some(profile_arn);
    }

    Ok(new_credentials)
}

/// 获取使用额度信息
pub(crate) async fn get_usage_limits(
    credentials: &KiroCredentials,
    config: &Config,
    token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<UsageLimitsResponse> {
    tracing::debug!("正在获取使用额度信息...");

    // 优先级：凭据.api_region > config.api_region > config.region
    let region = credentials.effective_api_region(config);
    let host = format!("q.{}.amazonaws.com", region);
    let machine_id = machine_id::generate_from_credentials(credentials, config);
    let kiro_version = &config.kiro_version;
    let os_name = &config.system_version;
    let node_version = &config.node_version;

    // 构建 URL
    let mut url = format!(
        "https://{}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST",
        host
    );

    // profileArn 是可选的
    if let Some(profile_arn) = &credentials.profile_arn {
        url.push_str(&format!("&profileArn={}", urlencoding::encode(profile_arn)));
    }

    // 构建 User-Agent headers
    let user_agent = format!(
        "aws-sdk-js/1.0.0 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{}-{}",
        os_name, node_version, kiro_version, machine_id
    );
    let amz_user_agent = format!("aws-sdk-js/1.0.0 KiroIDE-{}-{}", kiro_version, machine_id);

    let client = build_client(proxy, 60, config.tls_backend)?;

    let mut request = client
        .get(&url)
        .header("x-amz-user-agent", &amz_user_agent)
        .header("user-agent", &user_agent)
        .header("host", &host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=1")
        .header("Authorization", format!("Bearer {}", token))
        .header("Connection", "close");

    if credentials.is_api_key_credential() {
        request = request.header("tokentype", "API_KEY");
    }

    let response = request.send().await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let error_msg = match status.as_u16() {
            401 => "认证失败，Token 无效或已过期",
            403 => "权限不足，无法获取使用额度",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS 服务暂时不可用",
            _ => "获取使用额度失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let data: UsageLimitsResponse = response.json().await?;
    Ok(data)
}

// ============================================================================
// 多凭据 Token 管理器
// ============================================================================

/// 单个凭据条目的状态
struct CredentialRuntimeState {
    in_flight: AtomicU32,
    cooldown_until_ms: AtomicU64,
}

impl CredentialRuntimeState {
    fn new() -> Self {
        Self {
            in_flight: AtomicU32::new(0),
            cooldown_until_ms: AtomicU64::new(0),
        }
    }

    fn is_cooling_down(&self, now_ms: u64) -> bool {
        self.cooldown_until_ms.load(Ordering::Relaxed) > now_ms
    }

    fn clear_cooldown(&self) {
        self.cooldown_until_ms.store(0, Ordering::Relaxed);
    }

    fn set_cooldown_for(&self, duration: StdDuration) {
        let until = now_epoch_ms().saturating_add(duration.as_millis() as u64);
        self.cooldown_until_ms.store(until, Ordering::Relaxed);
    }
}

struct CredentialLeaseInner {
    runtime: Arc<CredentialRuntimeState>,
}

impl Drop for CredentialLeaseInner {
    fn drop(&mut self) {
        let _ = self
            .runtime
            .in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            });
    }
}

#[derive(Clone)]
struct CredentialLease {
    _inner: Arc<CredentialLeaseInner>,
}

impl CredentialLease {
    fn acquire(runtime: Arc<CredentialRuntimeState>) -> Self {
        runtime.in_flight.fetch_add(1, Ordering::AcqRel);
        Self {
            _inner: Arc::new(CredentialLeaseInner { runtime }),
        }
    }

    fn try_acquire(runtime: Arc<CredentialRuntimeState>, max_in_flight: u32) -> Option<Self> {
        let mut current = runtime.in_flight.load(Ordering::Acquire);
        loop {
            if current >= max_in_flight {
                return None;
            }
            match runtime.in_flight.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(Self {
                        _inner: Arc::new(CredentialLeaseInner { runtime }),
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
}

struct SelectedCredential {
    id: u64,
    credentials: KiroCredentials,
    lease: CredentialLease,
}

/// 运行时指标快照，用于 Admin API 和测试观察高并发状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetrics {
    pub load_balancing_mode: String,
    pub total: usize,
    pub available: usize,
    pub disabled: usize,
    pub cooling_down: usize,
    pub in_flight: u32,
}

/// 单个凭据条目的状态
struct CredentialEntry {
    /// 凭据唯一 ID
    id: u64,
    /// 凭据信息
    credentials: KiroCredentials,
    /// API 调用连续失败次数
    failure_count: u32,
    /// 400/403/429 类快速禁用状态累计命中次数
    immediate_failure_count: u32,
    /// Token 刷新连续失败次数
    refresh_failure_count: u32,
    /// 是否已禁用
    disabled: bool,
    /// 禁用原因（用于区分手动禁用 vs 自动禁用，便于自愈）
    disabled_reason: Option<DisabledReason>,
    /// API 调用成功次数
    success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    last_used_at: Option<String>,
    /// 高并发运行态，避免请求热路径依赖全局可变字段
    runtime: Arc<CredentialRuntimeState>,
}

/// 禁用原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisabledReason {
    /// Admin API 手动禁用
    Manual,
    /// 连续失败达到阈值后自动禁用
    TooManyFailures,
    /// 请求被上游拒绝达到阈值后禁用（如 400/403/429）
    ImmediateFailure,
    /// Token 刷新连续失败达到阈值后自动禁用
    TooManyRefreshFailures,
    /// 额度已用尽（如 MONTHLY_REQUEST_COUNT）
    QuotaExceeded,
    /// Refresh Token 永久失效（服务端返回 invalid_grant）
    InvalidRefreshToken,
    /// 凭据配置无效（如 authMethod=api_key 但缺少 kiroApiKey）
    InvalidConfig,
}

/// 统计数据持久化条目
#[derive(Serialize, Deserialize)]
struct StatsEntry {
    success_count: u64,
    last_used_at: Option<String>,
}

/// 高级模型探针证明信息，用于写入高级凭证库和审计流水。
#[derive(Debug, Clone)]
pub struct PremiumVaultProof {
    pub source_model: String,
    pub target_model: String,
    pub status: u16,
}

// ============================================================================
// Admin API 公开结构
// ============================================================================

/// 凭据条目快照（用于 Admin API 读取）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialEntrySnapshot {
    /// 凭据唯一 ID
    pub id: u64,
    /// 优先级
    pub priority: u32,
    /// 是否被禁用
    pub disabled: bool,
    /// 连续失败次数
    pub failure_count: u32,
    /// 认证方式
    pub auth_method: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// Token 过期时间
    pub expires_at: Option<String>,
    /// refreshToken 的 SHA-256 哈希（仅 OAuth 凭据，用于前端去重）
    pub refresh_token_hash: Option<String>,
    /// kiroApiKey 的 SHA-256 哈希（仅 API Key 凭据，用于前端去重）
    pub api_key_hash: Option<String>,
    /// kiroApiKey 的脱敏展示（仅 API Key 凭据，用于前端显示）
    pub masked_api_key: Option<String>,
    /// 用户邮箱（用于前端显示）
    pub email: Option<String>,
    /// API 调用成功次数
    pub success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    pub last_used_at: Option<String>,
    /// 是否配置了凭据级代理
    pub has_proxy: bool,
    /// 代理 URL（用于前端展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    /// Token 刷新连续失败次数
    pub refresh_failure_count: u32,
    /// 禁用原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// 端点名称（未显式配置时返回 None，由 Admin 层回退到默认值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// 是否已验证可调用高级模型
    pub premium_model_access: Option<bool>,
    /// 高级模型能力最近一次校验时间
    pub premium_model_access_checked_at: Option<String>,
    /// 最近一次高级模型探针目标模型
    pub premium_model_access_probe_model: Option<String>,
    /// 最近一次高级模型探针源模型
    pub premium_model_access_source_model: Option<String>,
    /// 最近一次高级模型探针错误摘要
    pub premium_model_access_last_error: Option<String>,
    /// 高级凭证库状态
    pub premium_vault_status: Option<String>,
}

/// 凭据管理器状态快照
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerSnapshot {
    /// 凭据条目列表
    pub entries: Vec<CredentialEntrySnapshot>,
    /// 当前活跃凭据 ID
    pub current_id: u64,
    /// 总凭据数量
    pub total: usize,
    /// 可用凭据数量
    pub available: usize,
}

/// 批量重置并启用凭据的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetAllCredentialsResult {
    /// 实际发生状态变化的凭据数量
    pub reset_count: usize,
    /// 因配置无效而保持禁用的凭据数量
    pub skipped_invalid_config_count: usize,
    /// 原本就处于可用且无失败状态的凭据数量
    pub unchanged_count: usize,
}

/// 批量清除 ImmediateFailure 禁用凭据的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearImmediateFailureDisabledResult {
    /// 实际清除的凭据数量
    pub cleared_count: usize,
    /// 因其他禁用原因而被跳过的凭据数量
    pub skipped_other_disabled_count: usize,
    /// 未禁用且无需处理的凭据数量
    pub unchanged_count: usize,
}

/// 多凭据 Token 管理器
///
/// 支持多个凭据的管理，实现固定优先级 + 故障转移策略
/// 故障统计基于 API 调用结果，而非 Token 刷新结果
pub struct MultiTokenManager {
    config: Config,
    proxy: Option<ProxyConfig>,
    /// 凭据条目列表
    entries: Mutex<Vec<CredentialEntry>>,
    /// 当前活动凭据 ID
    current_id: Mutex<u64>,
    /// 自适应轮换游标，用索引避免高并发热路径按 ID 全表定位。
    adaptive_cursor: AtomicUsize,
    /// Token 刷新锁，按凭据隔离，避免不同凭据刷新互相阻塞。
    refresh_locks: Mutex<HashMap<u64, Arc<TokioMutex<()>>>>,
    /// 凭据文件路径（用于回写）
    credentials_path: Option<PathBuf>,
    /// 是否为多凭据格式（数组格式才回写）
    is_multiple_format: bool,
    /// 负载均衡模式（运行时可修改）
    load_balancing_mode: Mutex<String>,
    /// 最近一次统计持久化时间（用于 debounce）
    last_stats_save_at: Mutex<Option<Instant>>,
    /// 统计数据是否有未落盘更新
    stats_dirty: AtomicBool,
}

/// 每个凭据最大 API 调用失败次数
const MAX_FAILURES_PER_CREDENTIAL: u32 = 3;
/// `400/429` 这类需要切换但不禁用的状态在重试前的短冷却，避免立即回切。
const RETRYABLE_STATUS_COOLDOWN: StdDuration = StdDuration::from_secs(5 * 60);
/// 单次获取调用上下文的最大内部尝试次数，避免大量坏凭据放大刷新/校验开销。
const MAX_CONTEXT_ACQUIRE_ATTEMPTS: usize = 30;
/// 统计数据持久化防抖间隔
const STATS_SAVE_DEBOUNCE: StdDuration = StdDuration::from_secs(30);

/// API 调用上下文
///
/// 绑定特定凭据的调用上下文，确保 token、credentials 和 id 的一致性
/// 用于解决并发调用时 current_id 竞态问题
#[derive(Clone)]
pub struct CallContext {
    /// 凭据 ID（用于 report_success/report_failure）
    pub id: u64,
    /// 凭据信息（用于构建请求头）
    pub credentials: KiroCredentials,
    /// 访问 Token
    pub token: String,
    _lease: CredentialLease,
}

impl CallContext {
    /// 将租约挂到响应对象上，确保流式响应消费完成或被丢弃后才释放 inflight。
    pub(crate) fn attach_lease_to_response(&self, response: &mut reqwest::Response) {
        response.extensions_mut().insert(self._lease.clone());
    }
}

impl MultiTokenManager {
    /// 创建多凭据 Token 管理器
    ///
    /// # Arguments
    /// * `config` - 应用配置
    /// * `credentials` - 凭据列表
    /// * `proxy` - 可选的代理配置
    /// * `credentials_path` - 凭据文件路径（用于回写）
    /// * `is_multiple_format` - 是否为多凭据格式（数组格式才回写）
    pub fn new(
        config: Config,
        credentials: Vec<KiroCredentials>,
        proxy: Option<ProxyConfig>,
        credentials_path: Option<PathBuf>,
        is_multiple_format: bool,
    ) -> anyhow::Result<Self> {
        // 计算当前最大 ID，为没有 ID 的凭据分配新 ID
        let max_existing_id = credentials.iter().filter_map(|c| c.id).max().unwrap_or(0);
        let mut next_id = max_existing_id + 1;
        let mut has_new_ids = false;
        let mut has_new_machine_ids = false;
        let config_ref = &config;

        let entries: Vec<CredentialEntry> = credentials
            .into_iter()
            .map(|mut cred| {
                cred.canonicalize_auth_method();
                let id = cred.id.unwrap_or_else(|| {
                    let id = next_id;
                    next_id += 1;
                    cred.id = Some(id);
                    has_new_ids = true;
                    id
                });
                if cred.machine_id.is_none() {
                    cred.machine_id =
                        Some(machine_id::generate_from_credentials(&cred, config_ref));
                    has_new_machine_ids = true;
                }
                CredentialEntry {
                    id,
                    credentials: cred.clone(),
                    failure_count: 0,
                    immediate_failure_count: 0,
                    refresh_failure_count: 0,
                    disabled: cred.disabled, // 从配置文件读取 disabled 状态
                    disabled_reason: if cred.disabled {
                        Some(DisabledReason::Manual)
                    } else {
                        None
                    },
                    success_count: 0,
                    last_used_at: None,
                    runtime: Arc::new(CredentialRuntimeState::new()),
                }
            })
            .collect();

        // 校验 API Key 凭据配置完整性：authMethod=api_key 时必须提供 kiroApiKey
        let mut entries = entries;
        for entry in &mut entries {
            if entry.credentials.kiro_api_key.is_none()
                && entry
                    .credentials
                    .auth_method
                    .as_deref()
                    .map(|m| m.eq_ignore_ascii_case("api_key") || m.eq_ignore_ascii_case("apikey"))
                    .unwrap_or(false)
            {
                tracing::warn!(
                    "凭据 #{} 配置了 authMethod=api_key 但缺少 kiroApiKey 字段，已自动禁用",
                    entry.id
                );
                entry.disabled = true;
                entry.disabled_reason = Some(DisabledReason::InvalidConfig);
            }
        }

        // 检测重复 ID
        let mut seen_ids = std::collections::HashSet::new();
        let mut duplicate_ids = Vec::new();
        for entry in &entries {
            if !seen_ids.insert(entry.id) {
                duplicate_ids.push(entry.id);
            }
        }
        if !duplicate_ids.is_empty() {
            anyhow::bail!("检测到重复的凭据 ID: {:?}", duplicate_ids);
        }

        // 选择初始凭据：优先级最高（priority 最小）的可用凭据，无可用凭据时为 0
        let initial_id = entries
            .iter()
            .filter(|e| !e.disabled)
            .min_by_key(|e| e.credentials.priority)
            .map(|e| e.id)
            .unwrap_or(0);

        let load_balancing_mode = config.load_balancing_mode.clone();
        let manager = Self {
            config,
            proxy,
            entries: Mutex::new(entries),
            current_id: Mutex::new(initial_id),
            adaptive_cursor: AtomicUsize::new(0),
            refresh_locks: Mutex::new(HashMap::new()),
            credentials_path,
            is_multiple_format,
            load_balancing_mode: Mutex::new(load_balancing_mode),
            last_stats_save_at: Mutex::new(None),
            stats_dirty: AtomicBool::new(false),
        };

        // 如果有新分配的 ID 或新生成的 machineId，立即持久化到配置文件
        if has_new_ids || has_new_machine_ids {
            if let Err(e) = manager.persist_credentials() {
                tracing::warn!("补全凭据 ID/machineId 后持久化失败: {}", e);
            } else {
                tracing::info!("已补全凭据 ID/machineId 并写回配置文件");
            }
        }

        // 加载持久化的统计数据（success_count, last_used_at）
        manager.load_stats();

        Ok(manager)
    }

    /// 获取配置的引用
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 获取凭据总数
    pub fn total_count(&self) -> usize {
        self.entries.lock().len()
    }

    /// 获取可用凭据数量
    pub fn available_count(&self) -> usize {
        self.entries.lock().iter().filter(|e| !e.disabled).count()
    }

    /// 获取轻量运行时指标。
    pub fn runtime_metrics(&self) -> RuntimeMetrics {
        let entries = self.entries.lock();
        let now_ms = now_epoch_ms();
        let disabled = entries.iter().filter(|e| e.disabled).count();
        let cooling_down = entries
            .iter()
            .filter(|e| !e.disabled && e.runtime.is_cooling_down(now_ms))
            .count();
        let in_flight = entries
            .iter()
            .map(|e| e.runtime.in_flight.load(Ordering::Relaxed))
            .sum();

        RuntimeMetrics {
            load_balancing_mode: self.get_load_balancing_mode(),
            total: entries.len(),
            available: entries
                .iter()
                .filter(|e| !e.disabled && !e.runtime.is_cooling_down(now_ms))
                .count(),
            disabled,
            cooling_down,
            in_flight,
        }
    }

    fn refresh_lock_for(&self, id: u64) -> Arc<TokioMutex<()>> {
        let mut locks = self.refresh_locks.lock();
        locks
            .entry(id)
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone()
    }

    /// 根据负载均衡模式选择下一个凭据
    ///
    /// - priority 模式：选择优先级最高（priority 最小）的可用凭据
    /// - balanced 模式：均衡选择可用凭据
    /// - round_robin 模式：按当前游标轮换选择可用凭据，降低高并发热点账号重复命中
    ///
    /// # 参数
    /// - `model`: 可选的模型名称，用于过滤支持该模型的凭据（如 opus 模型需要付费订阅）
    fn select_next_credential(&self, model: Option<&str>) -> Option<SelectedCredential> {
        let entries = self.entries.lock();
        let now_ms = now_epoch_ms();

        // 检查是否是 opus 模型
        let is_opus = model
            .map(|m| m.to_lowercase().contains("opus"))
            .unwrap_or(false);

        let mode = self.load_balancing_mode.lock().clone();
        let mode = mode.as_str();
        let is_available = |entry: &CredentialEntry| {
            if entry.disabled {
                return false;
            }
            if is_opus && !entry.credentials.supports_opus() {
                return false;
            }
            true
        };

        if mode == "adaptive_round_robin" {
            const PROBE_LIMIT: usize = 32;
            const MAX_UNPROVEN_IN_FLIGHT_PER_CREDENTIAL: u32 = 1;
            const MAX_PROVEN_IN_FLIGHT_PER_CREDENTIAL: u32 = 3;

            let len = entries.len();
            if len == 0 {
                return None;
            }
            let start_index = self.adaptive_cursor.load(Ordering::Relaxed) % len;
            let now_ms = now_epoch_ms();
            let probe_limit = len.min(PROBE_LIMIT);
            let is_clean = |entry: &CredentialEntry| {
                entry.failure_count == 0
                    && entry.immediate_failure_count == 0
                    && entry.refresh_failure_count == 0
            };
            let max_in_flight_for = |entry: &CredentialEntry| {
                if is_clean(entry) && entry.success_count > 0 {
                    MAX_PROVEN_IN_FLIGHT_PER_CREDENTIAL
                } else {
                    MAX_UNPROVEN_IN_FLIGHT_PER_CREDENTIAL
                }
            };
            let candidate_rank = |entry: &CredentialEntry, in_flight: u32| {
                let health_rank = if is_clean(entry) && entry.success_count > 0 {
                    0
                } else if is_clean(entry) {
                    1
                } else {
                    2
                };
                let failure_rank = entry
                    .failure_count
                    .saturating_add(entry.immediate_failure_count)
                    .saturating_add(entry.refresh_failure_count);
                (
                    health_rank,
                    in_flight,
                    failure_rank,
                    entry.credentials.priority,
                    entry.success_count,
                )
            };

            let mut best_index: Option<usize> = None;
            let mut best_key = (u8::MAX, u32::MAX, u32::MAX, u32::MAX, u64::MAX);
            for offset in 0..probe_limit {
                let index = (start_index + offset) % len;
                let entry = &entries[index];
                if !is_available(entry) || entry.runtime.is_cooling_down(now_ms) {
                    continue;
                }
                let in_flight = entry.runtime.in_flight.load(Ordering::Relaxed);
                if in_flight >= max_in_flight_for(entry) {
                    continue;
                }
                let key = candidate_rank(entry, in_flight);
                if key < best_key {
                    best_key = key;
                    best_index = Some(index);
                }
            }

            if let Some(index) = best_index {
                let entry = &entries[index];
                if let Some(lease) =
                    CredentialLease::try_acquire(entry.runtime.clone(), max_in_flight_for(entry))
                {
                    self.adaptive_cursor
                        .store((index + 1) % len, Ordering::Relaxed);
                    *self.current_id.lock() = entry.id;
                    return Some(SelectedCredential {
                        id: entry.id,
                        credentials: entry.credentials.clone(),
                        lease,
                    });
                }
            }

            // 饥饿保护：短探测窗口都在忙时全池找最低负载，但仍优先保住已证明可用的凭据。
            let mut best_index: Option<usize> = None;
            let mut best_key = (u8::MAX, u32::MAX, u32::MAX, u32::MAX, u64::MAX);
            for (index, entry) in entries.iter().enumerate() {
                if !is_available(entry) || entry.runtime.is_cooling_down(now_ms) {
                    continue;
                }
                let in_flight = entry.runtime.in_flight.load(Ordering::Relaxed);
                if in_flight >= max_in_flight_for(entry) {
                    continue;
                }
                let key = candidate_rank(entry, in_flight);
                if key < best_key {
                    best_key = key;
                    best_index = Some(index);
                }
            }

            let index = best_index?;
            let entry = &entries[index];
            let lease =
                CredentialLease::try_acquire(entry.runtime.clone(), max_in_flight_for(entry))?;
            self.adaptive_cursor
                .store((index + 1) % len, Ordering::Relaxed);
            *self.current_id.lock() = entry.id;
            return Some(SelectedCredential {
                id: entry.id,
                credentials: entry.credentials.clone(),
                lease,
            });
        }

        // 非自适应模式保留原有语义：先构造可用列表，再按模式选择。
        let available: Vec<_> = entries
            .iter()
            .filter(|e| is_available(e) && !e.runtime.is_cooling_down(now_ms))
            .collect();

        if available.is_empty() {
            return None;
        }

        match mode {
            "round_robin" => {
                let current_id = *self.current_id.lock();
                let selected_index = available
                    .iter()
                    .position(|e| e.id == current_id)
                    .unwrap_or(0);
                let entry = available[selected_index];
                let next = available[(selected_index + 1) % available.len()];
                *self.current_id.lock() = next.id;

                Some(SelectedCredential {
                    id: entry.id,
                    credentials: entry.credentials.clone(),
                    lease: CredentialLease::acquire(entry.runtime.clone()),
                })
            }
            "balanced" => {
                // Least-Used 策略：选择成功次数最少的凭据
                // 平局时按优先级排序（数字越小优先级越高）
                let entry = available
                    .iter()
                    .min_by_key(|e| (e.success_count, e.credentials.priority))?;

                Some(SelectedCredential {
                    id: entry.id,
                    credentials: entry.credentials.clone(),
                    lease: CredentialLease::acquire(entry.runtime.clone()),
                })
            }
            _ => {
                // priority 模式（默认）：选择优先级最高的
                let entry = available.iter().min_by_key(|e| e.credentials.priority)?;
                Some(SelectedCredential {
                    id: entry.id,
                    credentials: entry.credentials.clone(),
                    lease: CredentialLease::acquire(entry.runtime.clone()),
                })
            }
        }
    }

    /// 获取 API 调用上下文
    ///
    /// 返回绑定了 id、credentials 和 token 的调用上下文
    /// 确保整个 API 调用过程中使用一致的凭据信息
    ///
    /// 如果 Token 过期或即将过期，会自动刷新
    /// Token 刷新失败会累计到当前凭据，达到阈值后禁用并切换
    ///
    /// # 参数
    /// - `model`: 可选的模型名称，用于过滤支持该模型的凭据（如 opus 模型需要付费订阅）
    pub async fn acquire_context(&self, model: Option<&str>) -> anyhow::Result<CallContext> {
        let total = self.total_count();
        let max_attempts =
            (total * MAX_FAILURES_PER_CREDENTIAL as usize).clamp(1, MAX_CONTEXT_ACQUIRE_ATTEMPTS);
        let mut attempt_count = 0;

        loop {
            if attempt_count >= max_attempts {
                anyhow::bail!(
                    "所有凭据均无法获取有效 Token（可用: {}/{}）",
                    self.available_count(),
                    total
                );
            }

            let selected = {
                let mode = self.load_balancing_mode.lock().clone();
                let rotates_on_each_request =
                    mode == "balanced" || mode == "round_robin" || mode == "adaptive_round_robin";

                // balanced/round_robin/adaptive_round_robin 模式：每次请求都重新选择，不固定 current_id
                // priority 模式：优先使用 current_id 指向的凭据
                let current_hit = if rotates_on_each_request {
                    None
                } else {
                    let entries = self.entries.lock();
                    let current_id = *self.current_id.lock();
                    let now_ms = now_epoch_ms();
                    entries
                        .iter()
                        .find(|e| {
                            e.id == current_id && !e.disabled && !e.runtime.is_cooling_down(now_ms)
                        })
                        .map(|e| SelectedCredential {
                            id: e.id,
                            credentials: e.credentials.clone(),
                            lease: CredentialLease::acquire(e.runtime.clone()),
                        })
                };

                if let Some(hit) = current_hit {
                    hit
                } else {
                    // 当前凭据不可用或轮换模式，根据负载均衡策略选择
                    let mut best = self.select_next_credential(model);

                    // 没有可用凭据：如果是"自动禁用导致全灭"，做一次类似重启的自愈
                    if best.is_none() {
                        let mut entries = self.entries.lock();
                        if entries.iter().any(|e| {
                            e.disabled && e.disabled_reason == Some(DisabledReason::TooManyFailures)
                        }) {
                            tracing::warn!(
                                "所有凭据均已被自动禁用，执行自愈：重置失败计数并重新启用（等价于重启）"
                            );
                            for e in entries.iter_mut() {
                                if e.disabled_reason == Some(DisabledReason::TooManyFailures) {
                                    e.disabled = false;
                                    e.disabled_reason = None;
                                    e.failure_count = 0;
                                    e.immediate_failure_count = 0;
                                    e.runtime.clear_cooldown();
                                }
                            }
                            drop(entries);
                            best = self.select_next_credential(model);
                        }
                    }

                    if let Some(selected) = best {
                        if mode != "round_robin" && mode != "adaptive_round_robin" {
                            // round_robin 在选择时已经把 current_id 推进到下一张凭据。
                            let mut current_id = self.current_id.lock();
                            *current_id = selected.id;
                        }
                        selected
                    } else {
                        let entries = self.entries.lock();
                        // 注意：必须在 bail! 之前计算 available_count，
                        // 因为 available_count() 会尝试获取 entries 锁，
                        // 而此时我们已经持有该锁，会导致死锁
                        let available = entries.iter().filter(|e| !e.disabled).count();
                        anyhow::bail!("所有凭据均已禁用（{}/{}）", available, total);
                    }
                }
            };
            let SelectedCredential {
                id,
                credentials,
                lease,
            } = selected;

            // 尝试获取/刷新 Token
            match self.try_ensure_token(id, &credentials, lease).await {
                Ok(ctx) => {
                    return Ok(ctx);
                }
                Err(e) => {
                    // refreshToken 永久失效 → 立即禁用，不累计重试
                    let has_available = if e.downcast_ref::<RefreshTokenInvalidError>().is_some() {
                        tracing::warn!("凭据 #{} refreshToken 永久失效: {}", id, e);
                        self.report_refresh_token_invalid(id)
                    } else {
                        tracing::warn!("凭据 #{} Token 刷新失败: {}", id, e);
                        self.report_refresh_failure(id)
                    };
                    attempt_count += 1;
                    if !has_available {
                        anyhow::bail!("所有凭据均已禁用（0/{}）", total);
                    }
                }
            }
        }
    }

    /// 选择优先级最高的未禁用凭据作为当前凭据（内部方法）
    ///
    /// 纯粹按优先级选择，不排除当前凭据，用于优先级变更后立即生效
    fn select_highest_priority(&self) {
        let entries = self.entries.lock();
        let mut current_id = self.current_id.lock();

        // 选择优先级最高的未禁用凭据（不排除当前凭据）
        if let Some(best) = entries
            .iter()
            .filter(|e| !e.disabled)
            .min_by_key(|e| e.credentials.priority)
        {
            if best.id != *current_id {
                tracing::info!(
                    "优先级变更后切换凭据: #{} -> #{}（优先级 {}）",
                    *current_id,
                    best.id,
                    best.credentials.priority
                );
                *current_id = best.id;
            }
        }
    }

    /// 尝试使用指定凭据获取有效 Token
    ///
    /// 使用双重检查锁定模式，确保同一时间只有一个刷新操作
    ///
    /// # Arguments
    /// * `id` - 凭据 ID，用于更新正确的条目
    /// * `credentials` - 凭据信息
    async fn try_ensure_token(
        &self,
        id: u64,
        credentials: &KiroCredentials,
        lease: CredentialLease,
    ) -> anyhow::Result<CallContext> {
        // API Key 凭据直接使用 kiro_api_key 作为 Bearer Token，无需刷新
        if credentials.is_api_key_credential() {
            let token = credentials
                .kiro_api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("API Key 凭据缺少 kiroApiKey"))?;
            return Ok(CallContext {
                id,
                credentials: credentials.clone(),
                token,
                _lease: lease,
            });
        }

        // 第一次检查（无锁）：快速判断是否需要刷新
        let needs_refresh = is_token_expired(credentials) || is_token_expiring_soon(credentials);

        let creds = if needs_refresh {
            // 同一凭据同一时间只允许一个刷新，不同凭据可并发刷新。
            let refresh_lock = self.refresh_lock_for(id);
            let _guard = refresh_lock.lock().await;

            // 第二次检查：获取锁后重新读取凭据，因为其他请求可能已经完成刷新
            let current_creds = {
                let entries = self.entries.lock();
                entries
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.credentials.clone())
                    .ok_or_else(|| anyhow::anyhow!("凭据 #{} 不存在", id))?
            };

            if is_token_expired(&current_creds) || is_token_expiring_soon(&current_creds) {
                // 确实需要刷新
                let effective_proxy = current_creds.effective_proxy(self.proxy.as_ref());
                let new_creds =
                    refresh_token(&current_creds, &self.config, effective_proxy.as_ref()).await?;

                if is_token_expired(&new_creds) {
                    anyhow::bail!("刷新后的 Token 仍然无效或已过期");
                }

                // 更新凭据
                {
                    let mut entries = self.entries.lock();
                    if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                        entry.credentials = new_creds.clone();
                    }
                }

                // 回写凭据到文件（仅多凭据格式），失败只记录警告
                if let Err(e) = self.persist_credentials() {
                    tracing::warn!("Token 刷新后持久化失败（不影响本次请求）: {}", e);
                }

                new_creds
            } else {
                // 其他请求已经完成刷新，直接使用新凭据
                tracing::debug!("Token 已被其他请求刷新，跳过刷新");
                current_creds
            }
        } else {
            credentials.clone()
        };

        let token = creds
            .access_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("没有可用的 accessToken"))?;

        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.refresh_failure_count = 0;
            }
        }

        Ok(CallContext {
            id,
            credentials: creds,
            token,
            _lease: lease,
        })
    }

    /// 是否存在可用于高级模型探针的未校验凭据。
    pub fn has_premium_probe_candidate(&self) -> bool {
        let entries = self.entries.lock();
        let now_ms = now_epoch_ms();
        entries.iter().any(|entry| {
            !entry.disabled
                && entry.credentials.premium_model_access.is_none()
                && !entry.runtime.is_cooling_down(now_ms)
        })
    }

    /// 获取一个仅用于高级模型探针的调用上下文，只会选择未校验凭据。
    pub async fn acquire_premium_probe_context(&self) -> anyhow::Result<CallContext> {
        let selected = {
            let entries = self.entries.lock();
            let now_ms = now_epoch_ms();
            let mut candidates: Vec<_> = entries
                .iter()
                .filter(|entry| {
                    !entry.disabled
                        && entry.credentials.premium_model_access.is_none()
                        && !entry.runtime.is_cooling_down(now_ms)
                })
                .collect();
            candidates.sort_by_key(|entry| entry.credentials.priority);

            candidates.into_iter().find_map(|entry| {
                CredentialLease::try_acquire(entry.runtime.clone(), 1).map(|lease| {
                    *self.current_id.lock() = entry.id;
                    SelectedCredential {
                        id: entry.id,
                        credentials: entry.credentials.clone(),
                        lease,
                    }
                })
            })
        };

        let selected = selected.ok_or_else(|| anyhow::anyhow!("没有未校验的高级模型候选凭据"))?;
        match self
            .try_ensure_token(selected.id, &selected.credentials, selected.lease)
            .await
        {
            Ok(ctx) => Ok(ctx),
            Err(e) => {
                self.report_refresh_failure(selected.id);
                Err(e)
            }
        }
    }

    /// 标记凭据高级模型能力并回写普通凭据文件。
    pub fn mark_premium_model_access(
        &self,
        id: u64,
        access: bool,
        proof: &PremiumVaultProof,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            let checked_at = Utc::now().to_rfc3339();
            entry.credentials.premium_model_access = Some(access);
            entry.credentials.premium_model_access_checked_at = Some(checked_at);
            entry.credentials.premium_model_access_probe_model = Some(proof.target_model.clone());
            entry.credentials.premium_model_access_source_model = Some(proof.source_model.clone());
            entry.credentials.premium_model_access_last_error = error.map(|s| s.to_string());
        }

        self.persist_credentials()?;
        Ok(())
    }

    fn read_premium_vault(&self, path: &PathBuf) -> anyhow::Result<Vec<KiroCredentials>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(&content)?)
    }

    fn write_premium_vault(
        &self,
        path: &PathBuf,
        credentials: &[KiroCredentials],
    ) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(credentials)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    fn append_premium_event(
        &self,
        event: &str,
        credential: &KiroCredentials,
        proof: &PremiumVaultProof,
    ) -> anyhow::Result<()> {
        let path = self.premium_event_log_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let refresh_token_hash = credential.refresh_token.as_deref().map(sha256_hex);
        let api_key_hash = credential.kiro_api_key.as_deref().map(sha256_hex);
        let payload = serde_json::json!({
            "event": event,
            "credentialId": credential.id,
            "at": Utc::now().to_rfc3339(),
            "sourceModel": proof.source_model,
            "targetModel": proof.target_model,
            "status": proof.status,
            "refreshTokenHash": refresh_token_hash,
            "apiKeyHash": api_key_hash,
            "email": credential.email,
        });

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{}", serde_json::to_string(&payload)?)?;
        file.flush()?;
        Ok(())
    }

    /// 将已验证高级模型可用的凭据移入高级凭证库，成功写库后才从普通池移除。
    pub fn move_credential_to_premium_vault(
        &self,
        id: u64,
        proof: &PremiumVaultProof,
    ) -> anyhow::Result<()> {
        let mut credential = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        let checked_at = Utc::now().to_rfc3339();
        credential.premium_model_access = Some(true);
        credential.premium_model_access_checked_at = Some(checked_at);
        credential.premium_model_access_probe_model = Some(proof.target_model.clone());
        credential.premium_model_access_source_model = Some(proof.source_model.clone());
        credential.premium_model_access_last_error = None;
        credential.premium_vault_status = Some("verified".to_string());

        let vault_path = self.premium_vault_path();
        let mut vault = self.read_premium_vault(&vault_path)?;
        let refresh_hash = credential.refresh_token.as_deref().map(sha256_hex);
        let api_key_hash = credential.kiro_api_key.as_deref().map(sha256_hex);
        let exists = vault.iter().any(|existing| {
            existing.id == credential.id
                || (refresh_hash.is_some()
                    && existing.refresh_token.as_deref().map(sha256_hex) == refresh_hash)
                || (api_key_hash.is_some()
                    && existing.kiro_api_key.as_deref().map(sha256_hex) == api_key_hash)
        });
        if !exists {
            vault.push(credential.clone());
        }
        self.write_premium_vault(&vault_path, &vault)?;
        self.append_premium_event("moved_to_premium_vault", &credential, proof)?;

        {
            let mut entries = self.entries.lock();
            entries.retain(|entry| entry.id != id);
        }
        self.select_highest_priority();
        self.persist_credentials()?;
        self.save_stats();
        tracing::info!(
            "凭据 #{} 已验证可调用高级模型并移入高级凭证库: {}",
            id,
            vault_path.display()
        );
        Ok(())
    }

    pub fn list_premium_vault_credentials(&self) -> anyhow::Result<Vec<KiroCredentials>> {
        self.read_premium_vault(&self.premium_vault_path())
    }

    pub fn restore_premium_vault_credential(&self, id: u64) -> anyhow::Result<u64> {
        let vault_path = self.premium_vault_path();
        let mut vault = self.read_premium_vault(&vault_path)?;
        let position = vault
            .iter()
            .position(|credential| credential.id == Some(id))
            .ok_or_else(|| anyhow::anyhow!("高级凭证不存在: {}", id))?;
        let mut credential = vault.remove(position);
        credential.disabled = false;
        credential.premium_vault_status = Some("restored".to_string());

        let restored_id = {
            let mut entries = self.entries.lock();
            let duplicate_id = credential
                .id
                .is_some_and(|candidate| entries.iter().any(|entry| entry.id == candidate));
            if duplicate_id || credential.id.is_none() {
                let new_id = entries.iter().map(|entry| entry.id).max().unwrap_or(0) + 1;
                credential.id = Some(new_id);
            }
            let id = credential.id.unwrap();
            entries.push(CredentialEntry {
                id,
                credentials: credential.clone(),
                failure_count: 0,
                immediate_failure_count: 0,
                refresh_failure_count: 0,
                disabled: false,
                disabled_reason: None,
                success_count: 0,
                last_used_at: None,
                runtime: Arc::new(CredentialRuntimeState::new()),
            });
            id
        };

        self.persist_credentials()?;
        self.write_premium_vault(&vault_path, &vault)?;
        self.append_premium_event(
            "restored_to_active_pool",
            &credential,
            &PremiumVaultProof {
                source_model: credential
                    .premium_model_access_source_model
                    .clone()
                    .unwrap_or_default(),
                target_model: credential
                    .premium_model_access_probe_model
                    .clone()
                    .unwrap_or_default(),
                status: 0,
            },
        )?;
        self.select_highest_priority();
        Ok(restored_id)
    }

    pub fn delete_premium_vault_credential(&self, id: u64) -> anyhow::Result<()> {
        let vault_path = self.premium_vault_path();
        let mut vault = self.read_premium_vault(&vault_path)?;
        let position = vault
            .iter()
            .position(|credential| credential.id == Some(id))
            .ok_or_else(|| anyhow::anyhow!("高级凭证不存在: {}", id))?;
        let mut credential = vault.remove(position);
        credential.premium_vault_status = Some("deleted".to_string());
        self.write_premium_vault(&vault_path, &vault)?;
        self.append_premium_event(
            "deleted_from_premium_vault",
            &credential,
            &PremiumVaultProof {
                source_model: credential
                    .premium_model_access_source_model
                    .clone()
                    .unwrap_or_default(),
                target_model: credential
                    .premium_model_access_probe_model
                    .clone()
                    .unwrap_or_default(),
                status: 0,
            },
        )?;
        Ok(())
    }

    /// 将凭据列表回写到源文件
    ///
    /// 仅在以下条件满足时回写：
    /// - 源文件是多凭据格式（数组）
    /// - credentials_path 已设置
    ///
    /// # Returns
    /// - `Ok(true)` - 成功写入文件
    /// - `Ok(false)` - 跳过写入（非多凭据格式或无路径配置）
    /// - `Err(_)` - 写入失败
    fn persist_credentials(&self) -> anyhow::Result<bool> {
        use anyhow::Context;

        // 仅多凭据格式才回写
        if !self.is_multiple_format {
            return Ok(false);
        }

        let path = match &self.credentials_path {
            Some(p) => p,
            None => return Ok(false),
        };

        // 收集所有凭据
        let credentials: Vec<KiroCredentials> = {
            let entries = self.entries.lock();
            entries
                .iter()
                .map(|e| {
                    let mut cred = e.credentials.clone();
                    cred.canonicalize_auth_method();
                    // 同步 disabled 状态到凭据对象
                    cred.disabled = e.disabled;
                    cred
                })
                .collect()
        };

        // 序列化为 pretty JSON
        let json = serde_json::to_string_pretty(&credentials).context("序列化凭据失败")?;

        // 写入文件（在 Tokio runtime 内使用 block_in_place 避免阻塞 worker）
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| std::fs::write(path, &json))
                .with_context(|| format!("回写凭据文件失败: {:?}", path))?;
        } else {
            std::fs::write(path, &json).with_context(|| format!("回写凭据文件失败: {:?}", path))?;
        }

        tracing::debug!("已回写凭据到文件: {:?}", path);
        Ok(true)
    }

    /// 获取缓存目录（凭据文件所在目录）
    pub fn cache_dir(&self) -> Option<PathBuf> {
        self.credentials_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    fn resolve_credentials_sidecar_path(&self, configured: &str, default_name: &str) -> PathBuf {
        let name = if configured.trim().is_empty() {
            default_name
        } else {
            configured
        };
        let path = PathBuf::from(name);
        if path.is_absolute() {
            path
        } else {
            self.cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(path)
        }
    }

    fn premium_vault_path(&self) -> PathBuf {
        self.resolve_credentials_sidecar_path(
            &self.config.premium_model_probe.premium_vault_path,
            "credentials.premium.json",
        )
    }

    fn premium_event_log_path(&self) -> PathBuf {
        self.resolve_credentials_sidecar_path(
            &self.config.premium_model_probe.event_log_path,
            "credentials.premium-events.jsonl",
        )
    }

    /// 统计数据文件路径
    fn stats_path(&self) -> Option<PathBuf> {
        self.cache_dir().map(|d| d.join("kiro_stats.json"))
    }

    /// 从磁盘加载统计数据并应用到当前条目
    fn load_stats(&self) {
        let path = match self.stats_path() {
            Some(p) => p,
            None => return,
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return, // 首次运行时文件不存在
        };

        let stats: HashMap<String, StatsEntry> = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("解析统计缓存失败，将忽略: {}", e);
                return;
            }
        };

        let mut entries = self.entries.lock();
        for entry in entries.iter_mut() {
            if let Some(s) = stats.get(&entry.id.to_string()) {
                entry.success_count = s.success_count;
                entry.last_used_at = s.last_used_at.clone();
            }
        }
        *self.last_stats_save_at.lock() = Some(Instant::now());
        self.stats_dirty.store(false, Ordering::Relaxed);
        tracing::info!("已从缓存加载 {} 条统计数据", stats.len());
    }

    /// 将当前统计数据持久化到磁盘
    fn save_stats(&self) {
        let path = match self.stats_path() {
            Some(p) => p,
            None => return,
        };

        let stats: HashMap<String, StatsEntry> = {
            let entries = self.entries.lock();
            entries
                .iter()
                .map(|e| {
                    (
                        e.id.to_string(),
                        StatsEntry {
                            success_count: e.success_count,
                            last_used_at: e.last_used_at.clone(),
                        },
                    )
                })
                .collect()
        };

        match serde_json::to_string_pretty(&stats) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("保存统计缓存失败: {}", e);
                } else {
                    *self.last_stats_save_at.lock() = Some(Instant::now());
                    self.stats_dirty.store(false, Ordering::Relaxed);
                }
            }
            Err(e) => tracing::warn!("序列化统计数据失败: {}", e),
        }
    }

    /// 标记统计数据已更新，并按 debounce 策略决定是否立即落盘
    fn save_stats_debounced(&self) {
        self.stats_dirty.store(true, Ordering::Relaxed);

        let should_flush = {
            let last = *self.last_stats_save_at.lock();
            match last {
                Some(last_saved_at) => last_saved_at.elapsed() >= STATS_SAVE_DEBOUNCE,
                None => true,
            }
        };

        if should_flush {
            self.save_stats();
        }
    }

    /// 报告指定凭据 API 调用成功
    ///
    /// 重置该凭据的失败计数
    ///
    /// # Arguments
    /// * `id` - 凭据 ID（来自 CallContext）
    pub fn report_success(&self, id: u64) {
        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.failure_count = 0;
                entry.immediate_failure_count = 0;
                entry.refresh_failure_count = 0;
                entry.runtime.clear_cooldown();
                entry.success_count += 1;
                entry.last_used_at = Some(Utc::now().to_rfc3339());
                tracing::debug!(
                    "凭据 #{} API 调用成功（累计 {} 次）",
                    id,
                    entry.success_count
                );
            }
        }
        self.save_stats_debounced();
    }

    /// 报告指定凭据 API 调用失败
    ///
    /// 增加失败计数，达到阈值时禁用凭据并切换到优先级最高的可用凭据
    /// 返回是否还有可用凭据可以重试
    ///
    /// # Arguments
    /// * `id` - 凭据 ID（来自 CallContext）
    pub fn report_failure(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.failure_count += 1;
            entry.last_used_at = Some(Utc::now().to_rfc3339());
            let failure_count = entry.failure_count;

            tracing::warn!(
                "凭据 #{} API 调用失败（{}/{}）",
                id,
                failure_count,
                MAX_FAILURES_PER_CREDENTIAL
            );

            if failure_count >= MAX_FAILURES_PER_CREDENTIAL {
                entry.disabled = true;
                entry.disabled_reason = Some(DisabledReason::TooManyFailures);
                tracing::error!("凭据 #{} 已连续失败 {} 次，已被禁用", id, failure_count);

                // 切换到优先级最高的可用凭据
                if let Some(next) = entries
                    .iter()
                    .filter(|e| !e.disabled)
                    .min_by_key(|e| e.credentials.priority)
                {
                    *current_id = next.id;
                    tracing::info!(
                        "已切换到凭据 #{}（优先级 {}）",
                        next.id,
                        next.credentials.priority
                    );
                } else {
                    tracing::error!("所有凭据均已禁用！");
                }
            }

            entries.iter().any(|e| !e.disabled)
        };
        self.save_stats_debounced();
        result
    }

    /// 报告指定凭据额度已用尽
    ///
    /// 用于处理 402 Payment Required 且 reason 为 `MONTHLY_REQUEST_COUNT` 的场景：
    /// - 立即禁用该凭据（不等待连续失败阈值）
    /// - 切换到下一个可用凭据继续重试
    /// - 返回是否还有可用凭据
    pub fn report_quota_exhausted(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::QuotaExceeded);
            entry.last_used_at = Some(Utc::now().to_rfc3339());
            // 设为阈值，便于在管理面板中直观看到该凭据已不可用
            entry.failure_count = MAX_FAILURES_PER_CREDENTIAL;

            tracing::error!("凭据 #{} 额度已用尽（MONTHLY_REQUEST_COUNT），已被禁用", id);

            // 切换到优先级最高的可用凭据
            if let Some(next) = entries
                .iter()
                .filter(|e| !e.disabled)
                .min_by_key(|e| e.credentials.priority)
            {
                *current_id = next.id;
                tracing::info!(
                    "已切换到凭据 #{}（优先级 {}）",
                    next.id,
                    next.credentials.priority
                );
                true
            } else {
                tracing::error!("所有凭据均已禁用！");
                false
            }
        };
        self.save_stats_debounced();
        result
    }

    /// 报告指定凭据遭遇需要立即禁用的请求失败。
    ///
    /// 用于处理上游明确拒绝当前凭据且无需保留的场景（如 403）：
    /// - 立即禁用该凭据
    /// - 切换到下一个可用凭据继续尝试
    /// - 返回是否还有可用凭据
    pub fn report_immediate_failure(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.immediate_failure_count += 1;
            entry.last_used_at = Some(Utc::now().to_rfc3339());
            let immediate_failure_count = entry.immediate_failure_count;

            tracing::warn!(
                "凭据 #{} 遭遇需立即禁用的请求失败（累计 {} 次）",
                id,
                immediate_failure_count
            );

            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::ImmediateFailure);
            entry.failure_count = MAX_FAILURES_PER_CREDENTIAL;
            entry.runtime.clear_cooldown();

            tracing::error!(
                "凭据 #{} 遭遇快速禁用类请求失败 {} 次，已禁用",
                id,
                immediate_failure_count
            );

            if let Some(next) = entries
                .iter()
                .filter(|e| !e.disabled)
                .min_by_key(|e| e.credentials.priority)
            {
                *current_id = next.id;
                tracing::info!(
                    "已切换到凭据 #{}（优先级 {}）",
                    next.id,
                    next.credentials.priority
                );
                true
            } else {
                tracing::error!("所有凭据均已禁用！");
                false
            }
        };
        self.save_stats_debounced();
        result
    }

    /// 报告指定凭据遭遇可重试但需要临时切换的状态失败。
    ///
    /// 用于处理 `400/429` 这类当前请求不应继续压在同一凭据上的场景：
    /// - 不禁用该凭据
    /// - 标记短冷却，避免所有模式立刻切回同一张凭据
    /// - 返回当前是否还有可立即尝试的其他凭据
    pub fn report_retryable_status_failure(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return false,
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.immediate_failure_count += 1;
            entry.last_used_at = Some(Utc::now().to_rfc3339());
            entry.runtime.set_cooldown_for(RETRYABLE_STATUS_COOLDOWN);

            tracing::warn!(
                "凭据 #{} 遭遇可重试状态失败（累计 {} 次），进入短冷却并切换下一张",
                id,
                entry.immediate_failure_count
            );

            let now_ms = now_epoch_ms();
            entries
                .iter()
                .any(|e| !e.disabled && !e.runtime.is_cooling_down(now_ms))
        };
        self.save_stats_debounced();
        result
    }

    /// 报告指定凭据刷新 Token 失败。
    ///
    /// 连续刷新失败达到阈值后禁用凭据并切换，阈值内保持当前凭据不切换，
    /// 与 API 401/403 的累计失败策略保持一致。
    pub fn report_refresh_failure(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.last_used_at = Some(Utc::now().to_rfc3339());
            entry.refresh_failure_count += 1;
            let refresh_failure_count = entry.refresh_failure_count;

            tracing::warn!(
                "凭据 #{} Token 刷新失败（{}/{}）",
                id,
                refresh_failure_count,
                MAX_FAILURES_PER_CREDENTIAL
            );

            if refresh_failure_count < MAX_FAILURES_PER_CREDENTIAL {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::TooManyRefreshFailures);

            tracing::error!(
                "凭据 #{} Token 已连续刷新失败 {} 次，已被禁用",
                id,
                refresh_failure_count
            );

            if let Some(next) = entries
                .iter()
                .filter(|e| !e.disabled)
                .min_by_key(|e| e.credentials.priority)
            {
                *current_id = next.id;
                tracing::info!(
                    "已切换到凭据 #{}（优先级 {}）",
                    next.id,
                    next.credentials.priority
                );
                true
            } else {
                tracing::error!("所有凭据均已禁用！");
                false
            }
        };
        self.save_stats_debounced();
        result
    }

    /// 报告指定凭据的 refreshToken 永久失效（invalid_grant）。
    ///
    /// 立即禁用凭据，不累计、不重试。
    /// 返回是否还有可用凭据。
    pub fn report_refresh_token_invalid(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.last_used_at = Some(Utc::now().to_rfc3339());
            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::InvalidRefreshToken);

            tracing::error!(
                "凭据 #{} refreshToken 已失效 (invalid_grant)，已立即禁用",
                id
            );

            if let Some(next) = entries
                .iter()
                .filter(|e| !e.disabled)
                .min_by_key(|e| e.credentials.priority)
            {
                *current_id = next.id;
                tracing::info!(
                    "已切换到凭据 #{}（优先级 {}）",
                    next.id,
                    next.credentials.priority
                );
                true
            } else {
                tracing::error!("所有凭据均已禁用！");
                false
            }
        };
        self.save_stats_debounced();
        result
    }

    /// 切换到优先级最高的可用凭据
    ///
    /// 返回是否成功切换
    pub fn switch_to_next(&self) -> bool {
        let entries = self.entries.lock();
        let mut current_id = self.current_id.lock();

        // 选择优先级最高的未禁用凭据（排除当前凭据）
        if let Some(next) = entries
            .iter()
            .filter(|e| !e.disabled && e.id != *current_id)
            .min_by_key(|e| e.credentials.priority)
        {
            *current_id = next.id;
            tracing::info!(
                "已切换到凭据 #{}（优先级 {}）",
                next.id,
                next.credentials.priority
            );
            true
        } else {
            // 没有其他可用凭据，检查当前凭据是否可用
            entries.iter().any(|e| e.id == *current_id && !e.disabled)
        }
    }

    // ========================================================================
    // Admin API 方法
    // ========================================================================

    /// 获取管理器状态快照（用于 Admin API）
    pub fn snapshot(&self) -> ManagerSnapshot {
        let entries = self.entries.lock();
        let current_id = *self.current_id.lock();
        let available = entries.iter().filter(|e| !e.disabled).count();

        ManagerSnapshot {
            entries: entries
                .iter()
                .map(|e| CredentialEntrySnapshot {
                    id: e.id,
                    priority: e.credentials.priority,
                    disabled: e.disabled,
                    failure_count: e.failure_count,
                    auth_method: if e.credentials.is_api_key_credential() {
                        Some("api_key".to_string())
                    } else {
                        e.credentials.auth_method.as_deref().map(|m| {
                            if m.eq_ignore_ascii_case("builder-id") || m.eq_ignore_ascii_case("iam")
                            {
                                "idc".to_string()
                            } else {
                                m.to_string()
                            }
                        })
                    },
                    has_profile_arn: e.credentials.profile_arn.is_some(),
                    expires_at: if e.credentials.is_api_key_credential() {
                        None // API Key 凭据本地不维护过期时间（服务端策略未知）
                    } else {
                        e.credentials.expires_at.clone()
                    },
                    refresh_token_hash: if e.credentials.is_api_key_credential() {
                        None
                    } else {
                        e.credentials.refresh_token.as_deref().map(sha256_hex)
                    },
                    api_key_hash: if e.credentials.is_api_key_credential() {
                        e.credentials.kiro_api_key.as_deref().map(sha256_hex)
                    } else {
                        None
                    },
                    masked_api_key: if e.credentials.is_api_key_credential() {
                        e.credentials.kiro_api_key.as_deref().map(mask_api_key)
                    } else {
                        None
                    },
                    email: e.credentials.email.clone(),
                    success_count: e.success_count,
                    last_used_at: e.last_used_at.clone(),
                    has_proxy: e.credentials.proxy_url.is_some(),
                    proxy_url: e.credentials.proxy_url.clone(),
                    refresh_failure_count: e.refresh_failure_count,
                    disabled_reason: e.disabled_reason.map(|r| {
                        match r {
                            DisabledReason::Manual => "Manual",
                            DisabledReason::TooManyFailures => "TooManyFailures",
                            DisabledReason::ImmediateFailure => "ImmediateFailure",
                            DisabledReason::TooManyRefreshFailures => "TooManyRefreshFailures",
                            DisabledReason::QuotaExceeded => "QuotaExceeded",
                            DisabledReason::InvalidRefreshToken => "InvalidRefreshToken",
                            DisabledReason::InvalidConfig => "InvalidConfig",
                        }
                        .to_string()
                    }),
                    endpoint: e.credentials.endpoint.clone(),
                    premium_model_access: e.credentials.premium_model_access,
                    premium_model_access_checked_at: e
                        .credentials
                        .premium_model_access_checked_at
                        .clone(),
                    premium_model_access_probe_model: e
                        .credentials
                        .premium_model_access_probe_model
                        .clone(),
                    premium_model_access_source_model: e
                        .credentials
                        .premium_model_access_source_model
                        .clone(),
                    premium_model_access_last_error: e
                        .credentials
                        .premium_model_access_last_error
                        .clone(),
                    premium_vault_status: e.credentials.premium_vault_status.clone(),
                })
                .collect(),
            current_id,
            total: entries.len(),
            available,
        }
    }

    /// 设置凭据禁用状态（Admin API）
    pub fn set_disabled(&self, id: u64, disabled: bool) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            entry.disabled = disabled;
            if !disabled {
                // 启用时重置失败计数
                entry.failure_count = 0;
                entry.immediate_failure_count = 0;
                entry.refresh_failure_count = 0;
                entry.runtime.clear_cooldown();
                entry.disabled_reason = None;
            } else {
                entry.disabled_reason = Some(DisabledReason::Manual);
                entry.runtime.clear_cooldown();
            }
        }
        // 持久化更改
        self.persist_credentials()?;
        Ok(())
    }

    /// 设置凭据优先级（Admin API）
    ///
    /// 修改优先级后会立即按新优先级重新选择当前凭据。
    /// 即使持久化失败，内存中的优先级和当前凭据选择也会生效。
    pub fn set_priority(&self, id: u64, priority: u32) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            entry.credentials.priority = priority;
        }
        // 立即按新优先级重新选择当前凭据（无论持久化是否成功）
        self.select_highest_priority();
        // 持久化更改
        self.persist_credentials()?;
        Ok(())
    }

    /// 重置凭据失败计数并重新启用（Admin API）
    pub fn reset_and_enable(&self, id: u64) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            if entry.disabled_reason == Some(DisabledReason::InvalidConfig) {
                anyhow::bail!("凭据 #{} 因配置无效被禁用，请修正配置后重启服务", id);
            }
            entry.failure_count = 0;
            entry.immediate_failure_count = 0;
            entry.refresh_failure_count = 0;
            entry.disabled = false;
            entry.disabled_reason = None;
            entry.runtime.clear_cooldown();
        }
        // 持久化更改
        self.persist_credentials()?;
        Ok(())
    }

    /// 批量重置所有凭据失败计数并重新启用。
    ///
    /// 仅跳过因配置无效而禁用的凭据，其余凭据统一在内存中批量修改，
    /// 最后只进行一次持久化，避免大凭据文件被重复全量回写。
    pub fn reset_and_enable_all(&self) -> anyhow::Result<ResetAllCredentialsResult> {
        let result = {
            let mut entries = self.entries.lock();
            let mut reset_count = 0;
            let mut skipped_invalid_config_count = 0;
            let mut unchanged_count = 0;

            for entry in entries.iter_mut() {
                if entry.disabled_reason == Some(DisabledReason::InvalidConfig) {
                    skipped_invalid_config_count += 1;
                    continue;
                }

                let needs_reset = entry.disabled
                    || entry.failure_count > 0
                    || entry.immediate_failure_count > 0
                    || entry.refresh_failure_count > 0
                    || entry.disabled_reason.is_some();

                if !needs_reset {
                    unchanged_count += 1;
                    continue;
                }

                entry.failure_count = 0;
                entry.immediate_failure_count = 0;
                entry.refresh_failure_count = 0;
                entry.disabled = false;
                entry.disabled_reason = None;
                entry.runtime.clear_cooldown();
                reset_count += 1;
            }

            ResetAllCredentialsResult {
                reset_count,
                skipped_invalid_config_count,
                unchanged_count,
            }
        };

        self.select_highest_priority();

        if result.reset_count > 0 {
            self.persist_credentials()?;
        }

        Ok(result)
    }

    /// 获取指定凭据的使用额度（Admin API）
    pub async fn get_usage_limits_for(&self, id: u64) -> anyhow::Result<UsageLimitsResponse> {
        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        // API Key 凭据直接使用 kiro_api_key，无需刷新
        let token = if credentials.is_api_key_credential() {
            credentials
                .kiro_api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("API Key 凭据缺少 kiroApiKey"))?
        } else {
            // 检查是否需要刷新 token
            let needs_refresh =
                is_token_expired(&credentials) || is_token_expiring_soon(&credentials);

            if needs_refresh {
                let refresh_lock = self.refresh_lock_for(id);
                let _guard = refresh_lock.lock().await;
                let current_creds = {
                    let entries = self.entries.lock();
                    entries
                        .iter()
                        .find(|e| e.id == id)
                        .map(|e| e.credentials.clone())
                        .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
                };

                if is_token_expired(&current_creds) || is_token_expiring_soon(&current_creds) {
                    let effective_proxy = current_creds.effective_proxy(self.proxy.as_ref());
                    let new_creds =
                        refresh_token(&current_creds, &self.config, effective_proxy.as_ref())
                            .await?;
                    {
                        let mut entries = self.entries.lock();
                        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                            entry.credentials = new_creds.clone();
                        }
                    }
                    // 持久化失败只记录警告，不影响本次请求
                    if let Err(e) = self.persist_credentials() {
                        tracing::warn!("Token 刷新后持久化失败（不影响本次请求）: {}", e);
                    }
                    new_creds
                        .access_token
                        .ok_or_else(|| anyhow::anyhow!("刷新后无 access_token"))?
                } else {
                    current_creds
                        .access_token
                        .ok_or_else(|| anyhow::anyhow!("凭据无 access_token"))?
                }
            } else {
                credentials
                    .access_token
                    .ok_or_else(|| anyhow::anyhow!("凭据无 access_token"))?
            }
        };

        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        let effective_proxy = credentials.effective_proxy(self.proxy.as_ref());
        let usage_limits =
            get_usage_limits(&credentials, &self.config, &token, effective_proxy.as_ref()).await?;

        // 更新订阅等级到凭据（仅在发生变化时持久化）
        if let Some(subscription_title) = usage_limits.subscription_title() {
            let changed = {
                let mut entries = self.entries.lock();
                if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                    let old_title = entry.credentials.subscription_title.clone();
                    if old_title.as_deref() != Some(subscription_title) {
                        entry.credentials.subscription_title = Some(subscription_title.to_string());
                        tracing::info!(
                            "凭据 #{} 订阅等级已更新: {:?} -> {}",
                            id,
                            old_title,
                            subscription_title
                        );
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            if changed {
                if let Err(e) = self.persist_credentials() {
                    tracing::warn!("订阅等级更新后持久化失败（不影响本次请求）: {}", e);
                }
            }
        }

        Ok(usage_limits)
    }

    /// 添加新凭据（Admin API）
    ///
    /// # 流程
    /// 1. 验证凭据基本字段（API Key: kiroApiKey 不为空; OAuth: refreshToken 不为空）
    /// 2. 基于 kiroApiKey 或 refreshToken 的 SHA-256 哈希检测重复
    /// 3. OAuth: 尝试刷新 Token 验证凭据有效性; API Key: 跳过
    /// 4. 分配新 ID（当前最大 ID + 1）
    /// 5. 添加到 entries 列表
    /// 6. 持久化到配置文件
    ///
    /// # 返回
    /// - `Ok(u64)` - 新凭据 ID
    /// - `Err(_)` - 验证失败或添加失败
    pub async fn add_credential(&self, new_cred: KiroCredentials) -> anyhow::Result<u64> {
        // 1. 基本验证
        if new_cred.is_api_key_credential() {
            let api_key = new_cred
                .kiro_api_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("API Key 凭据缺少 kiroApiKey"))?;
            if api_key.is_empty() {
                anyhow::bail!("kiroApiKey 为空");
            }
        } else {
            validate_refresh_token(&new_cred)?;
        }

        // 2. 基于哈希检测重复
        if new_cred.is_api_key_credential() {
            let new_api_key = new_cred
                .kiro_api_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("缺少 kiroApiKey"))?;
            let new_api_key_hash = sha256_hex(new_api_key);
            let duplicate_exists = {
                let entries = self.entries.lock();
                entries.iter().any(|entry| {
                    entry
                        .credentials
                        .kiro_api_key
                        .as_deref()
                        .map(sha256_hex)
                        .as_deref()
                        == Some(new_api_key_hash.as_str())
                })
            };
            if duplicate_exists {
                anyhow::bail!("凭据已存在（kiroApiKey 重复）");
            }
        } else {
            let new_refresh_token = new_cred
                .refresh_token
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("缺少 refreshToken"))?;
            let new_refresh_token_hash = sha256_hex(new_refresh_token);
            let duplicate_exists = {
                let entries = self.entries.lock();
                entries.iter().any(|entry| {
                    entry
                        .credentials
                        .refresh_token
                        .as_deref()
                        .map(sha256_hex)
                        .as_deref()
                        == Some(new_refresh_token_hash.as_str())
                })
            };
            if duplicate_exists {
                anyhow::bail!("凭据已存在（refreshToken 重复）");
            }
        }

        // 3. 验证凭据有效性（API Key 无需网络刷新）
        let mut validated_cred = if new_cred.is_api_key_credential() {
            new_cred.clone()
        } else {
            let effective_proxy = new_cred.effective_proxy(self.proxy.as_ref());
            refresh_token(&new_cred, &self.config, effective_proxy.as_ref()).await?
        };

        // 4. 分配新 ID
        let new_id = {
            let entries = self.entries.lock();
            entries.iter().map(|e| e.id).max().unwrap_or(0) + 1
        };

        // 5. 设置 ID 并保留用户输入的元数据
        validated_cred.id = Some(new_id);
        validated_cred.priority = new_cred.priority;
        validated_cred.auth_method = new_cred.auth_method.map(|m| {
            if m.eq_ignore_ascii_case("builder-id") || m.eq_ignore_ascii_case("iam") {
                "idc".to_string()
            } else {
                m
            }
        });
        validated_cred.client_id = new_cred.client_id;
        validated_cred.client_secret = new_cred.client_secret;
        validated_cred.region = new_cred.region;
        validated_cred.auth_region = new_cred.auth_region;
        validated_cred.api_region = new_cred.api_region;
        validated_cred.machine_id = new_cred.machine_id;
        validated_cred.email = new_cred.email;
        validated_cred.proxy_url = new_cred.proxy_url;
        validated_cred.proxy_username = new_cred.proxy_username;
        validated_cred.proxy_password = new_cred.proxy_password;
        validated_cred.kiro_api_key = new_cred.kiro_api_key;

        {
            let mut entries = self.entries.lock();
            entries.push(CredentialEntry {
                id: new_id,
                credentials: validated_cred,
                failure_count: 0,
                immediate_failure_count: 0,
                refresh_failure_count: 0,
                disabled: false,
                disabled_reason: None,
                success_count: 0,
                last_used_at: None,
                runtime: Arc::new(CredentialRuntimeState::new()),
            });
        }

        // 6. 持久化
        self.persist_credentials()?;

        tracing::info!("成功添加凭据 #{}", new_id);
        Ok(new_id)
    }

    /// 删除凭据（Admin API）
    ///
    /// # 前置条件
    /// - 凭据必须已禁用（disabled = true）
    ///
    /// # 行为
    /// 1. 验证凭据存在
    /// 2. 验证凭据已禁用
    /// 3. 从 entries 移除
    /// 4. 如果删除的是当前凭据，切换到优先级最高的可用凭据
    /// 5. 如果删除后没有凭据，将 current_id 重置为 0
    /// 6. 持久化到文件
    ///
    /// # 返回
    /// - `Ok(())` - 删除成功
    /// - `Err(_)` - 凭据不存在、未禁用或持久化失败
    pub fn delete_credential(&self, id: u64) -> anyhow::Result<()> {
        let was_current = {
            let mut entries = self.entries.lock();

            // 查找凭据
            let entry = entries
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;

            // 检查是否已禁用
            if !entry.disabled {
                anyhow::bail!("只能删除已禁用的凭据（请先禁用凭据 #{}）", id);
            }

            // 记录是否是当前凭据
            let current_id = *self.current_id.lock();
            let was_current = current_id == id;

            // 删除凭据
            entries.retain(|e| e.id != id);

            was_current
        };

        // 如果删除的是当前凭据，切换到优先级最高的可用凭据
        if was_current {
            self.select_highest_priority();
        }

        // 如果删除后没有任何凭据，将 current_id 重置为 0（与初始化行为保持一致）
        {
            let entries = self.entries.lock();
            if entries.is_empty() {
                let mut current_id = self.current_id.lock();
                *current_id = 0;
                tracing::info!("所有凭据已删除，current_id 已重置为 0");
            }
        }

        // 持久化更改
        self.persist_credentials()?;

        // 立即回写统计数据，清除已删除凭据的残留条目
        self.save_stats();

        tracing::info!("已删除凭据 #{}", id);
        Ok(())
    }

    /// 批量清除 `ImmediateFailure` 状态的已禁用凭据（Admin API）
    pub fn clear_immediate_failure_disabled(
        &self,
    ) -> anyhow::Result<ClearImmediateFailureDisabledResult> {
        let (result, removed_current, has_entries, next_current_id) = {
            let mut entries = self.entries.lock();
            let current_id = *self.current_id.lock();

            let mut cleared_count = 0;
            let mut skipped_other_disabled_count = 0;
            let mut unchanged_count = 0;
            let mut removed_current = false;

            for entry in entries.iter() {
                if entry.disabled && entry.disabled_reason == Some(DisabledReason::ImmediateFailure)
                {
                    cleared_count += 1;
                    if entry.id == current_id {
                        removed_current = true;
                    }
                } else if entry.disabled {
                    skipped_other_disabled_count += 1;
                } else {
                    unchanged_count += 1;
                }
            }

            entries.retain(|entry| {
                !(entry.disabled && entry.disabled_reason == Some(DisabledReason::ImmediateFailure))
            });

            let next_current_id = entries
                .iter()
                .filter(|entry| !entry.disabled)
                .min_by_key(|entry| entry.credentials.priority)
                .map(|entry| entry.id);

            (
                ClearImmediateFailureDisabledResult {
                    cleared_count,
                    skipped_other_disabled_count,
                    unchanged_count,
                },
                removed_current,
                !entries.is_empty(),
                next_current_id,
            )
        };

        if !has_entries {
            let mut current_id = self.current_id.lock();
            *current_id = 0;
            tracing::info!("所有凭据已删除，current_id 已重置为 0");
        } else if removed_current {
            let mut current_id = self.current_id.lock();
            *current_id = next_current_id.unwrap_or(0);
        }

        if result.cleared_count > 0 {
            self.persist_credentials()?;
            self.save_stats();
            tracing::info!(
                "已批量清除 {} 个 ImmediateFailure 状态的已禁用凭据",
                result.cleared_count
            );
        }

        Ok(result)
    }

    /// 强制刷新指定凭据的 Token（Admin API）
    ///
    /// 无条件调用上游 API 重新获取 access token，不检查是否过期。
    /// 适用于排查问题、Token 异常但未过期、主动更新凭据状态等场景。
    pub async fn force_refresh_token_for(&self, id: u64) -> anyhow::Result<()> {
        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        // 获取刷新锁防止并发刷新
        let refresh_lock = self.refresh_lock_for(id);
        let _guard = refresh_lock.lock().await;

        // 无条件调用 refresh_token
        let effective_proxy = credentials.effective_proxy(self.proxy.as_ref());
        let new_creds = refresh_token(&credentials, &self.config, effective_proxy.as_ref()).await?;

        // 更新 entries 中对应凭据
        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.credentials = new_creds;
                entry.refresh_failure_count = 0;
            }
        }

        // 持久化
        if let Err(e) = self.persist_credentials() {
            tracing::warn!("强制刷新 Token 后持久化失败: {}", e);
        }

        tracing::info!("凭据 #{} Token 已强制刷新", id);
        Ok(())
    }

    /// 获取负载均衡模式（Admin API）
    pub fn get_load_balancing_mode(&self) -> String {
        self.load_balancing_mode.lock().clone()
    }

    fn persist_load_balancing_mode(&self, mode: &str) -> anyhow::Result<()> {
        use anyhow::Context;

        let config_path = match self.config.config_path() {
            Some(path) => path.to_path_buf(),
            None => {
                tracing::warn!("配置文件路径未知，负载均衡模式仅在当前进程生效: {}", mode);
                return Ok(());
            }
        };

        let mut config = Config::load(&config_path)
            .with_context(|| format!("重新加载配置失败: {}", config_path.display()))?;
        config.load_balancing_mode = mode.to_string();
        config
            .save()
            .with_context(|| format!("持久化负载均衡模式失败: {}", config_path.display()))?;

        Ok(())
    }

    /// 设置负载均衡模式（Admin API）
    pub fn set_load_balancing_mode(&self, mode: String) -> anyhow::Result<()> {
        // 验证模式值
        if mode != "priority"
            && mode != "balanced"
            && mode != "round_robin"
            && mode != "adaptive_round_robin"
        {
            anyhow::bail!("无效的负载均衡模式: {}", mode);
        }

        let previous_mode = self.get_load_balancing_mode();
        if previous_mode == mode {
            return Ok(());
        }

        *self.load_balancing_mode.lock() = mode.clone();

        if let Err(err) = self.persist_load_balancing_mode(&mode) {
            *self.load_balancing_mode.lock() = previous_mode;
            return Err(err);
        }

        tracing::info!("负载均衡模式已设置为: {}", mode);
        Ok(())
    }
}

impl Drop for MultiTokenManager {
    fn drop(&mut self) {
        if self.stats_dirty.load(Ordering::Relaxed) {
            self.save_stats();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_is_token_expired_with_expired_token() {
        let mut credentials = KiroCredentials::default();
        credentials.expires_at = Some("2020-01-01T00:00:00Z".to_string());
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_move_credential_to_premium_vault_writes_vault_and_removes_active() {
        let dir = std::env::temp_dir().join(format!("kiro-premium-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let credentials_path = dir.join("credentials.json");

        let mut credential = KiroCredentials::default();
        credential.id = Some(1);
        credential.refresh_token = Some("test-refresh-token".to_string());
        credential.access_token = Some("test-access-token".to_string());
        credential.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        credential.machine_id = Some("a".repeat(64));

        std::fs::write(
            &credentials_path,
            serde_json::to_string_pretty(&vec![credential.clone()]).unwrap(),
        )
        .unwrap();

        let manager = MultiTokenManager::new(
            Config::default(),
            vec![credential],
            None,
            Some(credentials_path.clone()),
            true,
        )
        .unwrap();

        let proof = PremiumVaultProof {
            source_model: "claude-sonnet-4-5-20250929".to_string(),
            target_model: "claude-sonnet-4-6".to_string(),
            status: 200,
        };

        manager.move_credential_to_premium_vault(1, &proof).unwrap();

        let active: Vec<KiroCredentials> =
            serde_json::from_str(&std::fs::read_to_string(&credentials_path).unwrap()).unwrap();
        assert!(active.is_empty());

        let vault_path = dir.join("credentials.premium.json");
        let vault: Vec<KiroCredentials> =
            serde_json::from_str(&std::fs::read_to_string(&vault_path).unwrap()).unwrap();
        assert_eq!(vault.len(), 1);
        assert_eq!(vault[0].id, Some(1));
        assert_eq!(vault[0].premium_model_access, Some(true));
        assert_eq!(
            vault[0].premium_model_access_probe_model.as_deref(),
            Some("claude-sonnet-4-6")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_token_expired_with_valid_token() {
        let mut credentials = KiroCredentials::default();
        let future = Utc::now() + Duration::hours(1);
        credentials.expires_at = Some(future.to_rfc3339());
        assert!(!is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_within_5_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(3);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_no_expires_at() {
        let credentials = KiroCredentials::default();
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expiring_soon_within_10_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(8);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(is_token_expiring_soon(&credentials));
    }

    #[test]
    fn test_is_token_expiring_soon_beyond_10_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(15);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(!is_token_expiring_soon(&credentials));
    }

    #[test]
    fn test_validate_refresh_token_missing() {
        let credentials = KiroCredentials::default();
        let result = validate_refresh_token(&credentials);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_refresh_token_valid() {
        let mut credentials = KiroCredentials::default();
        credentials.refresh_token = Some("a".repeat(150));
        let result = validate_refresh_token(&credentials);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sha256_hex() {
        let result = sha256_hex("test");
        assert_eq!(
            result,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }

    #[tokio::test]
    async fn test_refresh_token_rejects_api_key_credential() {
        let config = Config::default();
        let mut credentials = KiroCredentials::default();
        credentials.kiro_api_key = Some("ksk_test_key_123".to_string());
        credentials.auth_method = Some("api_key".to_string());

        let result = refresh_token(&credentials, &config, None).await;

        assert!(result.is_err(), "API Key 凭据应被 refresh_token 拒绝");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("API Key 凭据不支持刷新"),
            "期望错误消息包含 'API Key 凭据不支持刷新'，实际: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_add_credential_reject_duplicate_refresh_token() {
        let config = Config::default();

        let mut existing = KiroCredentials::default();
        existing.refresh_token = Some("a".repeat(150));

        let manager = MultiTokenManager::new(config, vec![existing], None, None, false).unwrap();

        let mut duplicate = KiroCredentials::default();
        duplicate.refresh_token = Some("a".repeat(150));

        let result = manager.add_credential(duplicate).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("凭据已存在"));
    }

    #[tokio::test]
    async fn test_add_credential_api_key_success() {
        let config = Config::default();
        let manager = MultiTokenManager::new(config, vec![], None, None, false).unwrap();

        let mut api_key_cred = KiroCredentials::default();
        api_key_cred.kiro_api_key = Some("ksk_test_key_123".to_string());
        api_key_cred.auth_method = Some("api_key".to_string());

        let result = manager.add_credential(api_key_cred).await;
        assert!(result.is_ok());
        let id = result.unwrap();
        assert!(id > 0);
        assert_eq!(manager.total_count(), 1);
        assert_eq!(manager.available_count(), 1);
    }

    #[tokio::test]
    async fn test_add_credential_reject_duplicate_api_key() {
        let config = Config::default();

        let mut existing = KiroCredentials::default();
        existing.kiro_api_key = Some("ksk_existing_key".to_string());
        existing.auth_method = Some("api_key".to_string());

        let manager = MultiTokenManager::new(config, vec![existing], None, None, false).unwrap();

        let mut duplicate = KiroCredentials::default();
        duplicate.kiro_api_key = Some("ksk_existing_key".to_string());
        duplicate.auth_method = Some("api_key".to_string());

        let result = manager.add_credential(duplicate).await;
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("kiroApiKey 重复")
        );
    }

    #[tokio::test]
    async fn test_add_credential_api_key_empty_rejected() {
        let config = Config::default();
        let manager = MultiTokenManager::new(config, vec![], None, None, false).unwrap();

        let mut cred = KiroCredentials::default();
        cred.kiro_api_key = Some(String::new());
        cred.auth_method = Some("api_key".to_string());

        let result = manager.add_credential(cred).await;
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("kiroApiKey 为空")
        );
    }

    #[tokio::test]
    async fn test_add_credential_api_key_missing_key_rejected() {
        let config = Config::default();
        let manager = MultiTokenManager::new(config, vec![], None, None, false).unwrap();

        let mut cred = KiroCredentials::default();
        cred.auth_method = Some("api_key".to_string());
        // kiro_api_key is None

        let result = manager.add_credential(cred).await;
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("缺少 kiroApiKey")
        );
    }

    #[tokio::test]
    async fn test_add_credential_api_key_and_oauth_coexist() {
        let config = Config::default();

        let mut oauth_cred = KiroCredentials::default();
        oauth_cred.refresh_token = Some("a".repeat(150));

        let manager = MultiTokenManager::new(config, vec![oauth_cred], None, None, false).unwrap();

        let mut api_key_cred = KiroCredentials::default();
        api_key_cred.kiro_api_key = Some("ksk_new_key".to_string());
        api_key_cred.auth_method = Some("api_key".to_string());

        let result = manager.add_credential(api_key_cred).await;
        assert!(result.is_ok());
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 2);
    }

    // MultiTokenManager 测试

    #[test]
    fn test_multi_token_manager_new() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.priority = 0;
        let mut cred2 = KiroCredentials::default();
        cred2.priority = 1;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 2);
    }

    #[test]
    fn test_multi_token_manager_empty_credentials() {
        let config = Config::default();
        let result = MultiTokenManager::new(config, vec![], None, None, false);
        // 支持 0 个凭据启动（可通过管理面板添加）
        assert!(result.is_ok());
        let manager = result.unwrap();
        assert_eq!(manager.total_count(), 0);
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_duplicate_ids() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.id = Some(1);
        let mut cred2 = KiroCredentials::default();
        cred2.id = Some(1); // 重复 ID

        let result = MultiTokenManager::new(config, vec![cred1, cred2], None, None, false);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("重复的凭据 ID"),
            "错误消息应包含 '重复的凭据 ID'，实际: {}",
            err_msg
        );
    }

    #[test]
    fn test_multi_token_manager_api_key_missing_kiro_api_key_auto_disabled() {
        let config = Config::default();

        // auth_method=api_key 但缺少 kiro_api_key → 应被自动禁用
        let mut bad_cred = KiroCredentials::default();
        bad_cred.auth_method = Some("api_key".to_string());
        // kiro_api_key 保持 None

        let mut good_cred = KiroCredentials::default();
        good_cred.refresh_token = Some("valid_token".to_string());

        let manager =
            MultiTokenManager::new(config, vec![bad_cred, good_cred], None, None, false).unwrap();
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 1); // bad_cred 被禁用，只剩 1 个可用
    }

    #[test]
    fn test_multi_token_manager_api_key_with_kiro_api_key_not_disabled() {
        let config = Config::default();

        // auth_method=api_key 且有 kiro_api_key → 不应被禁用
        let mut cred = KiroCredentials::default();
        cred.auth_method = Some("api_key".to_string());
        cred.kiro_api_key = Some("ksk_test123".to_string());

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();
        assert_eq!(manager.total_count(), 1);
        assert_eq!(manager.available_count(), 1);
    }

    #[test]
    fn test_multi_token_manager_report_failure() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        // 前两次失败不会禁用（使用 ID 1）
        assert!(manager.report_failure(1));
        assert!(manager.report_failure(1));
        assert_eq!(manager.available_count(), 2);

        // 第三次失败会禁用第一个凭据
        assert!(manager.report_failure(1));
        assert_eq!(manager.available_count(), 1);

        // 继续失败第二个凭据（使用 ID 2）
        assert!(manager.report_failure(2));
        assert!(manager.report_failure(2));
        assert!(!manager.report_failure(2)); // 所有凭据都禁用了
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_report_success() {
        let config = Config::default();
        let cred = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        // 失败两次（使用 ID 1）
        manager.report_failure(1);
        manager.report_failure(1);

        // 成功后重置计数（使用 ID 1）
        manager.report_success(1);

        // 再失败两次不会禁用
        manager.report_failure(1);
        manager.report_failure(1);
        assert_eq!(manager.available_count(), 1);
    }

    #[test]
    fn test_multi_token_manager_switch_to_next() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.refresh_token = Some("token1".to_string());
        let mut cred2 = KiroCredentials::default();
        cred2.refresh_token = Some("token2".to_string());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        let initial_id = manager.snapshot().current_id;

        // 切换到下一个
        assert!(manager.switch_to_next());
        assert_ne!(manager.snapshot().current_id, initial_id);
    }

    #[test]
    fn test_set_load_balancing_mode_persists_to_config_file() {
        let config_path =
            std::env::temp_dir().join(format!("kiro-load-balancing-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&config_path, r#"{"loadBalancingMode":"priority"}"#).unwrap();

        let config = Config::load(&config_path).unwrap();
        let manager =
            MultiTokenManager::new(config, vec![KiroCredentials::default()], None, None, false)
                .unwrap();

        manager
            .set_load_balancing_mode("balanced".to_string())
            .unwrap();

        let persisted = Config::load(&config_path).unwrap();
        assert_eq!(persisted.load_balancing_mode, "balanced");
        assert_eq!(manager.get_load_balancing_mode(), "balanced");

        std::fs::remove_file(&config_path).unwrap();
    }

    #[test]
    fn test_set_load_balancing_mode_accepts_round_robin() {
        let config_path = std::env::temp_dir().join(format!(
            "kiro-load-balancing-round-robin-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&config_path, r#"{"loadBalancingMode":"priority"}"#).unwrap();

        let config = Config::load(&config_path).unwrap();
        let manager =
            MultiTokenManager::new(config, vec![KiroCredentials::default()], None, None, false)
                .unwrap();

        manager
            .set_load_balancing_mode("round_robin".to_string())
            .unwrap();

        let persisted = Config::load(&config_path).unwrap();
        assert_eq!(persisted.load_balancing_mode, "round_robin");
        assert_eq!(manager.get_load_balancing_mode(), "round_robin");

        std::fs::remove_file(&config_path).unwrap();
    }

    #[test]
    fn test_set_load_balancing_mode_accepts_adaptive_round_robin() {
        let config_path = std::env::temp_dir().join(format!(
            "kiro-load-balancing-adaptive-round-robin-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&config_path, r#"{"loadBalancingMode":"priority"}"#).unwrap();

        let config = Config::load(&config_path).unwrap();
        let manager =
            MultiTokenManager::new(config, vec![KiroCredentials::default()], None, None, false)
                .unwrap();

        manager
            .set_load_balancing_mode("adaptive_round_robin".to_string())
            .unwrap();

        let persisted = Config::load(&config_path).unwrap();
        assert_eq!(persisted.load_balancing_mode, "adaptive_round_robin");
        assert_eq!(manager.get_load_balancing_mode(), "adaptive_round_robin");

        std::fs::remove_file(&config_path).unwrap();
    }

    #[test]
    fn test_multi_token_manager_refresh_lock_is_per_credential() {
        let config = Config::default();
        let manager = MultiTokenManager::new(
            config,
            vec![KiroCredentials::default(), KiroCredentials::default()],
            None,
            None,
            false,
        )
        .unwrap();

        let first = manager.refresh_lock_for(1);
        let first_again = manager.refresh_lock_for(1);
        let second = manager.refresh_lock_for(2);

        assert!(Arc::ptr_eq(&first, &first_again));
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_auto_recovers_all_disabled() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(1);
        }
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(2);
        }

        assert_eq!(manager.available_count(), 0);

        // 应触发自愈：重置失败计数并重新启用，避免必须重启进程
        let ctx = manager.acquire_context(None).await.unwrap();
        assert!(ctx.token == "t1" || ctx.token == "t2");
        assert_eq!(manager.available_count(), 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_balanced_retries_until_bad_credential_disabled()
     {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut bad_cred = KiroCredentials::default();
        bad_cred.priority = 0;
        bad_cred.refresh_token = Some("bad".to_string());

        let mut good_cred = KiroCredentials::default();
        good_cred.priority = 1;
        good_cred.access_token = Some("good-token".to_string());
        good_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![bad_cred, good_cred], None, None, false).unwrap();

        let ctx = manager.acquire_context(None).await.unwrap();
        assert_eq!(ctx.id, 2);
        assert_eq!(ctx.token, "good-token");
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_round_robin_rotates_credentials() {
        let mut config = Config::default();
        config.load_balancing_mode = "round_robin".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("token-1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("token-2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred3 = KiroCredentials::default();
        cred3.access_token = Some("token-3".to_string());
        cred3.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2, cred3], None, None, false).unwrap();

        let first = manager.acquire_context(None).await.unwrap();
        let second = manager.acquire_context(None).await.unwrap();
        let third = manager.acquire_context(None).await.unwrap();
        let fourth = manager.acquire_context(None).await.unwrap();

        assert_eq!(
            vec![first.id, second.id, third.id, fourth.id],
            vec![1, 2, 3, 1]
        );
    }

    #[tokio::test]
    async fn test_multi_token_manager_adaptive_round_robin_skips_inflight_credentials() {
        let mut config = Config::default();
        config.load_balancing_mode = "adaptive_round_robin".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.kiro_api_key = Some("token-1".to_string());
        let mut cred2 = KiroCredentials::default();
        cred2.kiro_api_key = Some("token-2".to_string());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        let first = manager.acquire_context(None).await.unwrap();
        let second = manager.acquire_context(None).await.unwrap();

        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
        assert_eq!(manager.runtime_metrics().in_flight, 2);

        drop(first);
        assert_eq!(manager.runtime_metrics().in_flight, 1);

        drop(second);
        assert_eq!(manager.runtime_metrics().in_flight, 0);
    }

    #[tokio::test]
    async fn test_multi_token_manager_adaptive_round_robin_prefers_successful_credentials() {
        let mut config = Config::default();
        config.load_balancing_mode = "adaptive_round_robin".to_string();

        let mut unproven = KiroCredentials::default();
        unproven.kiro_api_key = Some("unproven-token".to_string());
        let mut proven = KiroCredentials::default();
        proven.kiro_api_key = Some("proven-token".to_string());

        let manager =
            MultiTokenManager::new(config, vec![unproven, proven], None, None, false).unwrap();
        manager.report_success(2);

        let ctx = manager.acquire_context(None).await.unwrap();

        assert_eq!(ctx.id, 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_adaptive_round_robin_cools_down_first_immediate_failure() {
        let mut config = Config::default();
        config.load_balancing_mode = "adaptive_round_robin".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.kiro_api_key = Some("token-1".to_string());
        let mut cred2 = KiroCredentials::default();
        cred2.kiro_api_key = Some("token-2".to_string());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        assert!(manager.report_immediate_failure(1));

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|entry| entry.id == 1).unwrap();
        assert!(!first.disabled);

        let metrics = manager.runtime_metrics();
        assert_eq!(metrics.cooling_down, 1);
        assert_eq!(metrics.disabled, 0);

        let ctx = manager.acquire_context(None).await.unwrap();
        assert_eq!(ctx.id, 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_caps_refresh_attempts_at_30() {
        let config = Config::default();
        let credentials = (0..40).map(|_| KiroCredentials::default()).collect();
        let manager = MultiTokenManager::new(config, credentials, None, None, false).unwrap();

        let result = manager.acquire_context(None).await;

        assert!(result.is_err());
        let snapshot = manager.snapshot();
        let refresh_failures: u32 = snapshot
            .entries
            .iter()
            .map(|entry| entry.refresh_failure_count)
            .sum();
        assert_eq!(refresh_failures, 30);
    }

    #[test]
    fn test_multi_token_manager_report_refresh_failure() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        assert_eq!(manager.available_count(), 2);
        for _ in 0..(MAX_FAILURES_PER_CREDENTIAL - 1) {
            assert!(manager.report_refresh_failure(1));
        }
        assert_eq!(manager.available_count(), 2);

        assert!(manager.report_refresh_failure(1));
        assert_eq!(manager.available_count(), 1);

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|e| e.id == 1).unwrap();
        assert!(first.disabled);
        assert_eq!(first.refresh_failure_count, MAX_FAILURES_PER_CREDENTIAL);
        assert_eq!(snapshot.current_id, 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_refresh_failure_disabled_is_not_auto_recovered() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_refresh_failure(1);
            manager.report_refresh_failure(2);
        }
        assert_eq!(manager.available_count(), 0);

        let err = manager
            .acquire_context(None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("所有凭据均已禁用"),
            "错误应提示所有凭据禁用，实际: {}",
            err
        );
    }

    #[test]
    fn test_multi_token_manager_report_quota_exhausted() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        assert_eq!(manager.available_count(), 2);
        assert!(manager.report_quota_exhausted(1));
        assert_eq!(manager.available_count(), 1);

        // 再禁用第二个后，无可用凭据
        assert!(!manager.report_quota_exhausted(2));
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_report_immediate_failure_disables_immediately() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        assert_eq!(manager.available_count(), 2);
        assert!(manager.report_immediate_failure(1));
        assert_eq!(manager.available_count(), 1);

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|e| e.id == 1).unwrap();
        assert!(first.disabled);
        assert_eq!(first.disabled_reason.as_deref(), Some("ImmediateFailure"));
        assert_eq!(snapshot.current_id, 2);
    }

    async fn assert_retryable_status_failure_switches_without_disabling(mode: &str) {
        let mut config = Config::default();
        config.load_balancing_mode = mode.to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.priority = 0;
        cred1.kiro_api_key = Some("token-1".to_string());

        let mut cred2 = KiroCredentials::default();
        cred2.priority = 1;
        cred2.kiro_api_key = Some("token-2".to_string());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        assert!(manager.report_retryable_status_failure(1));

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|e| e.id == 1).unwrap();
        assert!(!first.disabled, "mode={}", mode);

        let now_ms = now_epoch_ms();
        let cooldown_until_ms = manager
            .entries
            .lock()
            .iter()
            .find(|e| e.id == 1)
            .unwrap()
            .runtime
            .cooldown_until_ms
            .load(Ordering::Relaxed);
        assert!(
            cooldown_until_ms >= now_ms + StdDuration::from_secs(5 * 60).as_millis() as u64,
            "mode={} cooldown_until_ms={} now_ms={}",
            mode,
            cooldown_until_ms,
            now_ms
        );

        let ctx = manager.acquire_context(None).await.unwrap();
        assert_eq!(ctx.id, 2, "mode={}", mode);
    }

    #[tokio::test]
    async fn test_multi_token_manager_retryable_status_failure_switches_in_priority_mode() {
        assert_retryable_status_failure_switches_without_disabling("priority").await;
    }

    #[tokio::test]
    async fn test_multi_token_manager_retryable_status_failure_switches_in_balanced_mode() {
        assert_retryable_status_failure_switches_without_disabling("balanced").await;
    }

    #[tokio::test]
    async fn test_multi_token_manager_retryable_status_failure_switches_in_round_robin_mode() {
        assert_retryable_status_failure_switches_without_disabling("round_robin").await;
    }

    #[tokio::test]
    async fn test_multi_token_manager_retryable_status_failure_switches_in_adaptive_round_robin_mode()
     {
        assert_retryable_status_failure_switches_without_disabling("adaptive_round_robin").await;
    }

    #[tokio::test]
    async fn test_multi_token_manager_immediate_failure_disabled_is_not_auto_recovered() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        manager.report_immediate_failure(1);
        manager.report_immediate_failure(2);
        assert_eq!(manager.available_count(), 0);

        let err = manager
            .acquire_context(None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("所有凭据均已禁用"),
            "错误应提示所有凭据禁用，实际: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_multi_token_manager_quota_disabled_is_not_auto_recovered() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        manager.report_quota_exhausted(1);
        manager.report_quota_exhausted(2);
        assert_eq!(manager.available_count(), 0);

        let err = manager
            .acquire_context(None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("所有凭据均已禁用"),
            "错误应提示所有凭据禁用，实际: {}",
            err
        );
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_reset_and_enable_all_recovers_recoverable_credentials() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.priority = 10;
        let mut cred2 = KiroCredentials::default();
        cred2.priority = 1;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(1);
            manager.report_refresh_failure(2);
        }
        assert_eq!(manager.available_count(), 0);

        let result = manager.reset_and_enable_all().unwrap();
        assert_eq!(result.reset_count, 2);
        assert_eq!(result.skipped_invalid_config_count, 0);
        assert_eq!(result.unchanged_count, 0);

        let snapshot = manager.snapshot();
        assert_eq!(snapshot.available, 2);
        assert_eq!(snapshot.current_id, 2);
        assert!(snapshot.entries.iter().all(|entry| !entry.disabled));
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| entry.failure_count == 0)
        );
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| entry.refresh_failure_count == 0)
        );
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| entry.disabled_reason.is_none())
        );
    }

    #[test]
    fn test_multi_token_manager_reset_and_enable_all_skips_invalid_config_credentials() {
        let config = Config::default();

        let valid = KiroCredentials::default();

        let mut invalid = KiroCredentials::default();
        invalid.auth_method = Some("api_key".to_string());
        invalid.disabled = false;
        invalid.kiro_api_key = None;

        let manager =
            MultiTokenManager::new(config, vec![valid, invalid], None, None, false).unwrap();

        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(1);
        }
        assert_eq!(manager.available_count(), 0);

        let result = manager.reset_and_enable_all().unwrap();
        assert_eq!(result.reset_count, 1);
        assert_eq!(result.skipped_invalid_config_count, 1);

        let snapshot = manager.snapshot();
        let valid_entry = snapshot.entries.iter().find(|entry| entry.id == 1).unwrap();
        let invalid_entry = snapshot.entries.iter().find(|entry| entry.id == 2).unwrap();

        assert!(!valid_entry.disabled);
        assert_eq!(valid_entry.failure_count, 0);
        assert!(valid_entry.disabled_reason.is_none());

        assert!(invalid_entry.disabled);
        assert_eq!(
            invalid_entry.disabled_reason.as_deref(),
            Some("InvalidConfig")
        );
        assert_eq!(snapshot.available, 1);
    }

    #[test]
    fn test_multi_token_manager_clear_immediate_failure_disabled_only_removes_immediate_failure() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();
        let cred3 = KiroCredentials::default();
        let cred4 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2, cred3, cred4], None, None, false)
                .unwrap();

        assert!(manager.report_immediate_failure(1));
        assert!(manager.report_quota_exhausted(2));
        manager.set_disabled(3, true).unwrap();

        let result = manager.clear_immediate_failure_disabled().unwrap();
        assert_eq!(result.cleared_count, 1);
        assert_eq!(result.skipped_other_disabled_count, 2);
        assert_eq!(result.unchanged_count, 1);

        let snapshot = manager.snapshot();
        assert_eq!(snapshot.total, 3);
        assert!(snapshot.entries.iter().all(|entry| entry.id != 1));

        let quota_entry = snapshot.entries.iter().find(|entry| entry.id == 2).unwrap();
        assert!(quota_entry.disabled);
        assert_eq!(
            quota_entry.disabled_reason.as_deref(),
            Some("QuotaExceeded")
        );

        let manual_entry = snapshot.entries.iter().find(|entry| entry.id == 3).unwrap();
        assert!(manual_entry.disabled);
        assert_eq!(manual_entry.disabled_reason.as_deref(), Some("Manual"));

        let untouched_entry = snapshot.entries.iter().find(|entry| entry.id == 4).unwrap();
        assert!(!untouched_entry.disabled);
    }

    // ============ 凭据级 Region 优先级测试 ============

    #[test]
    fn test_credential_region_priority_uses_credential_auth_region() {
        // 凭据配置了 auth_region 时，应使用凭据的 auth_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("eu-west-1".to_string());

        let region = credentials.effective_auth_region(&config);
        assert_eq!(region, "eu-west-1");
    }

    #[test]
    fn test_credential_region_priority_fallback_to_credential_region() {
        // 凭据未配置 auth_region 但配置了 region 时，应回退到凭据.region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("eu-central-1".to_string());

        let region = credentials.effective_auth_region(&config);
        assert_eq!(region, "eu-central-1");
    }

    #[test]
    fn test_credential_region_priority_fallback_to_config() {
        // 凭据未配置 auth_region 和 region 时，应回退到 config
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let credentials = KiroCredentials::default();
        assert!(credentials.auth_region.is_none());
        assert!(credentials.region.is_none());

        let region = credentials.effective_auth_region(&config);
        assert_eq!(region, "us-west-2");
    }

    #[test]
    fn test_multiple_credentials_use_respective_regions() {
        // 多凭据场景下，不同凭据使用各自的 auth_region
        let mut config = Config::default();
        config.region = "ap-northeast-1".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.auth_region = Some("us-east-1".to_string());

        let mut cred2 = KiroCredentials::default();
        cred2.region = Some("eu-west-1".to_string());

        let cred3 = KiroCredentials::default(); // 无 region，使用 config

        assert_eq!(cred1.effective_auth_region(&config), "us-east-1");
        assert_eq!(cred2.effective_auth_region(&config), "eu-west-1");
        assert_eq!(cred3.effective_auth_region(&config), "ap-northeast-1");
    }

    #[test]
    fn test_idc_oidc_endpoint_uses_credential_auth_region() {
        // 验证 IdC OIDC endpoint URL 使用凭据 auth_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("eu-central-1".to_string());

        let region = credentials.effective_auth_region(&config);
        let refresh_url = format!("https://oidc.{}.amazonaws.com/token", region);

        assert_eq!(refresh_url, "https://oidc.eu-central-1.amazonaws.com/token");
    }

    #[test]
    fn test_social_refresh_endpoint_uses_credential_auth_region() {
        // 验证 Social refresh endpoint URL 使用凭据 auth_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("ap-southeast-1".to_string());

        let region = credentials.effective_auth_region(&config);
        let refresh_url = format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken", region);

        assert_eq!(
            refresh_url,
            "https://prod.ap-southeast-1.auth.desktop.kiro.dev/refreshToken"
        );
    }

    #[test]
    fn test_api_call_uses_effective_api_region() {
        // 验证 API 调用使用 effective_api_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("eu-west-1".to_string());

        // 凭据.region 不参与 api_region 回退链
        let api_region = credentials.effective_api_region(&config);
        let api_host = format!("q.{}.amazonaws.com", api_region);

        assert_eq!(api_host, "q.us-west-2.amazonaws.com");
    }

    #[test]
    fn test_api_call_uses_credential_api_region() {
        // 凭据配置了 api_region 时，API 调用应使用凭据的 api_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.api_region = Some("eu-central-1".to_string());

        let api_region = credentials.effective_api_region(&config);
        let api_host = format!("q.{}.amazonaws.com", api_region);

        assert_eq!(api_host, "q.eu-central-1.amazonaws.com");
    }

    #[test]
    fn test_credential_region_empty_string_treated_as_set() {
        // 空字符串 auth_region 被视为已设置（虽然不推荐，但行为应一致）
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("".to_string());

        let region = credentials.effective_auth_region(&config);
        // 空字符串被视为已设置，不会回退到 config
        assert_eq!(region, "");
    }

    #[test]
    fn test_auth_and_api_region_independent() {
        // auth_region 和 api_region 互不影响
        let mut config = Config::default();
        config.region = "default".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("auth-only".to_string());
        credentials.api_region = Some("api-only".to_string());

        assert_eq!(credentials.effective_auth_region(&config), "auth-only");
        assert_eq!(credentials.effective_api_region(&config), "api-only");
    }
}
