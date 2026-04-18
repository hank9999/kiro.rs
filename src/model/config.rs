use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TlsBackend {
    Rustls,
    NativeTls,
}

impl Default for TlsBackend {
    fn default() -> Self {
        Self::Rustls
    }
}

/// API Key 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyConfig {
    pub id: String,
    pub key: String,
    pub name: String,
    pub enabled: bool,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
}

/// 代理池端口范围模板
///
/// 用于自动展开 `protocol://host:portStart`..`protocol://host:portEnd` 多个代理
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolTemplate {
    /// 代理协议，例如 "socks5h" / "socks5" / "http" / "https"
    #[serde(default = "default_pool_protocol")]
    pub protocol: String,
    /// 主机名
    pub host: String,
    /// 起始端口（含）
    pub port_start: u16,
    /// 结束端口（含）
    pub port_end: u16,
}

fn default_pool_protocol() -> String {
    "socks5h".to_string()
}

/// 代理池配置
///
/// 支持两种模式：
/// - `urls`：直接指定 URL 列表
/// - `template`：端口范围模板，自动展开成多个代理
///
/// 两种模式可同时存在，合并后去重
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolConfig {
    /// 是否启用代理池（关闭时等价于不存在）
    #[serde(default)]
    pub enabled: bool,

    /// 选择策略："round-robin" / "random" / "per-credential"
    #[serde(default = "default_pool_strategy")]
    pub strategy: String,

    /// 直接指定的代理 URL 列表（可选）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub urls: Option<Vec<String>>,

    /// 端口范围模板（可选）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<ProxyPoolTemplate>,

    /// 模板模式下的全局认证用户名（可选）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// 模板模式下的全局认证密码（可选）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// 连通性测试目标 URL（可选，默认 `https://ip.decodo.com/json`）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_url: Option<String>,

    /// 代理被标记为限流后的冷却时长（秒，默认 30）
    ///
    /// 冷却期内 `pick()` 会跳过该代理，冷却结束后自动恢复
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_secs: Option<u64>,
}

fn default_pool_strategy() -> String {
    "round-robin".to_string()
}

impl ProxyPoolConfig {
    /// 默认测试地址
    pub const DEFAULT_TEST_URL: &'static str = "https://ip.decodo.com/json";

    /// 默认代理限流冷却时长（秒）
    pub const DEFAULT_COOLDOWN_SECS: u64 = 30;

    /// 获取有效测试地址
    pub fn effective_test_url(&self) -> &str {
        self.test_url
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(Self::DEFAULT_TEST_URL)
    }

    /// 获取有效冷却时长（秒），最小 1 秒
    pub fn effective_cooldown_secs(&self) -> u64 {
        self.cooldown_secs
            .filter(|v| *v > 0)
            .unwrap_or(Self::DEFAULT_COOLDOWN_SECS)
    }
}

/// KNA 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_region")]
    pub region: String,

    /// Auth Region（用于 Token 刷新），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_region: Option<String>,

    /// API Region（用于 API 请求），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,

    #[serde(default = "default_kiro_version")]
    pub kiro_version: String,

    #[serde(default)]
    pub machine_id: Option<String>,

    #[serde(default)]
    pub api_key: Option<String>,

    /// API Keys 列表（新增，可选）
    #[serde(default)]
    pub api_keys: Vec<ApiKeyConfig>,

    #[serde(default = "default_system_version")]
    pub system_version: String,

    #[serde(default = "default_node_version")]
    pub node_version: String,

    #[serde(default = "default_tls_backend")]
    pub tls_backend: TlsBackend,

    /// 外部 count_tokens API 地址（可选）
    #[serde(default)]
    pub count_tokens_api_url: Option<String>,

    /// count_tokens API 密钥（可选）
    #[serde(default)]
    pub count_tokens_api_key: Option<String>,

    /// count_tokens API 认证类型（可选，"x-api-key" 或 "bearer"，默认 "x-api-key"）
    #[serde(default = "default_count_tokens_auth_type")]
    pub count_tokens_auth_type: String,

    /// HTTP 代理地址（可选）
    /// 支持格式: http://host:port, https://host:port, socks5://host:port
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// 代理认证用户名（可选）
    #[serde(default)]
    pub proxy_username: Option<String>,

    /// 代理认证密码（可选）
    #[serde(default)]
    pub proxy_password: Option<String>,

    /// 代理池配置（可选，启用后用于 IP 轮询，避免限流）
    ///
    /// 优先级：凭据级代理 > 代理池 > 全局单代理 > 无代理
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_pool: Option<ProxyPoolConfig>,

    /// Admin API 密钥（可选，启用 Admin API 功能）
    #[serde(default)]
    pub admin_api_key: Option<String>,

    /// 负载均衡模式（"priority" 或 "balanced"）
    #[serde(default = "default_load_balancing_mode")]
    pub load_balancing_mode: String,

    /// 是否开启非流式响应的 thinking 块提取（默认 true）
    ///
    /// 启用后，非流式响应中的 `<thinking>...</thinking>` 标签会被解析为
    /// 独立的 `{"type": "thinking", ...}` 内容块,与流式响应行为一致。
    #[serde(default = "default_extract_thinking")]
    pub extract_thinking: bool,

    /// 默认端点名称（凭据未显式指定 endpoint 时使用，默认 "ide"）
    #[serde(default = "default_endpoint")]
    pub default_endpoint: String,

    /// 端点特定的配置
    ///
    /// 键为端点名（如 "ide" / "cli"），值为该端点自由定义的参数对象。
    /// 未在此表出现的端点沿用实现内置默认值。
    #[serde(default)]
    pub endpoints: HashMap<String, serde_json::Value>,

    /// 配置文件路径（运行时元数据，不写入 JSON）
    #[serde(skip)]
    config_path: Option<PathBuf>,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_kiro_version() -> String {
    "0.11.107".to_string()
}

fn default_system_version() -> String {
    const SYSTEM_VERSIONS: &[&str] = &["darwin#24.6.0", "win32#10.0.22631"];
    SYSTEM_VERSIONS[fastrand::usize(..SYSTEM_VERSIONS.len())].to_string()
}

fn default_node_version() -> String {
    "22.22.0".to_string()
}

fn default_count_tokens_auth_type() -> String {
    "x-api-key".to_string()
}

fn default_tls_backend() -> TlsBackend {
    TlsBackend::Rustls
}

fn default_load_balancing_mode() -> String {
    "priority".to_string()
}

fn default_extract_thinking() -> bool {
    true
}

fn default_endpoint() -> String {
    crate::kiro::endpoint::ide::IDE_ENDPOINT_NAME.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            region: default_region(),
            auth_region: None,
            api_region: None,
            kiro_version: default_kiro_version(),
            machine_id: None,
            api_key: None,
            api_keys: Vec::new(),
            system_version: default_system_version(),
            node_version: default_node_version(),
            tls_backend: default_tls_backend(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: default_count_tokens_auth_type(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            proxy_pool: None,
            admin_api_key: None,
            load_balancing_mode: default_load_balancing_mode(),
            extract_thinking: default_extract_thinking(),
            default_endpoint: default_endpoint(),
            endpoints: HashMap::new(),
            config_path: None,
        }
    }
}

impl Config {
    /// 获取默认配置文件路径
    pub fn default_config_path() -> &'static str {
        "config.json"
    }

    /// 获取有效的 Auth Region（用于 Token 刷新）
    /// 优先使用 auth_region，未配置时回退到 region
    pub fn effective_auth_region(&self) -> &str {
        self.auth_region.as_deref().unwrap_or(&self.region)
    }

    /// 获取有效的 API Region（用于 API 请求）
    /// 优先使用 api_region，未配置时回退到 region
    pub fn effective_api_region(&self) -> &str {
        self.api_region.as_deref().unwrap_or(&self.region)
    }

    /// 从文件加载配置
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            // 配置文件不存在，返回默认配置
            let mut config = Self::default();
            config.config_path = Some(path.to_path_buf());
            return Ok(config);
        }

        let content = fs::read_to_string(path)?;
        let mut config: Config = serde_json::from_str(&content)?;
        config.config_path = Some(path.to_path_buf());
        Ok(config)
    }

    /// 获取配置文件路径（如果有）
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// 将当前配置写回原始配置文件
    pub fn save(&self) -> anyhow::Result<()> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("配置文件路径未知，无法保存配置"))?;

        let content = serde_json::to_string_pretty(self).context("序列化配置失败")?;
        fs::write(path, content)
            .with_context(|| format!("写入配置文件失败: {}", path.display()))?;
        Ok(())
    }

    /// 根据 `proxy_pool` 构建运行时的代理池
    ///
    /// 返回 `None` 表示未启用或解析为空
    pub fn build_proxy_pool(&self) -> Option<crate::http_client::ProxyPool> {
        let pool_cfg = self.proxy_pool.as_ref()?;
        if !pool_cfg.enabled {
            return None;
        }

        use crate::http_client::{ProxyConfig, ProxyPool, ProxyStrategy};

        let strategy = ProxyStrategy::from_str_lossy(&pool_cfg.strategy);

        let mut entries: Vec<ProxyConfig> = Vec::new();

        // 端口范围模板展开
        if let Some(tpl) = pool_cfg.template.as_ref() {
            if !tpl.host.trim().is_empty() {
                let part = ProxyPool::from_template(
                    &tpl.protocol,
                    &tpl.host,
                    tpl.port_start,
                    tpl.port_end,
                    pool_cfg.username.as_deref(),
                    pool_cfg.password.as_deref(),
                    strategy,
                );
                entries.extend(part.entries().iter().cloned());
            }
        }

        // 直接指定的 URL 列表
        if let Some(urls) = pool_cfg.urls.as_ref() {
            for url in urls {
                let trimmed = url.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let mut cfg = ProxyConfig::new(trimmed);
                if let (Some(u), Some(p)) = (pool_cfg.username.as_deref(), pool_cfg.password.as_deref()) {
                    if !trimmed.contains('@') {
                        cfg = cfg.with_auth(u, p);
                    }
                }
                entries.push(cfg);
            }
        }

        // 去重，保持原有顺序
        let mut seen = std::collections::HashSet::new();
        entries.retain(|cfg| seen.insert(cfg.clone()));

        if entries.is_empty() {
            return None;
        }

        let cooldown = std::time::Duration::from_secs(pool_cfg.effective_cooldown_secs());
        Some(ProxyPool::new(entries, strategy).with_cooldown(cooldown))
    }

    /// 收集所有有效的 API Keys（主Key + 启用的新Keys）
    pub fn collect_valid_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();

        // 1. 添加主Key（如果存在且非空）
        if let Some(key) = &self.api_key {
            if !key.trim().is_empty() {
                keys.push(key.clone());
            }
        }

        // 2. 添加所有启用的新Keys
        for api_key_config in &self.api_keys {
            if api_key_config.enabled && !api_key_config.key.trim().is_empty() {
                keys.push(api_key_config.key.clone());
            }
        }

        keys
    }
}
