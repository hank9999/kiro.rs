//! Admin API 业务逻辑服务

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http_client::ProxyConfig;
use crate::kiro::auth::idc::{self, BUILDER_ID_START_URL};
use crate::kiro::auth::social;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;
use crate::model::config::Config;

use super::error::AdminServiceError;
use super::proxy_pool::{GetUrlResult, ProxyPoolManager};
use super::types::{
    AddCredentialRequest, AddCredentialResponse, AssignProxyRequest, BalanceResponse,
    BatchAddProxyRequest, CredentialStatusItem, CredentialsStatusResponse,
    LoadBalancingModeResponse, PollIdcLoginResponse, ProxyPoolEntry, ProxyPoolResponse,
    SetLoadBalancingModeRequest, StartIdcLoginRequest, StartIdcLoginResponse,
    StartSocialLoginRequest, StartSocialLoginResponse,
    UpdateCredentialRequest, UpdateRefreshTokenRequest,
};

/// 余额缓存过期时间（秒），5 分钟
const BALANCE_CACHE_TTL_SECS: i64 = 300;

/// 缓存的余额条目（含时间戳）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedBalance {
    /// 缓存时间（Unix 秒）
    cached_at: f64,
    /// 缓存的余额数据
    data: BalanceResponse,
}

/// Admin 服务
///
/// 封装所有 Admin API 的业务逻辑
pub struct AdminService {
    token_manager: Arc<MultiTokenManager>,
    balance_cache: Mutex<HashMap<u64, CachedBalance>>,
    cache_path: Option<PathBuf>,
    /// 已注册的端点名称集合（用于 add_credential 校验）
    known_endpoints: HashSet<String>,
    /// 代理 IP 池管理器
    proxy_pool: ProxyPoolManager,
    /// 进行中的 IdC 设备授权会话
    idc_sessions: Arc<Mutex<HashMap<String, IdcAuthSession>>>,
    /// 进行中的 Social 登录会话
    social_sessions: Arc<Mutex<HashMap<String, SocialAuthSession>>>,
}

/// Social 登录会话状态
struct SocialAuthSession {
    auth_endpoint: String,
    /// 发起时生成的 state，用于 CSRF 验证
    state: String,
    code_verifier: String,
    redirect_uri: String,
    expires_at: DateTime<Utc>,
    /// 收到 OAuth 回调时的数据（code + login_option + path）
    callback_rx: tokio::sync::Mutex<tokio::sync::oneshot::Receiver<social::OAuthCallbackData>>,
    cred_template: KiroCredentials,
    proxy: Option<ProxyConfig>,
    /// Drop 时自动关闭回调服务器并释放端口
    _server_handle: social::ServerHandle,
    /// 重新登录时更新此凭据的 Token（非 None 时更新已有凭据而非创建新凭据）
    relogin_target_id: Option<u64>,
}

/// IdC 设备授权会话状态
struct IdcAuthSession {
    region: String,
    client_id: String,
    client_secret: String,
    device_code: String,
    expires_at: DateTime<Utc>,
    poll_interval: i64,
    /// 登录成功后写入的凭据配置
    cred_template: KiroCredentials,
    /// 用于发起 token 请求的代理
    proxy: Option<ProxyConfig>,
    /// 重新登录时更新此凭据的 Token（非 None 时更新已有凭据而非创建新凭据）
    relogin_target_id: Option<u64>,
}

impl AdminService {
    pub fn new(
        token_manager: Arc<MultiTokenManager>,
        known_endpoints: impl IntoIterator<Item = String>,
    ) -> Self {
        let cache_path = token_manager
            .cache_dir()
            .map(|d| d.join("kiro_balance_cache.json"));

        let proxy_pool_path = token_manager
            .cache_dir()
            .map(|d| d.join("proxy_pool.json"));

        let balance_cache = Self::load_balance_cache_from(&cache_path);

        let svc = Self {
            token_manager,
            balance_cache: Mutex::new(balance_cache),
            cache_path,
            known_endpoints: known_endpoints.into_iter().collect(),
            proxy_pool: ProxyPoolManager::new(proxy_pool_path),
            idc_sessions: Arc::new(Mutex::new(HashMap::new())),
            social_sessions: Arc::new(Mutex::new(HashMap::new())),
        };

        // 后台任务：每 5 分钟清理过期的登录会话，防止内存泄漏
        {
            let idc = Arc::clone(&svc.idc_sessions);
            let social = Arc::clone(&svc.social_sessions);
            tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(std::time::Duration::from_secs(300));
                loop {
                    interval.tick().await;
                    let now = Utc::now();
                    idc.lock().retain(|_, s| now < s.expires_at);
                    social.lock().retain(|_, s| now < s.expires_at);
                }
            });
        }

        svc
    }

    /// 获取所有凭据状态
    pub fn get_all_credentials(&self) -> CredentialsStatusResponse {
        let snapshot = self.token_manager.snapshot();
        let default_endpoint = self.token_manager.config().default_endpoint.clone();

        let mut credentials: Vec<CredentialStatusItem> = snapshot
            .entries
            .into_iter()
            .map(|entry| CredentialStatusItem {
                id: entry.id,
                priority: entry.priority,
                disabled: entry.disabled,
                failure_count: entry.failure_count,
                is_current: entry.id == snapshot.current_id,
                expires_at: entry.expires_at,
                auth_method: entry.auth_method,
                has_profile_arn: entry.has_profile_arn,
                refresh_token_hash: entry.refresh_token_hash,
                api_key_hash: entry.api_key_hash,
                masked_api_key: entry.masked_api_key,
                email: entry.email,
                success_count: entry.success_count,
                last_used_at: entry.last_used_at.clone(),
                has_proxy: entry.has_proxy,
                proxy_url: entry.proxy_url,
                refresh_failure_count: entry.refresh_failure_count,
                disabled_reason: entry.disabled_reason,
                endpoint: entry.endpoint.unwrap_or_else(|| default_endpoint.clone()),
            })
            .collect();

        // 按优先级排序（数字越小优先级越高）
        credentials.sort_by_key(|c| c.priority);

        CredentialsStatusResponse {
            total: snapshot.total,
            available: snapshot.available,
            current_id: snapshot.current_id,
            credentials,
        }
    }

    /// 设置凭据禁用状态
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<(), AdminServiceError> {
        // 先获取当前凭据 ID，用于判断是否需要切换
        let snapshot = self.token_manager.snapshot();
        let current_id = snapshot.current_id;

        self.token_manager
            .set_disabled(id, disabled)
            .map_err(|e| self.classify_error(e, id))?;

        // 只有禁用的是当前凭据时才尝试切换到下一个
        if disabled && id == current_id {
            let _ = self.token_manager.switch_to_next();
        }
        Ok(())
    }

    /// 设置凭据优先级
    pub fn set_priority(&self, id: u64, priority: u32) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_priority(id, priority)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 重置失败计数并重新启用
    pub fn reset_and_enable(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .reset_and_enable(id)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 获取凭据余额（带缓存）
    pub async fn get_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        // 先查缓存
        {
            let cache = self.balance_cache.lock();
            if let Some(cached) = cache.get(&id) {
                let now = Utc::now().timestamp() as f64;
                if (now - cached.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    tracing::debug!("凭据 #{} 余额命中缓存", id);
                    return Ok(cached.data.clone());
                }
            }
        }

        // 缓存未命中或已过期，从上游获取
        let balance = self.fetch_balance(id).await?;

        // 更新缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.insert(
                id,
                CachedBalance {
                    cached_at: Utc::now().timestamp() as f64,
                    data: balance.clone(),
                },
            );
        }
        self.save_balance_cache();

        Ok(balance)
    }

    /// 从上游获取余额（无缓存）
    async fn fetch_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        let usage = self
            .token_manager
            .get_usage_limits_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        let current_usage = usage.current_usage();
        let usage_limit = usage.usage_limit();
        let remaining = (usage_limit - current_usage).max(0.0);
        let usage_percentage = if usage_limit > 0.0 {
            (current_usage / usage_limit * 100.0).min(100.0)
        } else {
            0.0
        };

        Ok(BalanceResponse {
            id,
            subscription_title: usage.subscription_title().map(|s| s.to_string()),
            current_usage,
            usage_limit,
            remaining,
            usage_percentage,
            next_reset_at: usage.next_date_reset,
        })
    }

    /// 添加新凭据
    pub async fn add_credential(
        &self,
        req: AddCredentialRequest,
    ) -> Result<AddCredentialResponse, AdminServiceError> {
        // 校验端点名：未指定则默认合法，指定则必须已注册
        if let Some(ref name) = req.endpoint {
            if !self.known_endpoints.contains(name) {
                let mut known: Vec<&str> =
                    self.known_endpoints.iter().map(|s| s.as_str()).collect();
                known.sort();
                return Err(AdminServiceError::InvalidCredential(format!(
                    "未知端点 \"{}\"，已注册端点: {:?}",
                    name, known
                )));
            }
        }

        // 构建凭据对象
        let email = req.email.clone();
        let new_cred = KiroCredentials {
            id: None,
            access_token: None,
            refresh_token: req.refresh_token,
            profile_arn: None,
            expires_at: None,
            auth_method: Some(req.auth_method),
            client_id: req.client_id,
            client_secret: req.client_secret,
            priority: req.priority,
            region: req.region,
            auth_region: req.auth_region,
            api_region: req.api_region,
            machine_id: req.machine_id,
            email: req.email,
            subscription_title: None, // 将在首次获取使用额度时自动更新
            proxy_url: req.proxy_url,
            proxy_username: req.proxy_username,
            proxy_password: req.proxy_password,
            disabled: false, // 新添加的凭据默认启用
            kiro_api_key: req.kiro_api_key,
            endpoint: req.endpoint,
        };

        // 调用 token_manager 添加凭据
        let credential_id = self
            .token_manager
            .add_credential(new_cred)
            .await
            .map_err(|e| self.classify_add_error(e))?;

        // 主动获取订阅等级，避免首次请求时 Free 账号绕过 Opus 模型过滤
        if let Err(e) = self.token_manager.get_usage_limits_for(credential_id).await {
            tracing::warn!("添加凭据后获取订阅等级失败（不影响凭据添加）: {}", e);
        }

        Ok(AddCredentialResponse {
            success: true,
            message: format!("凭据添加成功，ID: {}", credential_id),
            credential_id,
            email,
        })
    }

    /// 更新凭据的可编辑字段（email、proxy 等）
    pub fn update_credential(
        &self,
        id: u64,
        req: UpdateCredentialRequest,
    ) -> Result<(), AdminServiceError> {
        self.token_manager
            .update_credential(
                id,
                req.email.map(|v| if v.is_empty() { None } else { Some(v) }),
                req.proxy_url.map(|v| if v.is_empty() { None } else { Some(v) }),
                req.proxy_username.map(|v| if v.is_empty() { None } else { Some(v) }),
                req.proxy_password.map(|v| if v.is_empty() { None } else { Some(v) }),
            )
            .map_err(|e| self.classify_error(e, id))
    }

    /// 删除凭据
    pub fn delete_credential(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .delete_credential(id)
            .map_err(|e| self.classify_delete_error(e, id))?;

        // 清理已删除凭据的余额缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.remove(&id);
        }
        self.save_balance_cache();

        Ok(())
    }

    /// 从磁盘加载最新配置并应用更新，再写回磁盘。
    ///
    /// 每次读最新文件再写，避免多次调用之间字段互相覆盖。
    fn update_config_file(&self, updater: impl FnOnce(&mut Config)) {
        let base = self.token_manager.config();
        let Some(path) = base.config_path() else {
            return;
        };
        match Config::load(path) {
            Ok(mut fresh) => {
                updater(&mut fresh);
                if let Err(e) = fresh.save() {
                    tracing::warn!("保存配置文件失败: {}", e);
                }
            }
            Err(e) => tracing::warn!("读取配置文件失败（跳过持久化）: {}", e),
        }
    }

    /// 获取全局代理 URL
    pub fn get_global_proxy(&self) -> Option<String> {
        self.token_manager.proxy().map(|p| p.url.clone())
    }

    /// 设置全局代理 URL（None 表示清除）并持久化到配置文件
    pub fn set_global_proxy(&self, url: Option<String>) -> Result<(), AdminServiceError> {
        if let Some(ref u) = url {
            let valid_prefix = u.starts_with("http://")
                || u.starts_with("https://")
                || u.starts_with("socks5://")
                || u.starts_with("socks4://");
            if !valid_prefix {
                return Err(AdminServiceError::InvalidCredential(
                    "代理 URL 格式无效，需以 http://、https://、socks5:// 或 socks4:// 开头"
                        .to_string(),
                ));
            }
        }

        let proxy = url.as_deref().map(ProxyConfig::new);
        self.token_manager.set_global_proxy(proxy);

        // 从磁盘加载最新 config 再写，避免覆盖其他字段的并发修改
        let url_for_save = url;
        self.update_config_file(move |c| c.proxy_url = url_for_save);
        Ok(())
    }

    /// 持久化新的 Admin API Key 到配置文件（内存中的 key 由 handler 层负责更新）
    pub fn persist_admin_key(&self, new_key: &str) {
        let key = new_key.to_string();
        self.update_config_file(move |c| c.admin_api_key = Some(key));
    }

    /// 获取负载均衡模式
    pub fn get_load_balancing_mode(&self) -> LoadBalancingModeResponse {
        LoadBalancingModeResponse {
            mode: self.token_manager.get_load_balancing_mode(),
        }
    }

    /// 设置负载均衡模式
    pub fn set_load_balancing_mode(
        &self,
        req: SetLoadBalancingModeRequest,
    ) -> Result<LoadBalancingModeResponse, AdminServiceError> {
        // 验证模式值
        if req.mode != "priority" && req.mode != "balanced" {
            return Err(AdminServiceError::InvalidCredential(
                "mode 必须是 'priority' 或 'balanced'".to_string(),
            ));
        }

        self.token_manager
            .set_load_balancing_mode(req.mode.clone())
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        Ok(LoadBalancingModeResponse { mode: req.mode })
    }

    /// 更新指定凭据的 refreshToken（仅限已禁用凭据）
    pub fn update_refresh_token(
        &self,
        id: u64,
        req: UpdateRefreshTokenRequest,
    ) -> Result<(), AdminServiceError> {
        self.token_manager
            .update_refresh_token(id, req.refresh_token, req.access_token, req.expires_at)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("不存在") {
                    AdminServiceError::NotFound { id }
                } else if msg.contains("只能为已禁用") || msg.contains("refreshToken 重复") || msg.contains("已被截断") || msg.contains("refreshToken 为空") || msg.contains("缺少 refreshToken") {
                    AdminServiceError::InvalidCredential(msg)
                } else {
                    AdminServiceError::InternalError(msg)
                }
            })
    }

    /// 强制刷新指定凭据的 Token
    pub async fn force_refresh_token(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .force_refresh_token_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))
    }

    // ============ 余额缓存持久化 ============

    fn load_balance_cache_from(cache_path: &Option<PathBuf>) -> HashMap<u64, CachedBalance> {
        let path = match cache_path {
            Some(p) => p,
            None => return HashMap::new(),
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };

        // 文件中使用字符串 key 以兼容 JSON 格式
        let map: HashMap<String, CachedBalance> = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("解析余额缓存失败，将忽略: {}", e);
                return HashMap::new();
            }
        };

        let now = Utc::now().timestamp() as f64;
        map.into_iter()
            .filter_map(|(k, v)| {
                let id = k.parse::<u64>().ok()?;
                // 丢弃超过 TTL 的条目
                if (now - v.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    Some((id, v))
                } else {
                    None
                }
            })
            .collect()
    }

    fn save_balance_cache(&self) {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return,
        };

        // 持有锁期间完成序列化和写入，防止并发损坏
        let cache = self.balance_cache.lock();
        let map: HashMap<String, &CachedBalance> =
            cache.iter().map(|(k, v)| (k.to_string(), v)).collect();

        match serde_json::to_string_pretty(&map) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::warn!("保存余额缓存失败: {}", e);
                }
            }
            Err(e) => tracing::warn!("序列化余额缓存失败: {}", e),
        }
    }

    // ============ 代理池管理 ============

    /// 获取代理池列表（含凭据引用计数）
    pub fn get_proxy_pool(&self) -> ProxyPoolResponse {
        let proxies = self.proxy_pool.list();
        let credentials = {
            let snapshot = self.token_manager.snapshot();
            snapshot.entries
        };

        let pool: Vec<ProxyPoolEntry> = proxies
            .into_iter()
            .map(|p| {
                let count = credentials
                    .iter()
                    .filter(|c| {
                        c.proxy_url
                            .as_deref()
                            .map(|u| u == p.url)
                            .unwrap_or(false)
                    })
                    .count() as u32;
                ProxyPoolEntry {
                    id: p.id,
                    url: p.url,
                    label: p.label,
                    enabled: p.enabled,
                    credential_count: count,
                }
            })
            .collect();

        ProxyPoolResponse {
            total: pool.len(),
            proxies: pool,
        }
    }

    /// 添加代理到池中
    pub fn add_proxy(
        &self,
        url: String,
        label: Option<String>,
    ) -> Result<ProxyPoolEntry, AdminServiceError> {
        let entry = self
            .proxy_pool
            .add(url, label)
            .map_err(|e| AdminServiceError::InvalidCredential(e.to_string()))?;
        Ok(ProxyPoolEntry {
            id: entry.id,
            url: entry.url,
            label: entry.label,
            enabled: entry.enabled,
            credential_count: 0,
        })
    }

    /// 批量添加代理
    pub fn batch_add_proxies(
        &self,
        req: BatchAddProxyRequest,
    ) -> (Vec<ProxyPoolEntry>, Vec<String>) {
        let (added, errors) = self.proxy_pool.batch_add(req.urls);
        let result = added
            .into_iter()
            .map(|e| ProxyPoolEntry {
                id: e.id,
                url: e.url,
                label: e.label,
                enabled: e.enabled,
                credential_count: 0,
            })
            .collect();
        (result, errors)
    }

    /// 删除代理池中的代理
    pub fn delete_proxy(&self, id: u64) -> Result<(), AdminServiceError> {
        self.proxy_pool.delete(id).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("不存在") {
                AdminServiceError::NotFound { id }
            } else {
                AdminServiceError::InternalError(msg)
            }
        })
    }

    /// 设置代理启用/禁用状态
    pub fn set_proxy_enabled(&self, id: u64, enabled: bool) -> Result<(), AdminServiceError> {
        self.proxy_pool
            .set_enabled(id, enabled)
            .map_err(|_| AdminServiceError::NotFound { id })
    }

    /// 将代理池中的代理分配给指定凭据
    pub fn assign_proxy_to_credential(
        &self,
        credential_id: u64,
        req: AssignProxyRequest,
    ) -> Result<(), AdminServiceError> {
        let proxy_url = match req.proxy_id {
            Some(proxy_id) => {
                let url = match self.proxy_pool.get_url(proxy_id) {
                    GetUrlResult::Ok(url) => url,
                    GetUrlResult::NotFound => {
                        return Err(AdminServiceError::NotFound { id: proxy_id })
                    }
                    GetUrlResult::Disabled => {
                        return Err(AdminServiceError::InvalidCredential(format!(
                            "代理 #{} 已被禁用，请先启用后再分配",
                            proxy_id
                        )))
                    }
                };
                Some(url)
            }
            None => None, // 清除代理
        };

        self.token_manager
            .update_credential(
                credential_id,
                None,              // email 不修改
                Some(proxy_url),   // 设置或清除 proxy_url（Some(None) = 清除，Some(Some(url)) = 设置）
                None,              // proxy_username 不修改
                None,              // proxy_password 不修改
            )
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("不存在") {
                    AdminServiceError::NotFound { id: credential_id }
                } else {
                    AdminServiceError::InternalError(msg)
                }
            })
    }

    // ============ 错误分类 ============

    /// 分类简单操作错误（set_disabled, set_priority, reset_and_enable）
    fn classify_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类余额查询错误（可能涉及上游 API 调用）
    fn classify_balance_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();

        // 1. 凭据不存在
        if msg.contains("不存在") {
            return AdminServiceError::NotFound { id };
        }

        // 2. API Key 凭据不支持刷新：客户端请求错误，映射为 400
        if msg.contains("API Key 凭据不支持刷新") {
            return AdminServiceError::InvalidCredential(msg);
        }

        // 3. 上游服务错误特征：HTTP 响应错误或网络错误
        let is_upstream_error =
            // HTTP 响应错误（来自 refresh_*_token 的错误消息）
            msg.contains("凭证已过期或无效") ||
            msg.contains("权限不足") ||
            msg.contains("已被限流") ||
            msg.contains("服务器错误") ||
            msg.contains("Token 刷新失败") ||
            msg.contains("暂时不可用") ||
            // 网络错误（reqwest 错误格式）
            msg.contains("error sending request") ||
            msg.contains("error trying to connect") ||
            msg.contains("connection") ||
            msg.contains("timeout") ||
            msg.contains("timed out") ||
            msg.contains("proxy") ||
            msg.contains("SOCKS") ||
            msg.contains("dns") ||
            msg.contains("DNS");

        if is_upstream_error {
            AdminServiceError::UpstreamError(msg)
        } else {
            // 4. 默认归类为内部错误（本地验证失败、配置错误等）
            // 包括：缺少 refreshToken、refreshToken 已被截断、无法生成 machineId 等
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类添加凭据错误
    fn classify_add_error(&self, e: anyhow::Error) -> AdminServiceError {
        let msg = e.to_string();

        // 凭据验证失败（refreshToken 无效、格式错误等）
        let is_invalid_credential = msg.contains("缺少 refreshToken")
            || msg.contains("refreshToken 为空")
            || msg.contains("refreshToken 已被截断")
            || msg.contains("凭据已存在")
            || msg.contains("refreshToken 重复")
            || msg.contains("kiroApiKey 重复")
            || msg.contains("缺少 kiroApiKey")
            || msg.contains("kiroApiKey 为空")
            || msg.contains("凭证已过期或无效")
            || msg.contains("权限不足")
            || msg.contains("已被限流");

        if is_invalid_credential {
            AdminServiceError::InvalidCredential(msg)
        } else if msg.contains("error trying to connect")
            || msg.contains("connection")
            || msg.contains("timeout")
        {
            AdminServiceError::UpstreamError(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    // ── Social 登录（Portal PKCE OAuth）────────────────────────────────────────

    /// 发起 Social 登录，返回 portal URL 供用户在浏览器打开
    ///
    /// 模式选择：
    /// - `callback_base_url` 为 Some → 远程模式：redirect_uri 使用服务端公网地址，不启动本地端口
    /// - `callback_base_url` 为 None  → 本地模式：启动本地 TCP 回调服务器（浏览器与服务端须同机）
    pub async fn start_social_login(
        &self,
        req: StartSocialLoginRequest,
    ) -> Result<StartSocialLoginResponse, AdminServiceError> {
        let global_proxy = self.token_manager.proxy();
        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);

        let auth_endpoint = req
            .auth_endpoint
            .unwrap_or_else(|| social::KIRO_AUTH_ENDPOINT.to_string());

        let (code_verifier, code_challenge) = social::generate_pkce();
        let state = uuid::Uuid::new_v4().to_string();

        let (tx, rx) = tokio::sync::oneshot::channel::<social::OAuthCallbackData>();

        // 启动本地 TCP 回调服务器（本地模式）
        // 远程访问时用户须从浏览器地址栏复制回调 URL，通过 complete_social_login 接口手动完成
        let (port, server_handle) = social::start_callback_server(tx)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        let redirect_uri = format!("http://127.0.0.1:{}", port);
        let portal_url = social::build_portal_url(&state, &code_challenge, &redirect_uri);

        let expires_at = Utc::now() + Duration::minutes(10);
        let session_id = uuid::Uuid::new_v4().to_string();

        let cred_template = KiroCredentials {
            auth_method: Some("social".to_string()),
            priority: req.priority,
            email: req.email,
            proxy_url: req.proxy_url,
            ..Default::default()
        };

        let session = SocialAuthSession {
            auth_endpoint,
            state,
            code_verifier,
            redirect_uri,
            expires_at,
            callback_rx: tokio::sync::Mutex::new(rx),
            cred_template,
            proxy,
            _server_handle: server_handle,
            relogin_target_id: None,
        };

        self.social_sessions
            .lock()
            .insert(session_id.clone(), session);

        Ok(StartSocialLoginResponse {
            session_id,
            portal_url,
            expires_at: expires_at.to_rfc3339(),
        })
    }

    /// 轮询一次 Social 登录状态
    pub async fn poll_social_login(
        &self,
        session_id: &str,
    ) -> Result<PollIdcLoginResponse, AdminServiceError> {
        use tokio::sync::oneshot::error::TryRecvError;

        // 一次加锁同时完成：过期检查 + 非阻塞回调接收，消除 TOCTOU
        enum PollOutcome {
            Expired,
            Closed,
            Pending,
            Received(social::OAuthCallbackData),
        }

        let outcome = {
            let sessions = self.social_sessions.lock();
            let Some(session) = sessions.get(session_id) else {
                return Err(AdminServiceError::NotFound { id: 0 });
            };

            if Utc::now() >= session.expires_at {
                PollOutcome::Expired
            } else {
                match session.callback_rx.try_lock() {
                    Ok(mut rx) => match rx.try_recv() {
                        Ok(data) => PollOutcome::Received(data),
                        Err(TryRecvError::Empty) => PollOutcome::Pending,
                        Err(TryRecvError::Closed) => PollOutcome::Closed,
                    },
                    Err(_) => PollOutcome::Pending,
                }
            }
        };

        match outcome {
            PollOutcome::Pending => return Ok(PollIdcLoginResponse::Pending),
            PollOutcome::Expired => {
                self.social_sessions.lock().remove(session_id);
                return Ok(PollIdcLoginResponse::Expired);
            }
            PollOutcome::Closed => {
                self.social_sessions.lock().remove(session_id);
                return Err(AdminServiceError::InternalError(
                    "Social 登录回调服务器已关闭，请重新发起登录".to_string(),
                ));
            }
            PollOutcome::Received(callback) => {
                self.do_complete_social_login(session_id, callback).await
            }
        }
    }

    /// 内部：完成 Social 登录的 token 兑换和凭据创建（供轮询回调和手动完成共用）
    ///
    /// 调用前须确认 session 存在且未过期。会在内部做 state CSRF 校验。
    async fn do_complete_social_login(
        &self,
        session_id: &str,
        callback: social::OAuthCallbackData,
    ) -> Result<PollIdcLoginResponse, AdminServiceError> {
        // 先做 CSRF 校验（不移除 session，校验失败时保持 session 可继续轮询）
        {
            let sessions = self.social_sessions.lock();
            let s = sessions
                .get(session_id)
                .ok_or(AdminServiceError::NotFound { id: 0 })?;
            if callback.state != s.state {
                tracing::warn!(
                    "Social 登录 state 不匹配（期望 {}, 收到 {}），已拒绝",
                    s.state, callback.state
                );
                return Err(AdminServiceError::InternalError(
                    "OAuth state 不匹配，请重新发起登录".to_string(),
                ));
            }
        }

        // 移除 session（含 code_verifier 等敏感数据）
        let session = self
            .social_sessions
            .lock()
            .remove(session_id)
            .ok_or(AdminServiceError::NotFound { id: 0 })?;

        let config = self.token_manager.config();

        // 构建完整的 redirect_uri（与 IDE 行为一致）
        let full_redirect_uri = if callback.login_option.is_empty() {
            format!("{}{}", session.redirect_uri, callback.path)
        } else {
            format!(
                "{}{}?login_option={}",
                session.redirect_uri,
                callback.path,
                urlencoding::encode(&callback.login_option),
            )
        };

        let token = social::exchange_code_for_token(
            &session.auth_endpoint,
            &callback.code,
            &session.code_verifier,
            &full_redirect_uri,
            config,
            session.proxy.as_ref(),
        )
        .await
        .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        // 重新登录模式：更新已有凭据而非创建新凭据
        if let Some(target_id) = session.relogin_target_id {
            let refresh_token = token.refresh_token.ok_or_else(|| {
                AdminServiceError::InternalError(
                    "Social 登录未返回 refreshToken，无法更新凭据".to_string(),
                )
            })?;
            self.do_relogin_update(target_id, refresh_token)
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
            tracing::info!("Social 重新登录成功，凭据 #{} Token 已更新", target_id);
            return Ok(PollIdcLoginResponse::Success { credential_id: target_id });
        }

        let mut new_cred = session.cred_template;
        new_cred.access_token = Some(token.access_token);
        new_cred.refresh_token = token.refresh_token;
        new_cred.expires_at = token.expires_at.or_else(|| {
            token.expires_in.map(|secs| (Utc::now() + Duration::seconds(secs)).to_rfc3339())
        });
        if let Some(arn) = token.profile_arn {
            new_cred.profile_arn = Some(arn);
        }

        let credential_id = self
            .token_manager
            .add_credential(new_cred)
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        tracing::info!("Social 登录成功，已添加凭据 #{}", credential_id);
        Ok(PollIdcLoginResponse::Success { credential_id })
    }

    /// 手动完成 Social 登录：远程访问时从浏览器地址栏粘贴的回调 URL 中提取参数，直接完成 token 兑换
    pub async fn complete_social_login(
        &self,
        session_id: &str,
        code: String,
        state: String,
        login_option: String,
        path: String,
    ) -> Result<PollIdcLoginResponse, AdminServiceError> {
        // 过期检查
        {
            let sessions = self.social_sessions.lock();
            let s = sessions
                .get(session_id)
                .ok_or(AdminServiceError::NotFound { id: 0 })?;
            if Utc::now() >= s.expires_at {
                return Ok(PollIdcLoginResponse::Expired);
            }
        }

        let callback = social::OAuthCallbackData { code, login_option, path, state };
        self.do_complete_social_login(session_id, callback).await
    }

    /// 分类删除凭据错误
    fn classify_delete_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else if msg.contains("只能删除已禁用的凭据") || msg.contains("请先禁用凭据") {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    // ── IdC 设备授权登录 ──────────────────────────────────────────────────────

    /// 发起 IdC 设备授权，返回验证码和 URL
    pub async fn start_idc_login(
        &self,
        req: StartIdcLoginRequest,
    ) -> Result<StartIdcLoginResponse, AdminServiceError> {
        let config = self.token_manager.config();
        let global_proxy = self.token_manager.proxy();

        // 代理：优先用请求级，否则回退全局
        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);

        let start_url = req
            .start_url
            .as_deref()
            .unwrap_or(BUILDER_ID_START_URL);

        // 1. 注册 OIDC 客户端
        let reg = idc::register_client(&req.region, config, proxy.as_ref())
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        // 2. 发起设备授权
        let device = idc::start_device_authorization(
            &req.region,
            start_url,
            &reg.client_id,
            &reg.client_secret,
            config,
            proxy.as_ref(),
        )
        .await
        .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        let expires_at = Utc::now() + Duration::seconds(device.expires_in);
        let session_id = Uuid::new_v4().to_string();

        // 构建登录成功后写入的凭据模板
        let cred_template = KiroCredentials {
            auth_method: Some("idc".to_string()),
            client_id: Some(reg.client_id.clone()),
            client_secret: Some(reg.client_secret.clone()),
            region: Some(req.region.clone()),
            priority: req.priority,
            email: req.email,
            proxy_url: req.proxy_url,
            ..Default::default()
        };

        let session = IdcAuthSession {
            region: req.region,
            client_id: reg.client_id,
            client_secret: reg.client_secret,
            device_code: device.device_code,
            expires_at,
            poll_interval: device.interval.max(5),
            cred_template,
            proxy,
            relogin_target_id: None,
        };

        let poll_interval = session.poll_interval;
        self.idc_sessions.lock().insert(session_id.clone(), session);

        Ok(StartIdcLoginResponse {
            session_id,
            user_code: device.user_code,
            verification_uri: device.verification_uri,
            verification_uri_complete: device.verification_uri_complete,
            expires_at: expires_at.to_rfc3339(),
            poll_interval,
        })
    }

    /// 轮询一次 IdC 登录状态
    pub async fn poll_idc_login(
        &self,
        session_id: &str,
    ) -> Result<PollIdcLoginResponse, AdminServiceError> {
        let (region, client_id, client_secret, device_code, _expires_at, proxy, cred_template, relogin_target_id) = {
            let sessions = self.idc_sessions.lock();
            let s = sessions
                .get(session_id)
                .ok_or_else(|| AdminServiceError::NotFound { id: 0 })?;

            if Utc::now() >= s.expires_at {
                return Ok(PollIdcLoginResponse::Expired);
            }

            (
                s.region.clone(),
                s.client_id.clone(),
                s.client_secret.clone(),
                s.device_code.clone(),
                s.expires_at,
                s.proxy.clone(),
                s.cred_template.clone(),
                s.relogin_target_id,
            )
        };

        let config = self.token_manager.config();

        match idc::poll_token(
            &region, &client_id, &client_secret, &device_code, config, proxy.as_ref(),
        )
        .await
        {
            idc::PollResult::Pending => Ok(PollIdcLoginResponse::Pending),
            idc::PollResult::Expired => {
                self.idc_sessions.lock().remove(session_id);
                Ok(PollIdcLoginResponse::Expired)
            }
            idc::PollResult::Error(e) => Err(AdminServiceError::InternalError(e.to_string())),
            idc::PollResult::Success(token) => {
                self.idc_sessions.lock().remove(session_id);

                // 重新登录模式：更新已有凭据而非创建新凭据
                if let Some(target_id) = relogin_target_id {
                    if let Some(refresh_token) = token.refresh_token {
                        self.do_relogin_update(target_id, refresh_token)
                            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
                    }
                    tracing::info!("IdC 重新登录成功，凭据 #{} Token 已更新", target_id);
                    return Ok(PollIdcLoginResponse::Success { credential_id: target_id });
                }

                // 写入凭据
                let mut new_cred = cred_template;
                new_cred.access_token = Some(token.access_token);
                new_cred.refresh_token = token.refresh_token;
                if let Some(secs) = token.expires_in {
                    new_cred.expires_at =
                        Some((Utc::now() + Duration::seconds(secs)).to_rfc3339());
                }

                let credential_id = self
                    .token_manager
                    .add_credential(new_cred)
                    .await
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

                tracing::info!("IdC 设备授权登录成功，已添加凭据 #{}", credential_id);
                Ok(PollIdcLoginResponse::Success { credential_id })
            }
        }
    }

    /// 内部：重新登录完成后更新已有凭据的 Token（禁用→更新→重置→启用）
    fn do_relogin_update(&self, target_id: u64, refresh_token: String) -> anyhow::Result<()> {
        // 先禁用（update_refresh_token 要求凭据处于禁用状态）
        self.token_manager.set_disabled(target_id, true)?;
        // 更新 refreshToken（同时清空 accessToken 和 expiresAt，系统会在下次使用时自动刷新）
        self.token_manager.update_refresh_token(target_id, refresh_token, None, None)?;
        // 重置失败计数并重新启用
        self.token_manager.reset_and_enable(target_id)?;
        Ok(())
    }

    /// 发起 Social 重新登录（更新已有凭据的 Token 而非创建新凭据）
    pub async fn start_social_relogin(
        &self,
        target_id: u64,
        req: StartSocialLoginRequest,
    ) -> Result<StartSocialLoginResponse, AdminServiceError> {
        // 验证目标凭据存在
        {
            let snapshot = self.token_manager.snapshot();
            if !snapshot.entries.iter().any(|e| e.id == target_id) {
                return Err(AdminServiceError::NotFound { id: target_id });
            }
        }

        let global_proxy = self.token_manager.proxy();
        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);

        let auth_endpoint = req
            .auth_endpoint
            .unwrap_or_else(|| social::KIRO_AUTH_ENDPOINT.to_string());

        let (code_verifier, code_challenge) = social::generate_pkce();
        let state = uuid::Uuid::new_v4().to_string();

        let (tx, rx) = tokio::sync::oneshot::channel::<social::OAuthCallbackData>();

        let (port, server_handle) = social::start_callback_server(tx)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        let redirect_uri = format!("http://127.0.0.1:{}", port);
        let portal_url = social::build_portal_url(&state, &code_challenge, &redirect_uri);

        let expires_at = Utc::now() + Duration::minutes(10);
        let session_id = uuid::Uuid::new_v4().to_string();

        let session = SocialAuthSession {
            auth_endpoint,
            state,
            code_verifier,
            redirect_uri,
            expires_at,
            callback_rx: tokio::sync::Mutex::new(rx),
            cred_template: KiroCredentials::default(),
            proxy,
            _server_handle: server_handle,
            relogin_target_id: Some(target_id),
        };

        self.social_sessions
            .lock()
            .insert(session_id.clone(), session);

        Ok(StartSocialLoginResponse {
            session_id,
            portal_url,
            expires_at: expires_at.to_rfc3339(),
        })
    }

    /// 发起 IdC 重新登录（更新已有凭据的 Token 而非创建新凭据）
    pub async fn start_idc_relogin(
        &self,
        target_id: u64,
        req: StartIdcLoginRequest,
    ) -> Result<StartIdcLoginResponse, AdminServiceError> {
        // 验证目标凭据存在
        {
            let snapshot = self.token_manager.snapshot();
            if !snapshot.entries.iter().any(|e| e.id == target_id) {
                return Err(AdminServiceError::NotFound { id: target_id });
            }
        }

        let config = self.token_manager.config();
        let global_proxy = self.token_manager.proxy();

        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);

        let start_url = req
            .start_url
            .as_deref()
            .unwrap_or(BUILDER_ID_START_URL);

        let reg = idc::register_client(&req.region, config, proxy.as_ref())
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        let device = idc::start_device_authorization(
            &req.region,
            start_url,
            &reg.client_id,
            &reg.client_secret,
            config,
            proxy.as_ref(),
        )
        .await
        .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        let expires_at = Utc::now() + Duration::seconds(device.expires_in);
        let session_id = Uuid::new_v4().to_string();

        let session = IdcAuthSession {
            region: req.region,
            client_id: reg.client_id,
            client_secret: reg.client_secret,
            device_code: device.device_code,
            expires_at,
            poll_interval: device.interval.max(5),
            cred_template: KiroCredentials::default(),
            proxy,
            relogin_target_id: Some(target_id),
        };

        let poll_interval = session.poll_interval;
        self.idc_sessions.lock().insert(session_id.clone(), session);

        Ok(StartIdcLoginResponse {
            session_id,
            user_code: device.user_code,
            verification_uri: device.verification_uri,
            verification_uri_complete: device.verification_uri_complete,
            expires_at: expires_at.to_rfc3339(),
            poll_interval,
        })
    }
}
