//! Admin API 业务逻辑服务

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::anthropic::{available_models, types::ModelsResponse};
use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;
use crate::monitoring::{RequestActivitySnapshot, RequestMonitor};

use super::error::AdminServiceError;
use super::types::{
    AddApiKeyRequest, AddCredentialRequest, AddCredentialResponse, ApiKeyInfo,
    ApiKeysListResponse, BalanceResponse, CredentialStatusItem, CredentialsStatusResponse,
    GenerateApiKeyRequest, GenerateApiKeyResponse, LoadBalancingModeResponse, LogsResponse,
    ProxyPoolDto, ProxyPoolStatusResponse, ProxyPoolTemplateDto, ProxyTestItem,
    ProxyTestResponse, SetLoadBalancingModeRequest, TestProxyPoolRequest,
    UpdateApiKeyRequest, UpdateCredentialProxyRequest,
};
use crate::model::config::{
    ApiKeyConfig, Config, ProxyPoolConfig, ProxyPoolTemplate,
};
use std::time::{Duration, Instant};

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
    request_monitor: RequestMonitor,
    balance_cache: Mutex<HashMap<u64, CachedBalance>>,
    cache_path: Option<PathBuf>,
    log_path: PathBuf,
    config_path: PathBuf,
    /// 已注册的端点名称集合（用于 add_credential 校验）
    known_endpoints: HashSet<String>,
}

impl AdminService {
    pub fn new(
        token_manager: Arc<MultiTokenManager>,
        request_monitor: RequestMonitor,
        log_path: PathBuf,
        config_path: PathBuf,
        known_endpoints: impl IntoIterator<Item = String>,
    ) -> Self {
        let cache_path = token_manager
            .cache_dir()
            .map(|d| d.join("kiro_balance_cache.json"));

        let balance_cache = Self::load_balance_cache_from(&cache_path);

        Self {
            token_manager,
            request_monitor,
            balance_cache: Mutex::new(balance_cache),
            cache_path,
            log_path,
            config_path,
            known_endpoints: known_endpoints.into_iter().collect(),
        }
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
                proxy_url: entry.proxy_url.as_deref().map(mask_proxy_url),
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

    /// 获取负载均衡模式
    pub fn get_load_balancing_mode(&self) -> LoadBalancingModeResponse {
        LoadBalancingModeResponse {
            mode: self.token_manager.get_load_balancing_mode(),
        }
    }

    /// 获取当前服务暴露的模型列表
    pub fn get_available_models(&self) -> ModelsResponse {
        ModelsResponse {
            object: "list".to_string(),
            data: available_models(),
        }
    }

    /// 获取最近请求活动
    pub fn get_request_activity(&self, limit: usize) -> RequestActivitySnapshot {
        self.request_monitor.snapshot(limit)
    }

    /// 获取最近日志
    pub fn get_recent_logs(&self, lines: usize) -> LogsResponse {
        let lines = lines.clamp(1, 500);

        match read_tail_lines(&self.log_path, lines) {
            Ok((lines, truncated)) => LogsResponse {
                path: self.log_path.display().to_string(),
                available: true,
                fetched_at: Utc::now().to_rfc3339(),
                truncated,
                lines,
                error: None,
            },
            Err(error) => LogsResponse {
                path: self.log_path.display().to_string(),
                available: false,
                fetched_at: Utc::now().to_rfc3339(),
                truncated: false,
                lines: Vec::new(),
                error: Some(error),
            },
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
            // 网络错误（reqwest 错误）
            msg.contains("error trying to connect") ||
            msg.contains("connection") ||
            msg.contains("timeout") ||
            msg.contains("timed out");

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

    /// 分类删除凭据错误
    fn classify_delete_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else if msg.contains("只能删除已禁用的凭据") || msg.contains("请先禁用凭据")
        {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }
}

const LOG_TAIL_BYTES: u64 = 128 * 1024;

fn read_tail_lines(path: &Path, max_lines: usize) -> Result<(Vec<String>, bool), String> {
    let mut file = File::open(path).map_err(|e| format!("打开日志文件失败: {}", e))?;
    let file_size = file
        .metadata()
        .map_err(|e| format!("读取日志文件信息失败: {}", e))?
        .len();

    let start_offset = file_size.saturating_sub(LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(start_offset))
        .map_err(|e| format!("定位日志文件失败: {}", e))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|e| format!("读取日志文件失败: {}", e))?;

    // 从任意字节偏移读取尾部时，截断点可能落在 UTF-8 多字节字符中间。
    // 这里使用 lossy 解码，并在 start_offset > 0 时丢弃首个残缺行，避免整段读取失败。
    let buffer = String::from_utf8_lossy(&buffer);
    let mut lines: Vec<String> = buffer.lines().map(strip_ansi_codes).collect();

    if start_offset > 0 && !lines.is_empty() {
        lines.remove(0);
    }

    let truncated = lines.len() > max_lines;
    if truncated {
        lines = lines[lines.len() - max_lines..].to_vec();
    }

    Ok((lines, truncated))
}

fn strip_ansi_codes(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut in_escape = false;

    for ch in line.chars() {
        if in_escape {
            if matches!(ch, 'm' | 'K') {
                in_escape = false;
            }
            continue;
        }

        if ch == '\u{1b}' {
            in_escape = true;
            continue;
        }

        result.push(ch);
    }

    result
}

impl AdminService {
    // ============ API Key 管理方法 ============

    /// 获取所有 API Keys
    pub fn get_api_keys(&self) -> Result<ApiKeysListResponse, AdminServiceError> {
        let config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        let api_keys: Vec<ApiKeyInfo> = config
            .api_keys
            .iter()
            .map(|k| ApiKeyInfo {
                id: k.id.clone(),
                key: k.key.clone(),
                name: k.name.clone(),
                enabled: k.enabled,
                created_at: k.created_at.clone(),
                last_used_at: k.last_used_at.clone(),
                is_primary: false,
            })
            .collect();

        // 处理主Key（来自旧配置）
        let primary_key = config.api_key.as_ref().and_then(|key| {
            if key.trim().is_empty() {
                None
            } else {
                Some(ApiKeyInfo {
                    id: "primary".to_string(),
                    key: key.clone(),
                    name: "主Key（来自配置）".to_string(),
                    enabled: true,
                    created_at: "N/A".to_string(),
                    last_used_at: None,
                    is_primary: true,
                })
            }
        });

        Ok(ApiKeysListResponse {
            api_keys,
            primary_key,
        })
    }

    /// 添加新的 API Key
    pub fn add_api_key(
        &self,
        req: AddApiKeyRequest,
    ) -> Result<ApiKeyInfo, AdminServiceError> {
        let key = req.key.trim();
        if key.is_empty() {
            return Err(AdminServiceError::InvalidRequest("Key 不能为空".to_string()));
        }

        let mut config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        // 检查Key是否重复
        if config.api_key.as_ref().map(|k| k == key).unwrap_or(false) {
            return Err(AdminServiceError::InvalidRequest(
                "Key 与主Key重复".to_string(),
            ));
        }
        if config.api_keys.iter().any(|k| k.key == key) {
            return Err(AdminServiceError::InvalidRequest("Key 已存在".to_string()));
        }

        let new_key = ApiKeyConfig {
            id: uuid::Uuid::new_v4().to_string(),
            key: key.to_string(),
            name: req.name.trim().to_string(),
            enabled: true,
            created_at: Utc::now().to_rfc3339(),
            last_used_at: None,
        };

        let key_info = ApiKeyInfo {
            id: new_key.id.clone(),
            key: new_key.key.clone(),
            name: new_key.name.clone(),
            enabled: new_key.enabled,
            created_at: new_key.created_at.clone(),
            last_used_at: None,
            is_primary: false,
        };

        config.api_keys.push(new_key);
        config
            .save()
            .map_err(|e| AdminServiceError::InternalError(format!("保存配置失败: {}", e)))?;

        Ok(key_info)
    }

    /// 生成随机 API Key
    pub fn generate_api_key(
        &self,
        req: GenerateApiKeyRequest,
    ) -> Result<GenerateApiKeyResponse, AdminServiceError> {
        use rand::{thread_rng, Rng};
        use rand::distributions::Alphanumeric;

        let length = req.length.clamp(16, 64);
        let key: String = thread_rng()
            .sample_iter(&Alphanumeric)
            .take(length)
            .map(char::from)
            .collect();

        let mut config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        let new_key = ApiKeyConfig {
            id: uuid::Uuid::new_v4().to_string(),
            key: key.clone(),
            name: req.name.trim().to_string(),
            enabled: true,
            created_at: Utc::now().to_rfc3339(),
            last_used_at: None,
        };

        let id = new_key.id.clone();
        config.api_keys.push(new_key);
        config
            .save()
            .map_err(|e| AdminServiceError::InternalError(format!("保存配置失败: {}", e)))?;

        Ok(GenerateApiKeyResponse { key, id })
    }

    /// 更新 API Key
    pub fn update_api_key(
        &self,
        id: &str,
        req: UpdateApiKeyRequest,
    ) -> Result<(), AdminServiceError> {
        let mut config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        // 先找到Key的索引
        let key_index = config
            .api_keys
            .iter()
            .position(|k| k.id == id)
            .ok_or_else(|| AdminServiceError::NotFoundGeneric(format!("Key {} 不存在", id)))?;

        // 如果要禁用，检查是否至少保留一个启用的Key
        if let Some(false) = req.enabled {
            if config.api_keys[key_index].enabled {
                let enabled_count = config
                    .api_keys
                    .iter()
                    .filter(|k| k.enabled && k.id != id)
                    .count();
                let has_primary = config
                    .api_key
                    .as_ref()
                    .map(|k| !k.trim().is_empty())
                    .unwrap_or(false);

                if enabled_count == 0 && !has_primary {
                    return Err(AdminServiceError::InvalidRequest(
                        "至少需要保留一个启用的 Key".to_string(),
                    ));
                }
            }
        }

        // 更新Key
        let key = &mut config.api_keys[key_index];
        if let Some(name) = req.name {
            key.name = name.trim().to_string();
        }
        if let Some(enabled) = req.enabled {
            key.enabled = enabled;
        }

        config
            .save()
            .map_err(|e| AdminServiceError::InternalError(format!("保存配置失败: {}", e)))?;

        Ok(())
    }

    // ============ 代理池管理 ============

    /// 读取当前代理池配置（从 config.json）
    ///
    /// 注意：返回值会 mask password，避免通过 GET 接口泄露密码
    pub fn get_proxy_pool(&self) -> Result<ProxyPoolStatusResponse, AdminServiceError> {
        let config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        let mut dto = match &config.proxy_pool {
            Some(pool) => pool_config_to_dto(pool),
            None => ProxyPoolDto::default(),
        };
        // 屏蔽 password，GET 返回给前端时不暴露明文
        if dto.password.is_some() {
            dto.password = Some("***".to_string());
        }

        let snapshot = self.token_manager.proxy_pool_snapshot();
        let (proxies, resolved_urls, size, default_cooldown_secs) = match snapshot.as_ref() {
            Some(pool) => {
                let entries = pool.entries();
                let cooldowns = pool.cooldown_snapshot();
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                let mut urls = Vec::with_capacity(entries.len());
                let mut items = Vec::with_capacity(entries.len());
                for (i, entry) in entries.iter().enumerate() {
                    let masked = mask_proxy_url(&entry.url);
                    urls.push(masked.clone());
                    let until = cooldowns.get(i).copied().unwrap_or(0);
                    let remaining = if until > now_ms {
                        (until - now_ms).div_ceil(1000)
                    } else {
                        0
                    };
                    items.push(crate::admin::types::ProxyPoolItemStatus {
                        url: masked,
                        cooldown_until_ms: until,
                        cooldown_remaining_secs: remaining,
                    });
                }
                (items, urls, pool.len(), pool.default_cooldown().as_secs())
            }
            None => (
                Vec::new(),
                Vec::new(),
                0,
                crate::model::config::ProxyPoolConfig::DEFAULT_COOLDOWN_SECS,
            ),
        };

        let active = snapshot.is_some();
        let server_time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(ProxyPoolStatusResponse {
            config: dto,
            proxies,
            resolved_urls,
            size,
            active,
            server_time_ms,
            default_cooldown_secs,
        })
    }

    /// 更新代理池配置（持久化 + 热更新运行时池）
    pub fn update_proxy_pool(
        &self,
        mut dto: ProxyPoolDto,
    ) -> Result<ProxyPoolStatusResponse, AdminServiceError> {
        let strategy = dto.strategy.trim();
        let valid_strategies = ["round-robin", "random", "per-credential"];
        if !valid_strategies.contains(&strategy) {
            return Err(AdminServiceError::InvalidRequest(
                "strategy 必须是 round-robin / random / per-credential 之一".to_string(),
            ));
        }

        // 加载现有配置
        let mut config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        // 如果 password 字段为 "***"（占位符），保留原密码；其他情况按传入值覆盖
        if matches!(dto.password.as_deref(), Some("***")) {
            dto.password = config
                .proxy_pool
                .as_ref()
                .and_then(|p| p.password.clone());
        }

        let pool_cfg = dto_to_pool_config(&dto);

        config.proxy_pool = Some(pool_cfg.clone());
        config
            .save()
            .map_err(|e| AdminServiceError::InternalError(format!("保存配置失败: {}", e)))?;

        // 重建运行时代理池
        let new_pool = config.build_proxy_pool().map(std::sync::Arc::new);
        self.token_manager.replace_proxy_pool(new_pool.clone());

        if let Some(pool) = new_pool.as_ref() {
            tracing::info!(
                "代理池已更新：共 {} 个代理，策略 {}",
                pool.len(),
                pool.strategy().as_str()
            );
        } else {
            tracing::info!("代理池已关闭");
        }

        self.get_proxy_pool()
    }

    /// 测试代理池连通性
    pub async fn test_proxy_pool(
        &self,
        req: TestProxyPoolRequest,
    ) -> Result<ProxyTestResponse, AdminServiceError> {
        let config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        let test_url_override = req
            .test_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let test_url = test_url_override.unwrap_or_else(|| {
            config
                .proxy_pool
                .as_ref()
                .map(|p| p.effective_test_url().to_string())
                .unwrap_or_else(|| ProxyPoolConfig::DEFAULT_TEST_URL.to_string())
        });

        let timeout = Duration::from_secs(req.timeout_secs.unwrap_or(10).clamp(3, 60));

        let pool = self.token_manager.proxy_pool_snapshot();
        let entries: Vec<ProxyConfig> = pool
            .as_ref()
            .map(|p| p.entries().to_vec())
            .unwrap_or_default();

        if entries.is_empty() {
            return Ok(ProxyTestResponse {
                total: 0,
                success: 0,
                failed: 0,
                test_url,
                results: Vec::new(),
            });
        }

        let tls_backend = config.tls_backend;
        let mut handles = Vec::with_capacity(entries.len());
        for entry in entries {
            let url_owned = test_url.clone();
            let entry_clone = entry.clone();
            let handle = tokio::spawn(async move {
                test_single_proxy(&entry_clone, &url_owned, timeout, tls_backend).await
            });
            handles.push(handle);
        }

        let mut results = Vec::with_capacity(handles.len());
        let mut ok = 0usize;
        for handle in handles {
            match handle.await {
                Ok(item) => {
                    if item.success {
                        ok += 1;
                    }
                    results.push(item);
                }
                Err(e) => {
                    results.push(ProxyTestItem {
                        url: "<unknown>".to_string(),
                        success: false,
                        duration_ms: 0,
                        response_ip: None,
                        error: Some(format!("任务失败: {}", e)),
                    });
                }
            }
        }

        let total = results.len();
        let failed = total - ok;

        Ok(ProxyTestResponse {
            total,
            success: ok,
            failed,
            test_url,
            results,
        })
    }

    /// 更新凭据级代理配置
    pub async fn update_credential_proxy(
        &self,
        id: u64,
        req: UpdateCredentialProxyRequest,
    ) -> Result<(), AdminServiceError> {
        self.token_manager
            .update_credential_proxy(id, req.proxy_url, req.proxy_username, req.proxy_password)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 删除 API Key
    pub fn delete_api_key(&self, id: &str) -> Result<(), AdminServiceError> {
        let mut config = Config::load(&self.config_path)
            .map_err(|e| AdminServiceError::InternalError(format!("加载配置失败: {}", e)))?;

        let index = config
            .api_keys
            .iter()
            .position(|k| k.id == id)
            .ok_or_else(|| AdminServiceError::NotFoundGeneric(format!("Key {} 不存在", id)))?;

        // 检查是否至少保留一个启用的Key
        let key_to_delete = &config.api_keys[index];
        if key_to_delete.enabled {
            let enabled_count = config.api_keys.iter().filter(|k| k.enabled && k.id != id).count();
            let has_primary = config.api_key.as_ref().map(|k| !k.trim().is_empty()).unwrap_or(false);

            if enabled_count == 0 && !has_primary {
                return Err(AdminServiceError::InvalidRequest(
                    "至少需要保留一个启用的 Key".to_string(),
                ));
            }
        }

        config.api_keys.remove(index);
        config
            .save()
            .map_err(|e| AdminServiceError::InternalError(format!("保存配置失败: {}", e)))?;

        Ok(())
    }
}

// ============ 代理池辅助函数 ============

/// DTO -> Config
fn dto_to_pool_config(dto: &ProxyPoolDto) -> ProxyPoolConfig {
    let cleaned: Vec<String> = dto
        .urls
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let urls = if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    };

    let template = dto.template.as_ref().and_then(|t| {
        let host = t.host.trim();
        if host.is_empty() {
            return None;
        }
        Some(ProxyPoolTemplate {
            protocol: t.protocol.trim().to_string(),
            host: host.to_string(),
            port_start: t.port_start,
            port_end: t.port_end,
        })
    });

    let username = dto
        .username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let password = dto
        .password
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let test_url = dto
        .test_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    ProxyPoolConfig {
        enabled: dto.enabled,
        strategy: dto.strategy.trim().to_string(),
        urls,
        template,
        username,
        password,
        test_url,
        cooldown_secs: dto.cooldown_secs,
    }
}

/// Config -> DTO
fn pool_config_to_dto(cfg: &ProxyPoolConfig) -> ProxyPoolDto {
    ProxyPoolDto {
        enabled: cfg.enabled,
        strategy: cfg.strategy.clone(),
        urls: cfg.urls.clone(),
        template: cfg.template.as_ref().map(|t| ProxyPoolTemplateDto {
            protocol: t.protocol.clone(),
            host: t.host.clone(),
            port_start: t.port_start,
            port_end: t.port_end,
        }),
        username: cfg.username.clone(),
        password: cfg.password.clone(),
        test_url: cfg.test_url.clone(),
        cooldown_secs: cfg.cooldown_secs,
    }
}

/// 测试单个代理
async fn test_single_proxy(
    entry: &ProxyConfig,
    test_url: &str,
    timeout: Duration,
    tls_backend: crate::model::config::TlsBackend,
) -> ProxyTestItem {
    let start = Instant::now();
    let timeout_secs = timeout.as_secs().max(1);
    let client = match build_client(Some(entry), timeout_secs, tls_backend) {
        Ok(c) => c,
        Err(e) => {
            return ProxyTestItem {
                url: entry.url.clone(),
                success: false,
                duration_ms: start.elapsed().as_millis() as u64,
                response_ip: None,
                error: Some(format!("构建客户端失败: {}", e)),
            };
        }
    };

    let masked_url = mask_proxy_url(&entry.url);
    match tokio::time::timeout(timeout, client.get(test_url).send()).await {
        Ok(Ok(resp)) => {
            let status = resp.status();
            if !status.is_success() {
                return ProxyTestItem {
                    url: masked_url,
                    success: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    response_ip: None,
                    error: Some(format!("HTTP 状态码: {}", status.as_u16())),
                };
            }
            let body = resp.text().await.unwrap_or_default();
            let ip = extract_ip_from_json(&body);
            ProxyTestItem {
                url: masked_url,
                success: true,
                duration_ms: start.elapsed().as_millis() as u64,
                response_ip: ip,
                error: None,
            }
        }
        Ok(Err(e)) => ProxyTestItem {
            url: masked_url,
            success: false,
            duration_ms: start.elapsed().as_millis() as u64,
            response_ip: None,
            error: Some(e.to_string()),
        },
        Err(_) => ProxyTestItem {
            url: masked_url,
            success: false,
            duration_ms: start.elapsed().as_millis() as u64,
            response_ip: None,
            error: Some(format!("超时（{}s）", timeout.as_secs())),
        },
    }
}

/// 屏蔽代理 URL 中的敏感信息（用户名保留，密码 mask）
fn mask_proxy_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let prefix = &url[..scheme_end + 3];
        let rest = &url[scheme_end + 3..];
        if let Some(at_pos) = rest.find('@') {
            let auth = &rest[..at_pos];
            let host = &rest[at_pos + 1..];
            // 保留用户名，屏蔽密码
            if let Some(colon) = auth.find(':') {
                let user = &auth[..colon];
                return format!("{}{}:***@{}", prefix, user, host);
            }
            return format!("{}***@{}", prefix, host);
        }
    }
    url.to_string()
}

/// 从响应 JSON 中尽力提取 IP 字段
fn extract_ip_from_json(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    // 常见字段名
    for key in ["ip", "query", "origin", "YourFuckingIPAddress"] {
        if let Some(v) = value.get(key) {
            if let Some(s) = v.as_str() {
                return Some(s.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{LOG_TAIL_BYTES, read_tail_lines};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_log_path(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.log", std::process::id()))
    }

    #[test]
    fn read_tail_lines_handles_utf8_boundary_in_tail_window() {
        let path = temp_log_path("kiro-admin-log-tail");
        let tail = (0..200)
            .map(|i| format!("2026-04-05 INFO 第{i} 行日志\n"))
            .collect::<String>();

        let mut content = "中".as_bytes().to_vec();
        let padding_len = (LOG_TAIL_BYTES as usize + 1)
            .checked_sub(content.len() + 1 + tail.len())
            .expect("tail payload should fit in the fixed test window");

        content.extend(std::iter::repeat_n(b'a', padding_len));
        content.push(b'\n');
        content.extend_from_slice(tail.as_bytes());

        fs::write(&path, &content).expect("test log file should be writable");

        let (lines, truncated) = read_tail_lines(&path, 10).expect("tail read should succeed");

        assert!(truncated);
        assert_eq!(lines.len(), 10);
        assert_eq!(
            lines.last().expect("should keep the last log line"),
            "2026-04-05 INFO 第199 行日志"
        );

        fs::remove_file(path).expect("test log file should be removable");
    }
}
