//! Token 管理模块
//!
//! 负责 Token 过期检测和刷新，支持 Social 和 IdC 认证方式
//! 支持单凭据 (TokenManager) 和多凭据 (MultiTokenManager) 管理

use anyhow::bail;
use base64::{Engine as _, engine::general_purpose};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as TokioMutex;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::machine_id;
use crate::kiro::model::available_profiles::ListAvailableProfilesResponse;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::token_refresh::{
    IdcRefreshRequest, IdcRefreshResponse, RefreshRequest, RefreshResponse,
};
use crate::kiro::model::usage_limits::UsageLimitsResponse;
use crate::kiro::web_portal;
use crate::model::config::Config;

/// Token 管理器
///
/// 负责管理凭据和 Token 的自动刷新
pub struct TokenManager {
    config: Config,
    credentials: KiroCredentials,
    proxy: Option<ProxyConfig>,
}

impl TokenManager {
    /// 创建新的 TokenManager 实例
    pub fn new(config: Config, credentials: KiroCredentials, proxy: Option<ProxyConfig>) -> Self {
        Self {
            config,
            credentials,
            proxy,
        }
    }

    /// 获取凭据的引用
    pub fn credentials(&self) -> &KiroCredentials {
        &self.credentials
    }

    /// 获取配置的引用
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 确保获取有效的访问 Token
    ///
    /// 如果 Token 过期或即将过期，会自动刷新
    pub async fn ensure_valid_token(&mut self) -> anyhow::Result<String> {
        if is_token_expired(&self.credentials) || is_token_expiring_soon(&self.credentials) {
            self.credentials =
                refresh_token(&self.credentials, &self.config, self.proxy.as_ref()).await?;

            // 刷新后再次检查 token 时间有效性
            if is_token_expired(&self.credentials) {
                anyhow::bail!("刷新后的 Token 仍然无效或已过期");
            }
        }

        self.credentials
            .access_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("没有可用的 accessToken"))
    }

    /// 获取使用额度信息
    ///
    /// 调用 getUsageLimits API 查询当前账户的使用额度
    pub async fn get_usage_limits(&mut self) -> anyhow::Result<UsageLimitsResponse> {
        let token = self.ensure_valid_token().await?;
        get_usage_limits(&self.credentials, &self.config, &token, self.proxy.as_ref()).await
    }
}

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

fn looks_like_email(s: &str) -> bool {
    // 极简但实用：避免误判过多，也不做 RFC 复杂校验
    if s.is_empty() || s.len() > 254 {
        return false;
    }
    if s.contains(' ') {
        return false;
    }

    let at = match s.find('@') {
        Some(v) => v,
        None => return false,
    };

    // 至少要有 local@domain.tld
    if at == 0 || at + 3 >= s.len() {
        return false;
    }

    let domain = &s[at + 1..];
    domain.contains('.')
}

/// 尽力从凭据中提取账户邮箱，仅用于管理面板展示。
///
/// 优先级：
/// 1. 已保存的 account_email（从 API 获取并持久化）
/// 2. 从 access_token JWT payload 中解析
///
/// 注意：
/// - 不验证 JWT 签名（展示用途足够）。
/// - 解析失败返回 None。
fn extract_account_email(credentials: &KiroCredentials) -> Option<String> {
    // 优先使用已保存的邮箱
    if let Some(email) = credentials.account_email.as_ref() {
        if !email.is_empty() {
            return Some(email.clone());
        }
    }

    let access_token = match credentials.access_token.as_deref() {
        Some(t) => t,
        None => return None,
    };

    // JWT: header.payload.signature
    let mut parts = access_token.split('.');
    let _header = parts.next();
    let payload_b64 = match parts.next() {
        Some(v) => v,
        None => return None,
    };
    let _sig = parts.next();

    // 如果还有更多段，说明不是标准 JWT
    if parts.next().is_some() {
        return None;
    }

    let payload_bytes = match general_purpose::URL_SAFE_NO_PAD.decode(payload_b64) {
        Ok(v) => v,
        Err(_) => return None,
    };

    let payload_json: serde_json::Value = match serde_json::from_slice(&payload_bytes) {
        Ok(v) => v,
        Err(_) => return None,
    };

    let obj = match payload_json.as_object() {
        Some(o) => o,
        None => return None,
    };

    // 常见 claim 名称（按优先级）
    const CANDIDATES: &[&str] = &[
        "email",
        "email_address",
        "preferred_username",
        "upn",
        "username",
        "user_name",
    ];

    for k in CANDIDATES {
        let v = match obj.get(*k) {
            Some(v) => v,
            None => continue,
        };
        let s = match v.as_str() {
            Some(s) => s.trim(),
            None => continue,
        };
        if looks_like_email(s) {
            return Some(s.to_string());
        }
    }

    None
}

/// 刷新 Token
pub(crate) async fn refresh_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    validate_refresh_token(credentials)?;

    // 根据 auth_method 选择刷新方式
    let auth_method = credentials.auth_method.as_deref().unwrap_or("social");

    match auth_method.to_lowercase().as_str() {
        "idc" | "builder-id" => {
            let mut new_credentials = refresh_idc_token(credentials, config, proxy).await?;

            // 仅当 profile_arn 为空时才调用 ListAvailableProfiles（降级：失败/空列表不阻断刷新）
            if new_credentials.profile_arn.is_none() {
                let token = new_credentials
                    .access_token
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("刷新 IdC Token 成功但缺少 accessToken"))?;

                match list_available_profiles(&new_credentials, config, token, proxy).await {
                    Ok(Some(profile_arn)) => {
                        tracing::info!("成功获取 IdC profileArn");
                        new_credentials.profile_arn = Some(profile_arn);
                    }
                    Ok(None) => {
                        // 可能是退避跳过、已达尝试次数、或真正的空列表，降级为 debug
                        tracing::debug!(
                            "未获取到 profileArn（可能为空列表/退避跳过），跳过写入 profileArn"
                        );
                    }
                    Err(e) => {
                        tracing::warn!("获取 profileArn 失败（不影响 IdC Token 刷新结果）: {}", e);
                    }
                }
            }

            Ok(new_credentials)
        }
        _ => refresh_social_token(credentials, config, proxy).await,
    }
}

/// 刷新 Social Token
async fn refresh_social_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    tracing::info!("正在刷新 Social Token...");

    let refresh_token = credentials
        .refresh_token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("缺少 refreshToken"))?;
    let region = &config.region;

    let refresh_url = format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken", region);
    let refresh_domain = format!("prod.{}.auth.desktop.kiro.dev", region);
    let machine_id = machine_id::generate_from_credentials(credentials, config)
        .ok_or_else(|| anyhow::anyhow!("无法生成 machineId"))?;
    let kiro_version = &config.kiro_version;

    let client = build_client(proxy, 60)?;
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

    // Social Token 默认有效期为 1 小时（3600 秒）
    // 如果响应中没有 expires_in，使用默认值确保 expires_at 总是被正确设置
    // 否则会导致 is_token_expired 始终返回 true，造成疯狂刷新
    const SOCIAL_DEFAULT_EXPIRES_IN_SECONDS: i64 = 3600;
    let expires_in = data.expires_in.unwrap_or(SOCIAL_DEFAULT_EXPIRES_IN_SECONDS);
    let expires_at = Utc::now() + Duration::seconds(expires_in);
    new_credentials.expires_at = Some(expires_at.to_rfc3339());

    tracing::debug!(
        "Social Token 刷新成功，有效期 {} 秒，过期时间: {}",
        expires_in,
        new_credentials.expires_at.as_deref().unwrap_or("unknown")
    );

    Ok(new_credentials)
}

/// IdC Token 刷新所需的 x-amz-user-agent header
const IDC_AMZ_USER_AGENT: &str = "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";

/// 刷新 IdC Token (AWS SSO OIDC)
async fn refresh_idc_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    tracing::info!("正在刷新 IdC Token...");

    let refresh_token = credentials
        .refresh_token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("缺少 refreshToken"))?;
    let client_id = credentials
        .client_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("IdC 刷新需要 clientId"))?;
    let client_secret = credentials
        .client_secret
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("IdC 刷新需要 clientSecret"))?;

    let region = &config.region;
    let refresh_url = format!("https://oidc.{}.amazonaws.com/token", region);

    let client = build_client(proxy, 60)?;
    let body = IdcRefreshRequest {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
        refresh_token: refresh_token.to_string(),
        grant_type: "refresh_token".to_string(),
    };

    let response = client
        .post(&refresh_url)
        .header("Content-Type", "application/json")
        .header("Host", format!("oidc.{}.amazonaws.com", region))
        .header("Connection", "keep-alive")
        .header("x-amz-user-agent", IDC_AMZ_USER_AGENT)
        .header("Accept", "*/*")
        .header("Accept-Language", "*")
        .header("sec-fetch-mode", "cors")
        .header("User-Agent", "node")
        .header("Accept-Encoding", "br, gzip, deflate")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
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

    // IdC Token 默认有效期为 1 小时（3600 秒）
    // 如果响应中没有 expires_in，使用默认值确保 expires_at 总是被正确设置
    const IDC_DEFAULT_EXPIRES_IN_SECONDS: i64 = 3600;
    let expires_in = data.expires_in.unwrap_or(IDC_DEFAULT_EXPIRES_IN_SECONDS);
    let expires_at = Utc::now() + Duration::seconds(expires_in);
    new_credentials.expires_at = Some(expires_at.to_rfc3339());

    tracing::debug!(
        "IdC Token 刷新成功，有效期 {} 秒，过期时间: {}",
        expires_in,
        new_credentials.expires_at.as_deref().unwrap_or("unknown")
    );

    Ok(new_credentials)
}

/// ListAvailableProfiles API 所需的 x-amz-user-agent header 前缀
const LIST_PROFILES_AMZ_USER_AGENT_PREFIX: &str = "aws-sdk-js/1.0.0";

/// 最大分页请求次数（防止无限循环）
const LIST_PROFILES_MAX_PAGES: usize = 5;

/// 错误响应 body 最大长度（用于日志截断）
const ERROR_BODY_MAX_LEN: usize = 2048;

/// 每个进程内对同一凭据最多尝试次数（避免在 profileArn 缺失/不可用时持续触发 API）
const LIST_PROFILES_MAX_LOOKUP_ATTEMPTS: u8 = 2;

/// 发生 429 限流后的退避时间（分钟）
const LIST_PROFILES_RATE_LIMIT_BACKOFF_MINUTES: i64 = 30;

/// 返回空 profiles/无 arn 时的退避时间（小时）
const LIST_PROFILES_EMPTY_RESULT_BACKOFF_HOURS: i64 = 24;

/// ListAvailableProfiles 缓存条目
#[derive(Debug, Clone, Default)]
struct ListProfilesCacheEntry {
    /// 已缓存的 profile_arn（成功获取后缓存）
    arn: Option<String>,
    /// 尝试次数
    attempts: u8,
    /// 上次使用的 accessToken 哈希（避免同一 token 重复尝试）
    last_access_token_hash: Option<String>,
    /// 退避截止时间（在此时间之前不再尝试）
    next_allowed_at: Option<DateTime<Utc>>,
}

/// 全局 ListAvailableProfiles 缓存
static LIST_PROFILES_CACHE: OnceLock<Mutex<HashMap<String, ListProfilesCacheEntry>>> =
    OnceLock::new();

/// 获取 ListAvailableProfiles 缓存
fn list_profiles_cache() -> &'static Mutex<HashMap<String, ListProfilesCacheEntry>> {
    LIST_PROFILES_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 计算字符串的 SHA256 哈希（十六进制）
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// 生成 ListAvailableProfiles 缓存键
fn list_profiles_cache_key(credentials: &KiroCredentials, config: &Config) -> Option<String> {
    let refresh_token = credentials.refresh_token.as_deref()?;
    if refresh_token.trim().is_empty() {
        return None;
    }
    Some(format!(
        "{}:{}",
        config.region,
        sha256_hex(&format!("list_available_profiles/{}", refresh_token))
    ))
}

/// ListAvailableProfiles 请求体
///
/// 当 `next_token` 为 None 时序列化为 `{}`（而不是空 body）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListAvailableProfilesRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    next_token: Option<&'a str>,
}

/// 调用 ListAvailableProfiles API 获取可用的 profileArn（用于 IdC/builder-id 凭证）
///
/// - 支持分页：循环请求直到找到第一个非空 arn 或 nextToken 为空
/// - profiles 为空/无 arn 时返回 Ok(None)（降级，不阻断调用方流程）
/// - 内置进程内缓存和退避机制，避免频繁调用 API
pub(crate) async fn list_available_profiles(
    credentials: &KiroCredentials,
    config: &Config,
    access_token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<Option<String>> {
    use anyhow::Context;

    tracing::debug!("正在获取可用 Profiles...");

    // ========================================================================
    // 进程内缓存 / 退避：避免 profileArn 缺失时每次刷新都触发 ListAvailableProfiles
    // 合并"检查缓存 + 预占本次尝试"到同一个锁，消除并发竞态
    // ========================================================================
    let cache_key = list_profiles_cache_key(credentials, config);
    let access_token_hash = sha256_hex(access_token);

    if let Some(ref key) = cache_key {
        let now = Utc::now();
        let mut cache = list_profiles_cache().lock();
        let entry = cache.entry(key.clone()).or_default();

        // 已缓存成功结果
        if let Some(ref arn) = entry.arn {
            tracing::debug!("profileArn 命中进程内缓存，跳过 ListAvailableProfiles 请求");
            return Ok(Some(arn.clone()));
        }
        // 已达到最大尝试次数
        if entry.attempts >= LIST_PROFILES_MAX_LOOKUP_ATTEMPTS {
            tracing::debug!(
                "profileArn 已尝试 {} 次，跳过 ListAvailableProfiles 请求",
                entry.attempts
            );
            return Ok(None);
        }
        // 处于退避期
        if let Some(next_allowed_at) = entry.next_allowed_at {
            if next_allowed_at > now {
                tracing::debug!(
                    "profileArn 获取处于退避期（直到 {}），跳过 ListAvailableProfiles 请求",
                    next_allowed_at
                );
                return Ok(None);
            }
        }
        // 已用当前 accessToken 尝试过
        if entry.last_access_token_hash.as_deref() == Some(access_token_hash.as_str()) {
            tracing::debug!(
                "profileArn 已用当前 accessToken 尝试过，跳过 ListAvailableProfiles 请求"
            );
            return Ok(None);
        }

        // 预占本次尝试（在同一个锁内完成，避免并发竞态）
        entry.attempts = entry.attempts.saturating_add(1);
        entry.last_access_token_hash = Some(access_token_hash.clone());
        entry.next_allowed_at = None;
    }

    let region = &config.region;
    let host = format!("q.{}.amazonaws.com", region);
    let url = format!("https://{}/ListAvailableProfiles", host);

    let machine_id = machine_id::generate_from_credentials(credentials, config)
        .ok_or_else(|| anyhow::anyhow!("无法生成 machineId"))?;
    let kiro_version = &config.kiro_version;
    let os_name = &config.system_version;
    let node_version = &config.node_version;

    // 构建 User-Agent headers（按抓包格式）
    let user_agent = format!(
        "aws-sdk-js/1.0.0 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{}-{}",
        os_name, node_version, kiro_version, machine_id
    );
    let amz_user_agent = format!(
        "{} KiroIDE-{}-{}",
        LIST_PROFILES_AMZ_USER_AGENT_PREFIX, kiro_version, machine_id
    );

    let client = build_client(proxy, 60)?;

    let mut next_token: Option<String> = None;
    let mut total_profiles = 0usize;

    // 分页循环
    for page in 0..LIST_PROFILES_MAX_PAGES {
        // 构建请求 body（next_token 为 None 时发送 `{}`）
        let body = ListAvailableProfilesRequest {
            next_token: next_token.as_deref(),
        };

        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-amz-user-agent", &amz_user_agent)
            .header("User-Agent", &user_agent)
            .header("host", &host)
            .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=1")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Connection", "close")
            .json(&body)
            .send()
            .await
            .context("发送 ListAvailableProfiles 请求失败")?;

        let status = response.status();
        if !status.is_success() {
            // 429/5xx 做退避，避免下一次刷新立即重复请求
            if let Some(ref key) = cache_key {
                let backoff_until = match status.as_u16() {
                    429 => Some(
                        Utc::now() + Duration::minutes(LIST_PROFILES_RATE_LIMIT_BACKOFF_MINUTES),
                    ),
                    500..=599 => Some(Utc::now() + Duration::minutes(5)),
                    _ => None,
                };
                if let Some(backoff_until) = backoff_until {
                    let mut cache = list_profiles_cache().lock();
                    let entry = cache.entry(key.clone()).or_default();
                    entry.next_allowed_at = Some(backoff_until);
                }
            }

            let body_text = match response.text().await {
                Ok(t) => truncate_string(&t, ERROR_BODY_MAX_LEN),
                Err(e) => format!("<读取响应 body 失败: {}>", e),
            };
            let error_msg = match status.as_u16() {
                401 => "认证失败，Token 无效或已过期",
                403 => "权限不足，无法获取可用 Profiles",
                429 => "请求过于频繁，已被限流",
                500..=599 => "服务器错误，AWS 服务暂时不可用",
                _ => "获取可用 Profiles 失败",
            };
            bail!("{}: {} {}", error_msg, status, body_text);
        }

        let data: ListAvailableProfilesResponse = response
            .json()
            .await
            .context("解析 ListAvailableProfiles 响应失败")?;

        let profiles = data.profiles.unwrap_or_default();
        total_profiles += profiles.len();

        // 查找第一个非空 arn
        let arn = profiles
            .into_iter()
            .filter_map(|p| p.arn)
            .map(|s| s.trim().to_string())
            .find(|arn| !arn.is_empty());

        if let Some(selected_arn) = arn {
            // 缓存成功结果
            if let Some(ref key) = cache_key {
                let mut cache = list_profiles_cache().lock();
                let entry = cache.entry(key.clone()).or_default();
                entry.arn = Some(selected_arn.clone());
                entry.next_allowed_at = None;
            }

            tracing::debug!(
                "从 {} 个 profiles（第 {} 页）中选择了 arn: {}...{}",
                total_profiles,
                page + 1,
                &selected_arn[..20.min(selected_arn.len())],
                &selected_arn[selected_arn.len().saturating_sub(10)..]
            );
            return Ok(Some(selected_arn));
        }

        // 检查是否有下一页
        next_token = data.next_token.filter(|t| !t.trim().is_empty());
        if next_token.is_none() {
            break;
        }

        tracing::debug!("第 {} 页无可用 arn，继续翻页...", page + 1);
    }

    tracing::debug!("共检查 {} 个 profiles，无可用 arn", total_profiles);

    // 空结果做退避，避免每次刷新都再次探测
    if let Some(ref key) = cache_key {
        let mut cache = list_profiles_cache().lock();
        let entry = cache.entry(key.clone()).or_default();
        entry.next_allowed_at =
            Some(Utc::now() + Duration::hours(LIST_PROFILES_EMPTY_RESULT_BACKOFF_HOURS));
    }

    Ok(None)
}

/// 截断字符串到指定长度
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...(truncated)", &s[..max_len])
    }
}

/// getUsageLimits API 所需的 x-amz-user-agent header 前缀
const USAGE_LIMITS_AMZ_USER_AGENT_PREFIX: &str = "aws-sdk-js/1.0.0";

/// 获取使用额度信息
pub(crate) async fn get_usage_limits(
    credentials: &KiroCredentials,
    config: &Config,
    token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<UsageLimitsResponse> {
    tracing::debug!("正在获取使用额度信息...");

    let region = &config.region;
    let host = format!("q.{}.amazonaws.com", region);
    let machine_id = machine_id::generate_from_credentials(credentials, config)
        .ok_or_else(|| anyhow::anyhow!("无法生成 machineId"))?;
    let kiro_version = &config.kiro_version;

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
        "aws-sdk-js/1.0.0 ua/2.1 os/darwin#24.6.0 lang/js md/nodejs#22.21.1 \
         api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{}-{}",
        kiro_version, machine_id
    );
    let amz_user_agent = format!(
        "{} KiroIDE-{}-{}",
        USAGE_LIMITS_AMZ_USER_AGENT_PREFIX, kiro_version, machine_id
    );

    let client = build_client(proxy, 60)?;

    let response = client
        .get(&url)
        .header("x-amz-user-agent", &amz_user_agent)
        .header("User-Agent", &user_agent)
        .header("host", &host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=1")
        .header("Authorization", format!("Bearer {}", token))
        .header("Connection", "close")
        .send()
        .await?;

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
struct CredentialEntry {
    /// 凭据唯一 ID
    id: u64,
    /// 凭据信息
    credentials: KiroCredentials,
    /// API 调用连续失败次数
    failure_count: u32,
    /// 是否已禁用
    disabled: bool,
    /// 禁用原因（用于区分手动禁用 vs 自动禁用，便于自愈）
    disabled_reason: Option<DisabledReason>,
}

/// 禁用原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisabledReason {
    /// Admin API 手动禁用
    Manual,
    /// 连续失败达到阈值后自动禁用
    TooManyFailures,
    /// 额度已用尽（如 MONTHLY_REQUEST_COUNT）
    QuotaExceeded,
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
    /// 账户邮箱（尽力从 token 中解析，仅用于展示）
    pub account_email: Option<String>,
    /// 用户 ID（从 API 获取，持久化保存）
    pub user_id: Option<String>,
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
    /// Token 刷新锁，确保同一时间只有一个刷新操作
    refresh_lock: TokioMutex<()>,
    /// 凭据文件路径（用于回写）
    credentials_path: Option<PathBuf>,
    /// 是否为多凭据格式（数组格式才回写）
    is_multiple_format: bool,
}

/// 每个凭据最大 API 调用失败次数
const MAX_FAILURES_PER_CREDENTIAL: u32 = 3;

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
}

impl MultiTokenManager {
    async fn get_access_token_for(&self, id: u64) -> anyhow::Result<(KiroCredentials, String)> {
        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        // 检查是否需要刷新 token
        let needs_refresh = is_token_expired(&credentials) || is_token_expiring_soon(&credentials);

        let token = if needs_refresh {
            let _guard = self.refresh_lock.lock().await;
            let current_creds = {
                let entries = self.entries.lock();
                entries
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.credentials.clone())
                    .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
            };

            if is_token_expired(&current_creds) || is_token_expiring_soon(&current_creds) {
                let new_creds =
                    refresh_token(&current_creds, &self.config, self.proxy.as_ref()).await?;
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
        };

        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        Ok((credentials, token))
    }

    fn candidate_idps(credentials: &KiroCredentials) -> &'static [&'static str] {
        // 优先使用保存的 provider（Github/Google/BuilderId）
        if let Some(provider) = credentials.provider.as_deref() {
            match provider.to_lowercase().as_str() {
                "github" => return &["Github", "Google", "BuilderId"],
                "google" => return &["Google", "Github", "BuilderId"],
                "builderid" | "builder-id" => return &["BuilderId", "Github", "Google"],
                _ => {}
            }
        }
        
        // 兼容性：按 auth_method 做一个"尽力猜测 + 多候选重试"
        let auth = credentials
            .auth_method
            .as_deref()
            .unwrap_or("social")
            .to_ascii_lowercase();

        match auth.as_str() {
            // social 更可能是 Github/Google
            "social" => &["Github", "Google", "BuilderId"],
            // builder-id / idc 默认 BuilderId
            "builder-id" | "idc" => &["BuilderId", "Github", "Google"],
            _ => &["BuilderId", "Github", "Google"],
        }
    }
    /// 获取指定凭据的“账号信息 + 套餐 + 用量明细”（通过 Kiro Web Portal API）
    ///
    /// 成功后会自动更新凭据的 account_email、user_id、provider 字段并持久化
    pub async fn get_account_info_for(
        &self,
        id: u64,
    ) -> anyhow::Result<web_portal::AccountAggregateInfo> {
        let (credentials, token) = self.get_access_token_for(id).await?;

        let proxy = self.proxy.as_ref();

        let mut last_err: Option<anyhow::Error> = None;
        for idp in Self::candidate_idps(&credentials) {
            // 并行请求：GetUserInfo & GetUserUsageAndLimits
            let (user_info, usage) = tokio::join!(
                web_portal::get_user_info(&token, idp, proxy),
                web_portal::get_user_usage_and_limits(&token, idp, proxy)
            );

            match usage {
                Ok(u) => {
                    // 尝试从 usage.user_info 获取邮箱和用户 ID，并更新凭据
                    let mut should_persist = false;
                    {
                        let mut entries = self.entries.lock();
                        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                            // 更新 account_email
                            if entry.credentials.account_email.is_none() {
                                if let Some(ref ui) = u.user_info {
                                    if let Some(ref email) = ui.email {
                                        entry.credentials.account_email = Some(email.clone());
                                        should_persist = true;
                                    }
                                }
                            }
                            // 更新 user_id
                            if entry.credentials.user_id.is_none() {
                                if let Some(ref ui) = u.user_info {
                                    if let Some(ref uid) = ui.user_id {
                                        entry.credentials.user_id = Some(uid.clone());
                                        should_persist = true;
                                    }
                                }
                            }
                            // 更新 provider
                            if entry.credentials.provider.is_none() {
                                entry.credentials.provider = Some(idp.to_string());
                                should_persist = true;
                            }
                        }
                    }
                    // 持久化（在锁外执行以避免死锁）
                    if should_persist {
                        if let Err(e) = self.persist_credentials() {
                            tracing::warn!("自动更新账户信息后持久化失败: {}", e);
                        }
                    }
                    // GetUserInfo 失败不应阻断（参考 Kiro-account-manager 的行为）
                    return Ok(web_portal::aggregate_account_info(user_info.ok(), u));
                }
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(match last_err {
            Some(e) => e,
            None => anyhow::anyhow!("获取账号信息失败：没有可用的 Idp 候选"),
        })
    }
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

        let entries: Vec<CredentialEntry> = credentials
            .into_iter()
            .map(|mut cred| {
                let id = cred.id.unwrap_or_else(|| {
                    let id = next_id;
                    next_id += 1;
                    cred.id = Some(id);
                    has_new_ids = true;
                    id
                });
                CredentialEntry {
                    id,
                    credentials: cred,
                    failure_count: 0,
                    disabled: false,
                    disabled_reason: None,
                }
            })
            .collect();

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

        // 选择初始凭据：优先级最高（priority 最小）的凭据，无凭据时为 0
        let initial_id = entries
            .iter()
            .min_by_key(|e| e.credentials.priority)
            .map(|e| e.id)
            .unwrap_or(0);

        let manager = Self {
            config,
            proxy,
            entries: Mutex::new(entries),
            current_id: Mutex::new(initial_id),
            refresh_lock: TokioMutex::new(()),
            credentials_path,
            is_multiple_format,
        };

        // 如果有新分配的 ID，立即持久化到配置文件
        if has_new_ids {
            if let Err(e) = manager.persist_credentials() {
                tracing::warn!("新分配 ID 后持久化失败: {}", e);
            } else {
                tracing::info!("已为凭据分配新 ID 并写回配置文件");
            }
        }

        Ok(manager)
    }

    /// 获取配置的引用
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 获取当前活动凭据的克隆
    pub fn credentials(&self) -> KiroCredentials {
        let entries = self.entries.lock();
        let current_id = *self.current_id.lock();
        entries
            .iter()
            .find(|e| e.id == current_id)
            .map(|e| e.credentials.clone())
            .unwrap_or_default()
    }

    /// 获取凭据总数
    pub fn total_count(&self) -> usize {
        self.entries.lock().len()
    }

    /// 获取可用凭据数量
    pub fn available_count(&self) -> usize {
        self.entries.lock().iter().filter(|e| !e.disabled).count()
    }

    /// 获取 API 调用上下文
    ///
    /// 返回绑定了 id、credentials 和 token 的调用上下文
    /// 确保整个 API 调用过程中使用一致的凭据信息
    ///
    /// 如果 Token 过期或即将过期，会自动刷新
    /// Token 刷新失败时会尝试下一个可用凭据（不计入失败次数）
    pub async fn acquire_context(&self) -> anyhow::Result<CallContext> {
        let total = self.total_count();
        let mut tried_count = 0;

        loop {
            if tried_count >= total {
                anyhow::bail!(
                    "所有凭据均无法获取有效 Token（可用: {}/{}）",
                    self.available_count(),
                    total
                );
            }

            let (id, credentials) = {
                let mut entries = self.entries.lock();
                let current_id = *self.current_id.lock();

                // 找到当前凭据
                if let Some(entry) = entries.iter().find(|e| e.id == current_id && !e.disabled) {
                    (entry.id, entry.credentials.clone())
                } else {
                    // 当前凭据不可用，选择优先级最高的可用凭据
                    let mut best = entries
                        .iter()
                        .filter(|e| !e.disabled)
                        .min_by_key(|e| e.credentials.priority);

                    // 没有可用凭据：如果是"自动禁用导致全灭"，做一次类似重启的自愈
                    if best.is_none()
                        && entries.iter().any(|e| {
                            e.disabled && e.disabled_reason == Some(DisabledReason::TooManyFailures)
                        })
                    {
                        tracing::warn!(
                            "所有凭据均已被自动禁用，执行自愈：重置失败计数并重新启用（等价于重启）"
                        );
                        for e in entries.iter_mut() {
                            if e.disabled_reason == Some(DisabledReason::TooManyFailures) {
                                e.disabled = false;
                                e.disabled_reason = None;
                                e.failure_count = 0;
                            }
                        }
                        best = entries
                            .iter()
                            .filter(|e| !e.disabled)
                            .min_by_key(|e| e.credentials.priority);
                    }

                    if let Some(entry) = best {
                        // 先提取数据
                        let new_id = entry.id;
                        let new_creds = entry.credentials.clone();
                        drop(entries);
                        // 更新 current_id
                        let mut current_id = self.current_id.lock();
                        *current_id = new_id;
                        (new_id, new_creds)
                    } else {
                        // 注意：必须在 bail! 之前计算 available_count，
                        // 因为 available_count() 会尝试获取 entries 锁，
                        // 而此时我们已经持有该锁，会导致死锁
                        let available = entries.iter().filter(|e| !e.disabled).count();
                        anyhow::bail!("所有凭据均已禁用（{}/{}）", available, total);
                    }
                }
            };

            // 尝试获取/刷新 Token
            match self.try_ensure_token(id, &credentials).await {
                Ok(ctx) => {
                    return Ok(ctx);
                }
                Err(e) => {
                    tracing::warn!("凭据 #{} Token 刷新失败，尝试下一个凭据: {}", id, e);

                    // Token 刷新失败，切换到下一个优先级的凭据（不计入失败次数）
                    self.switch_to_next_by_priority();
                    tried_count += 1;
                }
            }
        }
    }

    /// 切换到下一个优先级最高的可用凭据（内部方法）
    fn switch_to_next_by_priority(&self) {
        let entries = self.entries.lock();
        let mut current_id = self.current_id.lock();

        // 选择优先级最高的未禁用凭据（排除当前凭据）
        if let Some(entry) = entries
            .iter()
            .filter(|e| !e.disabled && e.id != *current_id)
            .min_by_key(|e| e.credentials.priority)
        {
            *current_id = entry.id;
            tracing::info!(
                "已切换到凭据 #{}（优先级 {}）",
                entry.id,
                entry.credentials.priority
            );
        }
    }

    /// 选择优先级最高的未禁用凭据作为当前凭据（内部方法）
    ///
    /// 与 `switch_to_next_by_priority` 不同，此方法不排除当前凭据，
    /// 纯粹按优先级选择，用于优先级变更后立即生效
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
    ) -> anyhow::Result<CallContext> {
        // 第一次检查（无锁）：快速判断是否需要刷新
        let needs_refresh = is_token_expired(credentials) || is_token_expiring_soon(credentials);

        let creds = if needs_refresh {
            // 获取刷新锁，确保同一时间只有一个刷新操作
            let _guard = self.refresh_lock.lock().await;

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
                let new_creds =
                    refresh_token(&current_creds, &self.config, self.proxy.as_ref()).await?;

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

        Ok(CallContext {
            id,
            credentials: creds,
            token,
        })
    }

    /// 将凭据列表回写到源文件
    ///
    /// 回写规则：
    /// - credentials_path 未设置：跳过写入
    /// - 如果启动时读取的是多凭据格式（数组），或当前凭据数量 != 1：写回数组格式
    /// - 否则（单凭据且启动时为单对象格式）：写回单对象格式
    ///
    /// # Returns
    /// - `Ok(true)` - 成功写入文件
    /// - `Ok(false)` - 跳过写入（无路径配置）
    /// - `Err(_)` - 写入失败
    fn persist_credentials(&self) -> anyhow::Result<bool> {
        use anyhow::Context;

        let path = match &self.credentials_path {
            Some(p) => p,
            None => return Ok(false),
        };

        // 收集所有凭据
        let mut credentials: Vec<KiroCredentials> = {
            let entries = self.entries.lock();
            entries.iter().map(|e| e.credentials.clone()).collect()
        };

        // 稳定输出：按 priority、id 排序
        credentials.sort_by_key(|c| (c.priority, c.id.unwrap_or(u64::MAX)));

        // 序列化为 pretty JSON
        let json = if self.is_multiple_format || credentials.len() != 1 {
            serde_json::to_string_pretty(&credentials).context("序列化凭据失败")?
        } else {
            let only = credentials
                .get(0)
                .ok_or_else(|| anyhow::anyhow!("序列化凭据失败：凭据数量异常"))?;
            serde_json::to_string_pretty(only).context("序列化凭据失败")?
        };

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

    /// 报告指定凭据 API 调用成功
    ///
    /// 重置该凭据的失败计数
    ///
    /// # Arguments
    /// * `id` - 凭据 ID（来自 CallContext）
    pub fn report_success(&self, id: u64) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.failure_count = 0;
            tracing::debug!("凭据 #{} API 调用成功", id);
        }
    }

    /// 报告指定凭据 API 调用失败
    ///
    /// 增加失败计数，达到阈值时禁用凭据并切换到优先级最高的可用凭据
    /// 返回是否还有可用凭据可以重试
    ///
    /// # Arguments
    /// * `id` - 凭据 ID（来自 CallContext）
    pub fn report_failure(&self, id: u64) -> bool {
        let mut entries = self.entries.lock();
        let mut current_id = self.current_id.lock();

        let entry = match entries.iter_mut().find(|e| e.id == id) {
            Some(e) => e,
            None => return entries.iter().any(|e| !e.disabled),
        };

        entry.failure_count += 1;
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
                return false;
            }
        }

        // 检查是否还有可用凭据
        entries.iter().any(|e| !e.disabled)
    }

    /// 报告指定凭据额度已用尽
    ///
    /// 用于处理 402 Payment Required 且 reason 为 `MONTHLY_REQUEST_COUNT` 的场景：
    /// - 立即禁用该凭据（不等待连续失败阈值）
    /// - 切换到下一个可用凭据继续重试
    /// - 返回是否还有可用凭据
    pub fn report_quota_exhausted(&self, id: u64) -> bool {
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
        // 设为阈值，便于在管理面板中直观看到该凭据已不可用
        entry.failure_count = MAX_FAILURES_PER_CREDENTIAL;

        tracing::error!(
            "凭据 #{} 额度已用尽（MONTHLY_REQUEST_COUNT），已被禁用",
            id
        );

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
            return true;
        }

        tracing::error!("所有凭据均已禁用！");
        false
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

    /// 获取使用额度信息
    pub async fn get_usage_limits(&self) -> anyhow::Result<UsageLimitsResponse> {
        let ctx = self.acquire_context().await?;
        get_usage_limits(
            &ctx.credentials,
            &self.config,
            &ctx.token,
            self.proxy.as_ref(),
        )
        .await
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
                    auth_method: e.credentials.auth_method.clone(),
                    has_profile_arn: e.credentials.profile_arn.is_some(),
                    expires_at: e.credentials.expires_at.clone(),
                    account_email: extract_account_email(&e.credentials),
                    user_id: e.credentials.user_id.clone(),
                })
                .collect(),
            current_id,
            total: entries.len(),
            available,
        }
    }

    /// 删除指定凭据（Admin API）
    ///
    /// 删除后会：
    /// - 从内存 entries 移除
    /// - 如果当前凭据被删除或当前凭据不可用，则选择一个可用凭据作为新的 current_id（否则为 0）
    /// - 回写到凭据文件（credentials_path 必须已配置）
    pub fn remove_credential(&self, id: u64) -> anyhow::Result<()> {
        if id == 0 {
            anyhow::bail!("无效的凭据 ID: 0");
        }

        {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let idx = entries
                .iter()
                .position(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;

            entries.remove(idx);

            // 如果 current_id 指向被删除/不存在/已禁用，则重新选择一个可用凭据
            let current_exists_and_enabled = entries
                .iter()
                .any(|e| e.id == *current_id && !e.disabled);

            if !current_exists_and_enabled {
                *current_id = entries
                    .iter()
                    .filter(|e| !e.disabled)
                    .min_by_key(|e| e.credentials.priority)
                    .map(|e| e.id)
                    .unwrap_or(0);
            }
        }

        // 必须持久化（否则“删除”会在重启后回滚）
        let wrote = self.persist_credentials()?;
        if !wrote {
            anyhow::bail!("凭据文件路径未配置，无法持久化删除操作");
        }

        Ok(())
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
                entry.disabled_reason = None;
            } else {
                entry.disabled_reason = Some(DisabledReason::Manual);
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
            entry.failure_count = 0;
            entry.disabled = false;
            entry.disabled_reason = None;
        }
        // 持久化更改
        self.persist_credentials()?;
        Ok(())
    }

    /// 获取指定凭据的使用额度（Admin API）
    pub async fn get_usage_limits_for(&self, id: u64) -> anyhow::Result<UsageLimitsResponse> {
        let (credentials, token) = self.get_access_token_for(id).await?;
        get_usage_limits(&credentials, &self.config, &token, self.proxy.as_ref()).await
    }

    /// 添加新凭据（Admin API）
    ///
    /// # 流程
    /// 1. 验证凭据基本字段（refresh_token 不为空）
    /// 2. 尝试刷新 Token 验证凭据有效性
    /// 3. 分配新 ID（当前最大 ID + 1）
    /// 4. 添加到 entries 列表
    /// 5. 调用 API 获取账户信息（邮箱等）
    /// 6. 持久化到配置文件
    ///
    /// # 返回
    /// - `Ok(u64)` - 新凭据 ID
    /// - `Err(_)` - 验证失败或添加失败
    pub async fn add_credential(&self, new_cred: KiroCredentials) -> anyhow::Result<u64> {
        // 1. 基本验证
        validate_refresh_token(&new_cred)?;

        // 2. 尝试刷新 Token 验证凭据有效性
        let mut validated_cred =
            refresh_token(&new_cred, &self.config, self.proxy.as_ref()).await?;

        // 3. 分配新 ID
        let new_id = {
            let entries = self.entries.lock();
            entries.iter().map(|e| e.id).max().unwrap_or(0) + 1
        };

        // 4. 设置 ID 并保留用户输入的元数据
        validated_cred.id = Some(new_id);
        validated_cred.priority = new_cred.priority;
        validated_cred.auth_method = new_cred.auth_method.clone();
        validated_cred.client_id = new_cred.client_id;
        validated_cred.client_secret = new_cred.client_secret;
        validated_cred.provider = new_cred.provider.clone();

        // 5. 调用 API 获取账户信息（邮箱、用户 ID 等）
        if let Some(token) = validated_cred.access_token.as_ref() {
            let proxy = self.proxy.as_ref();
            // 尝试多个 idp 候选
            for idp in Self::candidate_idps(&validated_cred) {
                match web_portal::get_user_usage_and_limits(token, idp, proxy).await {
                    Ok(usage) => {
                        // 从 usage.user_info 获取邮箱和用户 ID
                        if let Some(user_info) = &usage.user_info {
                            if validated_cred.account_email.is_none() {
                                validated_cred.account_email = user_info.email.clone();
                            }
                            if validated_cred.user_id.is_none() {
                                validated_cred.user_id = user_info.user_id.clone();
                            }
                        }
                        // 记录提供商（如果从 API 能推断）
                        if validated_cred.provider.is_none() {
                            validated_cred.provider = Some(idp.to_string());
                        }
                        tracing::debug!("获取到账户信息: email={:?}, userId={:?}",
                            validated_cred.account_email, validated_cred.user_id);
                        break;
                    }
                    Err(e) => {
                        tracing::debug!("使用 idp={} 获取账户信息失败: {}", idp, e);
                        continue;
                    }
                }
            }
        }

        {
            let mut entries = self.entries.lock();
            entries.push(CredentialEntry {
                id: new_id,
                credentials: validated_cred,
                failure_count: 0,
                disabled: false,
                disabled_reason: None,
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

        tracing::info!("已删除凭据 #{}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_manager_new() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let tm = TokenManager::new(config, credentials, None);
        assert!(tm.credentials().access_token.is_none());
    }

    #[test]
    fn test_is_token_expired_with_expired_token() {
        let mut credentials = KiroCredentials::default();
        credentials.expires_at = Some("2020-01-01T00:00:00Z".to_string());
        assert!(is_token_expired(&credentials));
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

    // MultiTokenManager 测试

    #[test]
    fn test_multi_token_manager_new() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.priority = 0;
        let mut cred2 = KiroCredentials::default();
        cred2.priority = 1;

        let manager = match MultiTokenManager::new(config, vec![cred1, cred2], None, None, false) {
            Ok(v) => v,
            Err(e) => panic!("{:?}", e),
        };
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 2);
    }

    #[test]
    fn test_multi_token_manager_empty_credentials() {
        let config = Config::default();
        let result = MultiTokenManager::new(config, vec![], None, None, false);
        // 支持 0 个凭据启动（可通过管理面板添加）
        assert!(result.is_ok());
        let manager = match result {
            Ok(v) => v,
            Err(e) => panic!("{:?}", e),
        };
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
        let err_msg = match result.err() {
            Some(e) => e.to_string(),
            None => String::new(),
        };
        assert!(
            err_msg.contains("重复的凭据 ID"),
            "错误消息应包含 '重复的凭据 ID'，实际: {}",
            err_msg
        );
    }

    #[test]
    fn test_multi_token_manager_report_failure() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager = match MultiTokenManager::new(config, vec![cred1, cred2], None, None, false) {
            Ok(v) => v,
            Err(e) => panic!("{:?}", e),
        };

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

        let manager = match MultiTokenManager::new(config, vec![cred], None, None, false) {
            Ok(v) => v,
            Err(e) => panic!("{:?}", e),
        };

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

        let manager = match MultiTokenManager::new(config, vec![cred1, cred2], None, None, false) {
            Ok(v) => v,
            Err(e) => panic!("{:?}", e),
        };

        // 初始是第一个凭据
        assert_eq!(
            manager.credentials().refresh_token,
            Some("token1".to_string())
        );

        // 切换到下一个
        assert!(manager.switch_to_next());
        assert_eq!(
            manager.credentials().refresh_token,
            Some("token2".to_string())
        );
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
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).expect("创建管理器失败");

        // 凭据会自动分配 ID（从 1 开始）
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(1);
        }
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(2);
        }

        assert_eq!(manager.available_count(), 0);

        // 应触发自愈：重置失败计数并重新启用，避免必须重启进程
        let ctx = manager.acquire_context().await.expect("获取上下文失败");
        assert!(ctx.token == "t1" || ctx.token == "t2");
        assert_eq!(manager.available_count(), 2);
    }

    #[test]
    fn test_multi_token_manager_report_quota_exhausted() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).expect("创建管理器失败");

        // 凭据会自动分配 ID（从 1 开始）
        assert_eq!(manager.available_count(), 2);
        assert!(manager.report_quota_exhausted(1));
        assert_eq!(manager.available_count(), 1);

        // 再禁用第二个后，无可用凭据
        assert!(!manager.report_quota_exhausted(2));
        assert_eq!(manager.available_count(), 0);
    }

    #[tokio::test]
    async fn test_multi_token_manager_quota_disabled_is_not_auto_recovered() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).expect("创建管理器失败");

        manager.report_quota_exhausted(1);
        manager.report_quota_exhausted(2);
        assert_eq!(manager.available_count(), 0);

        let err = manager.acquire_context().await.err().expect("应返回错误").to_string();
        assert!(
            err.contains("所有凭据均已禁用"),
            "错误应提示所有凭据禁用，实际: {}",
            err
        );
        assert_eq!(manager.available_count(), 0);
    }

    /// 测试 IdC Token 刷新功能
    /// 从外部凭据文件加载 IdC 凭据并尝试刷新 Token
    #[tokio::test]
    #[ignore] // 需要有效的 IdC 凭据才能运行，使用 cargo test -- --ignored 运行
    async fn test_refresh_idc_token_from_file() {
        use std::path::Path;

        // 从外部文件加载凭据
        let credentials_path = Path::new(r"F:\working_ai\kiro2api-cc\credentials.json");
        if !credentials_path.exists() {
            println!("凭据文件不存在，跳过测试");
            return;
        }

        let content = std::fs::read_to_string(credentials_path).expect("读取凭据文件失败");
        let credentials_list: Vec<KiroCredentials> =
            serde_json::from_str(&content).expect("解析凭据文件失败");

        // 找到 IdC 凭据（id=14 或 id=15）
        let idc_credentials: Vec<_> = credentials_list
            .iter()
            .filter(|c| {
                c.auth_method
                    .as_ref()
                    .map(|m| m.to_lowercase() == "idc")
                    .unwrap_or(false)
            })
            .collect();

        if idc_credentials.is_empty() {
            println!("未找到 IdC 凭据，跳过测试");
            return;
        }

        println!("找到 {} 个 IdC 凭据", idc_credentials.len());

        let config = Config::default();

        for cred in idc_credentials {
            let id = cred.id.unwrap_or(0);
            println!("\n========================================");
            println!("测试凭据 #{}", id);
            println!("clientId: {:?}", cred.client_id.as_ref().map(|s| &s[..20.min(s.len())]));
            println!("refreshToken: {:?}", cred.refresh_token.as_ref().map(|s| &s[..20.min(s.len())]));
            println!("当前 expiresAt: {:?}", cred.expires_at);

            match refresh_idc_token(cred, &config, None).await {
                Ok(new_cred) => {
                    println!("刷新成功!");
                    println!("新 accessToken: {:?}", new_cred.access_token.as_ref().map(|s| &s[..50.min(s.len())]));
                    println!("新 expiresAt: {:?}", new_cred.expires_at);

                    // 验证新 Token 有效
                    assert!(new_cred.access_token.is_some(), "刷新后应有 accessToken");
                    assert!(new_cred.expires_at.is_some(), "刷新后应有 expiresAt");
                    assert!(!is_token_expired(&new_cred), "刷新后的 Token 不应过期");
                }
                Err(e) => {
                    println!("刷新失败: {}", e);
                    // 不 panic，继续测试其他凭据
                }
            }
        }
    }

    /// 测试 IdC 凭据发送"你好"请求
    /// 打印完整的请求信息以便调试 403 错误
    #[tokio::test]
    #[ignore] // 需要有效的 IdC 凭据才能运行，使用 cargo test -- --ignored 运行
    async fn test_idc_send_hello_request() {
        use crate::http_client::build_client;
        use crate::kiro::machine_id;
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, UserInputMessage,
        };
        use crate::kiro::model::requests::kiro::KiroRequest;
        use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, HeaderValue};
        use std::path::Path;

        // 从外部文件加载凭据
        let credentials_path = Path::new(r"F:\working_ai\kiro2api-cc\credentials.json");
        if !credentials_path.exists() {
            println!("凭据文件不存在，跳过测试");
            return;
        }

        let content = std::fs::read_to_string(credentials_path).expect("读取凭据文件失败");
        let credentials_list: Vec<KiroCredentials> =
            serde_json::from_str(&content).expect("解析凭据文件失败");

        // 找到 IdC 凭据（id=14 或 id=15）
        let idc_credentials: Vec<_> = credentials_list
            .iter()
            .filter(|c| {
                c.auth_method
                    .as_ref()
                    .map(|m| m.to_lowercase() == "idc")
                    .unwrap_or(false)
            })
            .collect();

        if idc_credentials.is_empty() {
            println!("未找到 IdC 凭据，跳过测试");
            return;
        }

        println!("找到 {} 个 IdC 凭据", idc_credentials.len());

        let config = Config::default();

        for cred in idc_credentials {
            let id = cred.id.unwrap_or(0);
            println!("\n========================================");
            println!("测试凭据 #{}", id);
            println!("========================================");

            // 先刷新 Token
            let refreshed_cred = match refresh_idc_token(cred, &config, None).await {
                Ok(c) => {
                    println!("Token 刷新成功");
                    c
                }
                Err(e) => {
                    println!("Token 刷新失败: {}", e);
                    continue;
                }
            };

            let access_token = match refreshed_cred.access_token.as_ref() {
                Some(t) => t,
                None => {
                    println!("刷新后无 accessToken");
                    continue;
                }
            };

            // 构建请求 - 使用 Kiro 的模型 ID 格式
            let model_ids = [
                "simple-task",           // 简单任务/意图分类
                "claude-sonnet-4.5",     // Sonnet 模型
                "claude-opus-4.5",       // Opus 模型
                "claude-haiku-4.5",      // Haiku 模型
            ];

            for model_id in model_ids {
                println!("\n--- 尝试模型: {} ---", model_id);

                let conversation_id = uuid::Uuid::new_v4().to_string();
                let user_message = UserInputMessage::new("你好", model_id)
                    .with_origin("AI_EDITOR");
                let current_message = CurrentMessage::new(user_message);
                let conversation_state = ConversationState::new(&conversation_id)
                    .with_agent_task_type("vibe")
                    .with_chat_trigger_type("MANUAL")
                    .with_current_message(current_message);

                let request = KiroRequest {
                    conversation_state,
                    profile_arn: refreshed_cred.profile_arn.clone(),
                };

                let request_body = serde_json::to_string_pretty(&request).expect("序列化请求失败");

                // 构建请求头
                let machine_id = machine_id::generate_from_credentials(&refreshed_cred, &config)
                    .unwrap_or_else(|| "unknown".to_string());
                let kiro_version = &config.kiro_version;
                let os_name = &config.system_version;
                let node_version = &config.node_version;

                let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{}-{}", kiro_version, machine_id);
                let user_agent = format!(
                    "aws-sdk-js/1.0.27 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.27 m/E KiroIDE-{}-{}",
                    os_name, node_version, kiro_version, machine_id
                );

                let region = &config.region;
                let url = format!("https://q.{}.amazonaws.com/generateAssistantResponse", region);
                let host = format!("q.{}.amazonaws.com", region);

                // 打印请求信息
                println!("\n--- 请求 URL ---");
                println!("{}", url);

                println!("\n--- 请求头 ---");
                println!("Content-Type: application/json");
                println!("Host: {}", host);
                println!("x-amzn-codewhisperer-optout: true");
                println!("x-amzn-kiro-agent-mode: vibe");
                println!("x-amz-user-agent: {}", x_amz_user_agent);
                println!("User-Agent: {}", user_agent);
                println!("amz-sdk-invocation-id: <uuid>");
                println!("amz-sdk-request: attempt=1; max=3");
                println!("Authorization: Bearer {}...", &access_token[..50.min(access_token.len())]);
                println!("Connection: close");

                println!("\n--- 请求体 ---");
                println!("{}", request_body);

                println!("\n--- profileArn ---");
                println!("{:?}", refreshed_cred.profile_arn);

                // 发送请求
                let client = build_client(None, 60).expect("构建 HTTP Client 失败");

                let response = client
                    .post(&url)
                    .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                    .header("x-amzn-codewhisperer-optout", HeaderValue::from_static("true"))
                    .header("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"))
                    .header("x-amz-user-agent", HeaderValue::from_str(&x_amz_user_agent).expect("header"))
                    .header(reqwest::header::USER_AGENT, HeaderValue::from_str(&user_agent).expect("header"))
                    .header(HOST, HeaderValue::from_str(&host).expect("header"))
                    .header("amz-sdk-invocation-id", HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()).expect("header"))
                    .header("amz-sdk-request", HeaderValue::from_static("attempt=1; max=3"))
                    .header(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", access_token)).expect("header"))
                    .header(CONNECTION, HeaderValue::from_static("close"))
                    .body(request_body.clone())
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status();
                        println!("\n--- 响应状态 ---");
                        println!("{}", status);

                        println!("\n--- 响应头 ---");
                        for (name, value) in resp.headers() {
                            println!("{}: {:?}", name, value);
                        }

                        let body = resp.text().await.unwrap_or_default();
                        println!("\n--- 响应体 ---");
                        println!("{}", body);

                        // 如果成功，跳出循环
                        if status.is_success() {
                            println!("\n!!! 成功 !!!");
                            break;
                        }

                        if status.as_u16() == 403 {
                            println!("\n!!! 403 错误分析 !!!");
                            println!("可能原因:");
                            println!("1. profileArn 不正确或缺失");
                            println!("2. accessToken 权限不足");
                            println!("3. 请求头格式不正确");
                            println!("4. IdC 凭据需要特殊的 profileArn");
                        }
                    }
                    Err(e) => {
                        println!("\n--- 请求失败 ---");
                        println!("{}", e);
                    }
                }
            } // end for model_id
        }
    }

    /// 完整诊断 IdC 403 错误
    /// 测试完整的 IdC 认证流程：Token 刷新 -> 获取 profileArn -> 发送 API 请求
    #[tokio::test]
    #[ignore] // 需要有效的 IdC 凭据才能运行，使用 cargo test -- --ignored 运行
    async fn test_idc_full_flow_diagnosis() {
        use crate::http_client::build_client;
        use crate::kiro::machine_id;
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, UserInputMessage,
        };
        use crate::kiro::model::requests::kiro::KiroRequest;
        use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, HeaderValue};
        use std::path::Path;

        println!("\n");
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║           IdC 403 错误完整诊断测试                           ║");
        println!("╚══════════════════════════════════════════════════════════════╝");

        // 从外部文件加载凭据
        let credentials_path = Path::new(r"F:\working_ai\kiro2api-cc\credentials.json");
        if !credentials_path.exists() {
            println!("凭据文件不存在，跳过测试");
            return;
        }

        let content = std::fs::read_to_string(credentials_path).expect("读取凭据文件失败");
        let credentials_list: Vec<KiroCredentials> =
            serde_json::from_str(&content).expect("解析凭据文件失败");

        // 找到 IdC 凭据
        let idc_credentials: Vec<_> = credentials_list
            .iter()
            .filter(|c| {
                c.auth_method
                    .as_ref()
                    .map(|m| m.to_lowercase() == "idc")
                    .unwrap_or(false)
            })
            .collect();

        if idc_credentials.is_empty() {
            println!("未找到 IdC 凭据，跳过测试");
            return;
        }

        println!("找到 {} 个 IdC 凭据\n", idc_credentials.len());

        let config = Config::default();

        for cred in idc_credentials {
            let id = cred.id.unwrap_or(0);
            println!("════════════════════════════════════════════════════════════════");
            println!("测试凭据 #{}", id);
            println!("════════════════════════════════════════════════════════════════");

            // ========== 步骤 1: 检查原始凭据 ==========
            println!("\n【步骤 1】检查原始凭据");
            println!("  - clientId: {:?}", cred.client_id.as_ref().map(|s| format!("{}...", &s[..20.min(s.len())])));
            println!("  - clientSecret: {:?}", cred.client_secret.as_ref().map(|s| format!("{}...", &s[..30.min(s.len())])));
            println!("  - refreshToken: {:?}", cred.refresh_token.as_ref().map(|s| format!("{}...", &s[..30.min(s.len())])));
            println!("  - 原始 profileArn: {:?}", cred.profile_arn);
            println!("  - 原始 expiresAt: {:?}", cred.expires_at);
            println!("  - authMethod: {:?}", cred.auth_method);

            // ========== 步骤 2: 刷新 IdC Token ==========
            println!("\n【步骤 2】刷新 IdC Token");
            let refreshed_cred = match refresh_idc_token(cred, &config, None).await {
                Ok(c) => {
                    println!("  ✓ Token 刷新成功");
                    println!("  - 新 accessToken: {}...", &c.access_token.as_ref().map(|s| &s[..50.min(s.len())]).unwrap_or(&"<无>"));
                    println!("  - 新 expiresAt: {:?}", c.expires_at);
                    println!("  - Token 是否过期: {}", is_token_expired(&c));
                    println!("  - Token 是否即将过期: {}", is_token_expiring_soon(&c));
                    c
                }
                Err(e) => {
                    println!("  ✗ Token 刷新失败: {}", e);
                    continue;
                }
            };

            let access_token = match refreshed_cred.access_token.as_ref() {
                Some(t) => t,
                None => {
                    println!("  ✗ 刷新后无 accessToken");
                    continue;
                }
            };

            // ========== 步骤 3: 获取 profileArn ==========
            println!("\n【步骤 3】获取 profileArn (ListAvailableProfiles)");
            let profile_arn = if let Some(arn) = &refreshed_cred.profile_arn {
                println!("  - 使用原始 profileArn: {}", arn);
                Some(arn.clone())
            } else {
                println!("  - 原始 profileArn 为空，尝试调用 ListAvailableProfiles...");
                match list_available_profiles(&refreshed_cred, &config, access_token, None).await {
                    Ok(Some(arn)) => {
                        println!("  ✓ 成功获取 profileArn: {}", arn);
                        Some(arn)
                    }
                    Ok(None) => {
                        println!("  ⚠ ListAvailableProfiles 返回空（可能是退避/缓存/空列表）");
                        None
                    }
                    Err(e) => {
                        println!("  ✗ ListAvailableProfiles 失败: {}", e);
                        None
                    }
                }
            };

            // ========== 步骤 4: 构建并发送 API 请求 ==========
            println!("\n【步骤 4】发送 API 请求");

            let conversation_id = uuid::Uuid::new_v4().to_string();
            let user_message = UserInputMessage::new("你好", "claude-sonnet-4.5")
                .with_origin("AI_EDITOR");
            let current_message = CurrentMessage::new(user_message);
            let conversation_state = ConversationState::new(&conversation_id)
                .with_agent_task_type("vibe")
                .with_chat_trigger_type("MANUAL")
                .with_current_message(current_message);

            let request = KiroRequest {
                conversation_state,
                profile_arn: profile_arn.clone(),
            };

            let request_body = serde_json::to_string_pretty(&request).expect("序列化请求失败");

            // 构建请求头
            let machine_id = machine_id::generate_from_credentials(&refreshed_cred, &config)
                .unwrap_or_else(|| "unknown".to_string());
            let kiro_version = &config.kiro_version;
            let os_name = &config.system_version;
            let node_version = &config.node_version;

            let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{}-{}", kiro_version, machine_id);
            let user_agent = format!(
                "aws-sdk-js/1.0.27 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.27 m/E KiroIDE-{}-{}",
                os_name, node_version, kiro_version, machine_id
            );

            let region = &config.region;
            let url = format!("https://q.{}.amazonaws.com/generateAssistantResponse", region);
            let host = format!("q.{}.amazonaws.com", region);

            println!("  - URL: {}", url);
            println!("  - profileArn in request: {:?}", profile_arn);
            println!("  - machineId: {}", machine_id);
            println!("  - Authorization: Bearer {}...", &access_token[..50.min(access_token.len())]);

            // 发送请求
            let client = build_client(None, 60).expect("构建 HTTP Client 失败");

            let response = client
                .post(&url)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .header("x-amzn-codewhisperer-optout", HeaderValue::from_static("true"))
                .header("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"))
                .header("x-amz-user-agent", HeaderValue::from_str(&x_amz_user_agent).expect("header"))
                .header(reqwest::header::USER_AGENT, HeaderValue::from_str(&user_agent).expect("header"))
                .header(HOST, HeaderValue::from_str(&host).expect("header"))
                .header("amz-sdk-invocation-id", HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()).expect("header"))
                .header("amz-sdk-request", HeaderValue::from_static("attempt=1; max=3"))
                .header(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", access_token)).expect("header"))
                .header(CONNECTION, HeaderValue::from_static("close"))
                .body(request_body.clone())
                .send()
                .await;

            match response {
                Ok(resp) => {
                    let status = resp.status();
                    println!("\n【响应结果】");
                    println!("  - 状态码: {}", status);

                    if status.is_success() {
                        println!("  ✓ 请求成功！");
                        let body = resp.text().await.unwrap_or_default();
                        println!("  - 响应体前 500 字符: {}", &body[..500.min(body.len())]);
                    } else {
                        let body = resp.text().await.unwrap_or_default();
                        println!("  ✗ 请求失败");
                        println!("  - 响应体: {}", body);

                        if status.as_u16() == 403 {
                            println!("\n【403 错误诊断】");
                            if profile_arn.is_none() {
                                println!("  ⚠ profileArn 为空！这很可能是 403 的原因");
                                println!("  → 建议：检查 ListAvailableProfiles 为何返回空");
                            }
                            if body.contains("bearer token") || body.contains("invalid") {
                                println!("  ⚠ Token 被认为无效");
                                println!("  → 可能原因：");
                                println!("    1. IdC Token 需要配合正确的 profileArn 使用");
                                println!("    2. Token 权限不足（scope 问题）");
                                println!("    3. Token 已被撤销");
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("\n【网络错误】");
                    println!("  ✗ 请求发送失败: {}", e);
                }
            }

            println!("\n");
        }

        println!("════════════════════════════════════════════════════════════════");
        println!("诊断测试完成");
        println!("════════════════════════════════════════════════════════════════");
    }

    /// 诊断 call_api_with_retry 在 IdC 模式下的 403 错误
    ///
    /// 此测试模拟实际的 API 调用流程，对比以下两种情况：
    /// 1. 使用凭据中的 profileArn（可能为空或过期）
    /// 2. 使用 ListAvailableProfiles 获取的最新 profileArn
    ///
    /// 输出完整的请求体 DUMP 以便诊断
    #[tokio::test]
    #[ignore] // 需要有效的 IdC 凭据才能运行，使用 cargo test -- --ignored 运行
    async fn test_idc_call_api_with_retry_diagnosis() {
        use crate::http_client::build_client;
        use crate::kiro::machine_id;
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, UserInputMessage,
        };
        use crate::kiro::model::requests::kiro::KiroRequest;
        use crate::kiro::provider::KiroProvider;
        use crate::model::config::Config;
        use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, HeaderValue};
        use std::path::Path;
        use std::sync::Arc;

        println!("\n");
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║     call_api_with_retry IdC 403 错误诊断测试                 ║");
        println!("╚══════════════════════════════════════════════════════════════╝");

        // 从外部文件加载凭据
        let credentials_path = Path::new(r"F:\working_ai\kiro2api-cc\credentials.json");
        if !credentials_path.exists() {
            println!("凭据文件不存在，跳过测试");
            return;
        }

        let content = std::fs::read_to_string(credentials_path).expect("读取凭据文件失败");
        let credentials_list: Vec<KiroCredentials> =
            serde_json::from_str(&content).expect("解析凭据文件失败");

        // 找到 IdC 凭据
        let idc_credentials: Vec<KiroCredentials> = credentials_list
            .into_iter()
            .filter(|c| {
                c.auth_method
                    .as_ref()
                    .map(|m| m.to_lowercase() == "idc")
                    .unwrap_or(false)
            })
            .collect();

        if idc_credentials.is_empty() {
            println!("未找到 IdC 凭据，跳过测试");
            return;
        }

        println!("找到 {} 个 IdC 凭据\n", idc_credentials.len());

        let config = Config::default();

        // ========== 测试 1: 使用 MultiTokenManager + KiroProvider（模拟实际流程）==========
        println!("════════════════════════════════════════════════════════════════");
        println!("【测试 1】使用 MultiTokenManager + KiroProvider（模拟实际流程）");
        println!("════════════════════════════════════════════════════════════════");

        let token_manager = match MultiTokenManager::new(
            config.clone(),
            idc_credentials.clone(),
            None,
            None,
            false,
        ) {
            Ok(tm) => Arc::new(tm),
            Err(e) => {
                println!("创建 MultiTokenManager 失败: {}", e);
                return;
            }
        };

        let provider = match KiroProvider::new(token_manager.clone()) {
            Ok(p) => Arc::new(p),
            Err(e) => {
                println!("创建 KiroProvider 失败: {}", e);
                return;
            }
        };

        // 获取调用上下文
        println!("\n【步骤 1.1】获取调用上下文 (acquire_context)");
        let ctx = match token_manager.acquire_context().await {
            Ok(c) => {
                println!("  ✓ 获取上下文成功");
                println!("  - credential_id: {}", c.id);
                println!("  - token: {}...", &c.token[..50.min(c.token.len())]);
                println!("  - credentials.profile_arn: {:?}", c.credentials.profile_arn);
                println!("  - credentials.auth_method: {:?}", c.credentials.auth_method);
                c
            }
            Err(e) => {
                println!("  ✗ 获取上下文失败: {}", e);
                return;
            }
        };

        // 构建请求体（模拟 handlers.rs 的行为）
        println!("\n【步骤 1.2】构建请求体（模拟 handlers.rs）");
        let conversation_id = uuid::Uuid::new_v4().to_string();
        let user_message = UserInputMessage::new("你好", "claude-sonnet-4.5")
            .with_origin("AI_EDITOR");
        let current_message = CurrentMessage::new(user_message);
        let conversation_state = ConversationState::new(&conversation_id)
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(current_message);

        // 关键：这里使用凭据中的 profile_arn（模拟 handlers.rs 的行为）
        let request_with_cred_arn = KiroRequest {
            conversation_state: conversation_state.clone(),
            profile_arn: ctx.credentials.profile_arn.clone(),
        };

        let request_body_with_cred_arn = serde_json::to_string_pretty(&request_with_cred_arn)
            .expect("序列化请求失败");

        println!("  - 使用凭据中的 profileArn: {:?}", ctx.credentials.profile_arn);
        println!("\n【请求体 DUMP（使用凭据 profileArn）】");
        println!("{}", request_body_with_cred_arn);

        // 发送请求
        println!("\n【步骤 1.3】发送 API 请求（使用凭据 profileArn）");
        match provider.call_api(&request_body_with_cred_arn).await {
            Ok(resp) => {
                let status = resp.status();
                println!("  - 状态码: {}", status);
                if status.is_success() {
                    println!("  ✓ 请求成功！");
                    let body = resp.text().await.unwrap_or_default();
                    println!("  - 响应体前 500 字符: {}", &body[..500.min(body.len())]);
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    println!("  ✗ 请求失败");
                    println!("  - 响应体: {}", body);
                }
            }
            Err(e) => {
                println!("  ✗ 请求失败: {}", e);
            }
        }

        // ========== 测试 2: 手动获取最新 profileArn 后发送请求 ==========
        println!("\n════════════════════════════════════════════════════════════════");
        println!("【测试 2】手动获取最新 profileArn 后发送请求");
        println!("════════════════════════════════════════════════════════════════");

        // 重新获取上下文（确保 token 有效）
        let ctx2 = match token_manager.acquire_context().await {
            Ok(c) => c,
            Err(e) => {
                println!("获取上下文失败: {}", e);
                return;
            }
        };

        // 调用 ListAvailableProfiles 获取最新的 profileArn
        println!("\n【步骤 2.1】调用 ListAvailableProfiles 获取最新 profileArn");
        let fresh_profile_arn = match list_available_profiles(
            &ctx2.credentials,
            &config,
            &ctx2.token,
            None,
        ).await {
            Ok(Some(arn)) => {
                println!("  ✓ 成功获取 profileArn: {}", arn);
                Some(arn)
            }
            Ok(None) => {
                println!("  ⚠ ListAvailableProfiles 返回空");
                None
            }
            Err(e) => {
                println!("  ✗ ListAvailableProfiles 失败: {}", e);
                None
            }
        };

        // 对比两个 profileArn
        println!("\n【步骤 2.2】对比 profileArn");
        println!("  - 凭据中的 profileArn: {:?}", ctx2.credentials.profile_arn);
        println!("  - 新获取的 profileArn: {:?}", fresh_profile_arn);

        let are_same = match (&ctx2.credentials.profile_arn, &fresh_profile_arn) {
            (Some(a), Some(b)) => a == b,
            (None, None) => true,
            _ => false,
        };
        println!("  - 两者是否相同: {}", are_same);

        // 使用新获取的 profileArn 构建请求
        println!("\n【步骤 2.3】使用新获取的 profileArn 构建请求");
        let conversation_id2 = uuid::Uuid::new_v4().to_string();
        let user_message2 = UserInputMessage::new("你好", "claude-sonnet-4.5")
            .with_origin("AI_EDITOR");
        let current_message2 = CurrentMessage::new(user_message2);
        let conversation_state2 = ConversationState::new(&conversation_id2)
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(current_message2);

        let request_with_fresh_arn = KiroRequest {
            conversation_state: conversation_state2,
            profile_arn: fresh_profile_arn.clone(),
        };

        let request_body_with_fresh_arn = serde_json::to_string_pretty(&request_with_fresh_arn)
            .expect("序列化请求失败");

        println!("  - 使用新获取的 profileArn: {:?}", fresh_profile_arn);
        println!("\n【请求体 DUMP（使用新获取的 profileArn）】");
        println!("{}", request_body_with_fresh_arn);

        // 发送请求
        println!("\n【步骤 2.4】发送 API 请求（使用新获取的 profileArn）");
        match provider.call_api(&request_body_with_fresh_arn).await {
            Ok(resp) => {
                let status = resp.status();
                println!("  - 状态码: {}", status);
                if status.is_success() {
                    println!("  ✓ 请求成功！");
                    let body = resp.text().await.unwrap_or_default();
                    println!("  - 响应体前 500 字符: {}", &body[..500.min(body.len())]);
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    println!("  ✗ 请求失败");
                    println!("  - 响应体: {}", body);
                }
            }
            Err(e) => {
                println!("  ✗ 请求失败: {}", e);
            }
        }

        // ========== 测试 3: 直接使用 HTTP Client 发送请求（完整 DUMP）==========
        println!("\n════════════════════════════════════════════════════════════════");
        println!("【测试 3】直接使用 HTTP Client 发送请求（完整请求头 DUMP）");
        println!("════════════════════════════════════════════════════════════════");

        let ctx3 = match token_manager.acquire_context().await {
            Ok(c) => c,
            Err(e) => {
                println!("获取上下文失败: {}", e);
                return;
            }
        };

        let machine_id = machine_id::generate_from_credentials(&ctx3.credentials, &config)
            .unwrap_or_else(|| "unknown".to_string());
        let kiro_version = &config.kiro_version;
        let os_name = &config.system_version;
        let node_version = &config.node_version;

        let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{}-{}", kiro_version, machine_id);
        let user_agent = format!(
            "aws-sdk-js/1.0.27 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.27 m/E KiroIDE-{}-{}",
            os_name, node_version, kiro_version, machine_id
        );

        let region = &config.region;
        let url = format!("https://q.{}.amazonaws.com/generateAssistantResponse", region);
        let host = format!("q.{}.amazonaws.com", region);

        println!("\n【完整请求信息 DUMP】");
        println!("URL: {}", url);
        println!("\n请求头:");
        println!("  Content-Type: application/json");
        println!("  Host: {}", host);
        println!("  x-amzn-codewhisperer-optout: true");
        println!("  x-amzn-kiro-agent-mode: vibe");
        println!("  x-amz-user-agent: {}", x_amz_user_agent);
        println!("  User-Agent: {}", user_agent);
        println!("  amz-sdk-invocation-id: <uuid>");
        println!("  amz-sdk-request: attempt=1; max=3");
        println!("  Authorization: Bearer {}...", &ctx3.token[..50.min(ctx3.token.len())]);
        println!("  Connection: close");

        // 测试 3.1: 使用凭据中的 profileArn
        println!("\n【测试 3.1】使用凭据中的 profileArn");
        let conversation_id3 = uuid::Uuid::new_v4().to_string();
        let user_message3 = UserInputMessage::new("你好", "claude-sonnet-4.5")
            .with_origin("AI_EDITOR");
        let current_message3 = CurrentMessage::new(user_message3);
        let conversation_state3 = ConversationState::new(&conversation_id3)
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(current_message3);

        let request3a = KiroRequest {
            conversation_state: conversation_state3.clone(),
            profile_arn: ctx3.credentials.profile_arn.clone(),
        };
        let request_body3a = serde_json::to_string(&request3a).expect("序列化失败");

        println!("请求体: {}", request_body3a);

        let client = build_client(None, 60).expect("构建 HTTP Client 失败");
        let response3a = client
            .post(&url)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header("x-amzn-codewhisperer-optout", HeaderValue::from_static("true"))
            .header("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"))
            .header("x-amz-user-agent", HeaderValue::from_str(&x_amz_user_agent).expect("header"))
            .header(reqwest::header::USER_AGENT, HeaderValue::from_str(&user_agent).expect("header"))
            .header(HOST, HeaderValue::from_str(&host).expect("header"))
            .header("amz-sdk-invocation-id", HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()).expect("header"))
            .header("amz-sdk-request", HeaderValue::from_static("attempt=1; max=3"))
            .header(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", ctx3.token)).expect("header"))
            .header(CONNECTION, HeaderValue::from_static("close"))
            .body(request_body3a)
            .send()
            .await;

        match response3a {
            Ok(resp) => {
                let status = resp.status();
                println!("\n响应状态: {}", status);
                println!("响应头:");
                for (name, value) in resp.headers() {
                    println!("  {}: {:?}", name, value);
                }
                let body = resp.text().await.unwrap_or_default();
                println!("响应体: {}", body);
            }
            Err(e) => {
                println!("请求失败: {}", e);
            }
        }

        // 测试 3.2: 使用新获取的 profileArn
        if fresh_profile_arn.is_some() && fresh_profile_arn != ctx3.credentials.profile_arn {
            println!("\n【测试 3.2】使用新获取的 profileArn");
            let conversation_id3b = uuid::Uuid::new_v4().to_string();
            let user_message3b = UserInputMessage::new("你好", "claude-sonnet-4.5")
                .with_origin("AI_EDITOR");
            let current_message3b = CurrentMessage::new(user_message3b);
            let conversation_state3b = ConversationState::new(&conversation_id3b)
                .with_agent_task_type("vibe")
                .with_chat_trigger_type("MANUAL")
                .with_current_message(current_message3b);

            let request3b = KiroRequest {
                conversation_state: conversation_state3b,
                profile_arn: fresh_profile_arn.clone(),
            };
            let request_body3b = serde_json::to_string(&request3b).expect("序列化失败");

            println!("请求体: {}", request_body3b);

            let response3b = client
                .post(&url)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .header("x-amzn-codewhisperer-optout", HeaderValue::from_static("true"))
                .header("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"))
                .header("x-amz-user-agent", HeaderValue::from_str(&x_amz_user_agent).expect("header"))
                .header(reqwest::header::USER_AGENT, HeaderValue::from_str(&user_agent).expect("header"))
                .header(HOST, HeaderValue::from_str(&host).expect("header"))
                .header("amz-sdk-invocation-id", HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()).expect("header"))
                .header("amz-sdk-request", HeaderValue::from_static("attempt=1; max=3"))
                .header(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", ctx3.token)).expect("header"))
                .header(CONNECTION, HeaderValue::from_static("close"))
                .body(request_body3b)
                .send()
                .await;

            match response3b {
                Ok(resp) => {
                    let status = resp.status();
                    println!("\n响应状态: {}", status);
                    println!("响应头:");
                    for (name, value) in resp.headers() {
                        println!("  {}: {:?}", name, value);
                    }
                    let body = resp.text().await.unwrap_or_default();
                    println!("响应体: {}", body);
                }
                Err(e) => {
                    println!("请求失败: {}", e);
                }
            }
        }

        // ========== 诊断总结 ==========
        println!("\n════════════════════════════════════════════════════════════════");
        println!("【诊断总结】");
        println!("════════════════════════════════════════════════════════════════");
        println!("1. test_idc_full_flow_diagnosis 成功的原因:");
        println!("   - 每次都调用 ListAvailableProfiles 获取最新的 profileArn");
        println!("   - 将获取到的 profileArn 直接放入请求体");
        println!("");
        println!("2. call_api_with_retry 可能失败的原因:");
        println!("   - handlers.rs 使用 state.profile_arn（全局配置）");
        println!("   - 而不是从当前凭据中动态获取 profileArn");
        println!("   - 对于 IdC 凭据，每个凭据可能有不同的 profileArn");
        println!("");
        println!("3. 建议修复方案:");
        println!("   - 在 acquire_context 时确保 profileArn 是最新的");
        println!("   - 或者在 handlers.rs 中使用凭据的 profileArn 而非全局配置");
        println!("════════════════════════════════════════════════════════════════");
    }
}
