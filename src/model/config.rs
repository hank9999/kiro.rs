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

    /// Admin API 密钥（可选，启用 Admin API 功能）
    #[serde(default)]
    pub admin_api_key: Option<String>,

    /// 负载均衡模式（"priority"、"balanced"、"round_robin" 或 "adaptive_round_robin"）
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

    /// 高级模型凭证筛选配置。命中探针后会把可调用高级模型的凭证移入高级凭证库。
    #[serde(default)]
    pub premium_model_probe: PremiumModelProbeConfig,

    /// 配置文件路径（运行时元数据，不写入 JSON）
    #[serde(skip)]
    config_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PremiumModelProbeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub probability: f64,
    #[serde(default = "default_premium_probe_non_stream_only")]
    pub non_stream_only: bool,
    #[serde(default = "default_premium_probe_source_models")]
    pub source_models: Vec<String>,
    #[serde(default = "default_premium_probe_target_model")]
    pub target_model: String,
    #[serde(default = "default_premium_probe_vault_path")]
    pub premium_vault_path: String,
    #[serde(default = "default_premium_probe_event_log_path")]
    pub event_log_path: String,
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

fn default_premium_probe_non_stream_only() -> bool {
    true
}

fn default_premium_probe_source_models() -> Vec<String> {
    vec![
        "claude-sonnet-4-5-20250929".to_string(),
        "claude-sonnet-4-5-20250929-thinking".to_string(),
    ]
}

fn default_premium_probe_target_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_premium_probe_vault_path() -> String {
    "credentials.premium.json".to_string()
}

fn default_premium_probe_event_log_path() -> String {
    "credentials.premium-events.jsonl".to_string()
}

impl Default for PremiumModelProbeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            probability: 0.0,
            non_stream_only: default_premium_probe_non_stream_only(),
            source_models: default_premium_probe_source_models(),
            target_model: default_premium_probe_target_model(),
            premium_vault_path: default_premium_probe_vault_path(),
            event_log_path: default_premium_probe_event_log_path(),
        }
    }
}

impl PremiumModelProbeConfig {
    pub fn should_probe_model(&self, model: &str, stream: bool) -> bool {
        if !self.enabled || self.probability <= 0.0 {
            return false;
        }
        if self.non_stream_only && stream {
            return false;
        }
        self.source_models
            .iter()
            .any(|source| source.eq_ignore_ascii_case(model))
    }

    pub fn probability_clamped(&self) -> f64 {
        self.probability.clamp(0.0, 1.0)
    }
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
            system_version: default_system_version(),
            node_version: default_node_version(),
            tls_backend: default_tls_backend(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: default_count_tokens_auth_type(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            admin_api_key: None,
            load_balancing_mode: default_load_balancing_mode(),
            extract_thinking: default_extract_thinking(),
            default_endpoint: default_endpoint(),
            endpoints: HashMap::new(),
            premium_model_probe: PremiumModelProbeConfig::default(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_premium_model_probe_defaults_disabled() {
        let config = Config::default();
        assert!(!config.premium_model_probe.enabled);
        assert_eq!(config.premium_model_probe.probability, 0.0);
        assert!(config.premium_model_probe.non_stream_only);
    }

    #[test]
    fn test_premium_model_probe_parses_and_matches_source_model() {
        let json = r#"{
            "premiumModelProbe": {
                "enabled": true,
                "probability": 0.02,
                "nonStreamOnly": true,
                "sourceModels": ["claude-sonnet-4-5-20250929"],
                "targetModel": "claude-sonnet-4-6",
                "premiumVaultPath": "credentials.premium.json",
                "eventLogPath": "credentials.premium-events.jsonl"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.premium_model_probe.enabled);
        assert!(
            config
                .premium_model_probe
                .should_probe_model("claude-sonnet-4-5-20250929", false)
        );
        assert!(
            !config
                .premium_model_probe
                .should_probe_model("claude-sonnet-4-5-20250929", true)
        );
        assert_eq!(config.premium_model_probe.target_model, "claude-sonnet-4-6");
    }
}
