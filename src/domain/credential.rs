//! Credential 数据模型（迁移自 `kiro::model::credentials::KiroCredentials`）
//!
//! 字段集合与原 KiroCredentials 保持一致；JSON 兼容契约不变（`credentials.json` 单/多格式）。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::domain::error::ConfigError;
use crate::infra::http::client::ProxyConfig;

/// Kiro 凭据
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,

    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: u32,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_region: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_username: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,

    #[serde(default)]
    pub disabled: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub kiro_api_key: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn canonicalize_auth_method_value(value: &str) -> &str {
    if value.eq_ignore_ascii_case("builder-id") || value.eq_ignore_ascii_case("iam") {
        "idc"
    } else if value.eq_ignore_ascii_case("api_key") || value.eq_ignore_ascii_case("apikey") {
        "api_key"
    } else {
        value
    }
}

/// 凭据文件（单对象 / 数组双格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CredentialsFile {
    Single(Credential),
    Multiple(Vec<Credential>),
}

impl CredentialsFile {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(CredentialsFile::Multiple(vec![]));
        }
        let content = fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(CredentialsFile::Multiple(vec![]));
        }
        Ok(serde_json::from_str(&content)?)
    }

    /// 按 priority 升序排序后的凭据列表（同时归一 auth_method）
    pub fn into_sorted_credentials(self) -> Vec<Credential> {
        match self {
            CredentialsFile::Single(mut cred) => {
                cred.canonicalize_auth_method();
                vec![cred]
            }
            CredentialsFile::Multiple(mut creds) => {
                creds.sort_by_key(|c| c.priority);
                for cred in &mut creds {
                    cred.canonicalize_auth_method();
                }
                creds
            }
        }
    }

    pub fn is_multiple(&self) -> bool {
        matches!(self, CredentialsFile::Multiple(_))
    }
}

impl Credential {
    pub const PROXY_DIRECT: &'static str = "direct";

    pub fn default_credentials_path() -> &'static str {
        "credentials.json"
    }

    /// 凭据.auth_region > 凭据.region > config.auth_region > config.region
    pub fn effective_auth_region<'a>(&'a self, config: &'a Config) -> &'a str {
        self.auth_region
            .as_deref()
            .or(self.region.as_deref())
            .unwrap_or(config.effective_auth_region())
    }

    /// 凭据.api_region > config.api_region > config.region
    pub fn effective_api_region<'a>(&'a self, config: &'a Config) -> &'a str {
        self.api_region
            .as_deref()
            .unwrap_or(config.effective_api_region())
    }

    /// 凭据代理 > 全局代理 > 无代理；"direct" 表示显式不走代理
    pub fn effective_proxy(&self, global_proxy: Option<&ProxyConfig>) -> Option<ProxyConfig> {
        match self.proxy_url.as_deref() {
            Some(url) if url.eq_ignore_ascii_case(Self::PROXY_DIRECT) => None,
            Some(url) => {
                let mut proxy = ProxyConfig::new(url);
                if let (Some(username), Some(password)) =
                    (&self.proxy_username, &self.proxy_password)
                {
                    proxy = proxy.with_auth(username, password);
                }
                Some(proxy)
            }
            None => global_proxy.cloned(),
        }
    }

    pub fn canonicalize_auth_method(&mut self) {
        let auth_method = match &self.auth_method {
            Some(m) => m,
            None => return,
        };
        let canonical = canonicalize_auth_method_value(auth_method);
        if canonical != auth_method {
            self.auth_method = Some(canonical.to_string());
        }
    }

    /// Free 账号不支持 Opus 模型
    pub fn supports_opus(&self) -> bool {
        match &self.subscription_title {
            Some(title) => !title.to_uppercase().contains("FREE"),
            None => true,
        }
    }

    pub fn is_api_key_credential(&self) -> bool {
        self.kiro_api_key.is_some()
            || self
                .auth_method
                .as_deref()
                .map(|m| m.eq_ignore_ascii_case("api_key") || m.eq_ignore_ascii_case("apikey"))
                .unwrap_or(false)
    }
}

#[cfg(test)]
impl Credential {
    fn from_json(json_string: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_string)
    }

    fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn test_from_json() {
        let json = r#"{
            "accessToken": "test_token",
            "refreshToken": "test_refresh",
            "profileArn": "arn:aws:test",
            "expiresAt": "2024-01-01T00:00:00Z",
            "authMethod": "social"
        }"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.access_token, Some("test_token".to_string()));
        assert_eq!(creds.refresh_token, Some("test_refresh".to_string()));
        assert_eq!(creds.profile_arn, Some("arn:aws:test".to_string()));
        assert_eq!(creds.expires_at, Some("2024-01-01T00:00:00Z".to_string()));
        assert_eq!(creds.auth_method, Some("social".to_string()));
    }

    #[test]
    fn test_from_json_with_unknown_keys() {
        let json = r#"{
            "accessToken": "test_token",
            "unknownField": "should be ignored"
        }"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.access_token, Some("test_token".to_string()));
    }

    #[test]
    fn test_to_json() {
        let creds = Credential {
            access_token: Some("token".to_string()),
            auth_method: Some("social".to_string()),
            ..Default::default()
        };
        let json = creds.to_pretty_json().unwrap();
        assert!(json.contains("accessToken"));
        assert!(json.contains("authMethod"));
        assert!(!json.contains("refreshToken"));
        assert!(!json.contains("priority"));
    }

    #[test]
    fn test_default_credentials_path() {
        assert_eq!(Credential::default_credentials_path(), "credentials.json");
    }

    #[test]
    fn test_priority_default() {
        let json = r#"{"refreshToken": "test"}"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.priority, 0);
    }

    #[test]
    fn test_priority_explicit() {
        let json = r#"{"refreshToken": "test", "priority": 5}"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.priority, 5);
    }

    #[test]
    fn test_credentials_file_single() {
        let json = r#"{"refreshToken": "test", "expiresAt": "2025-12-31T00:00:00Z"}"#;
        let f: CredentialsFile = serde_json::from_str(json).unwrap();
        assert!(matches!(f, CredentialsFile::Single(_)));
    }

    #[test]
    fn test_credentials_file_multiple() {
        let json = r#"[
            {"refreshToken": "test1", "priority": 1},
            {"refreshToken": "test2", "priority": 0}
        ]"#;
        let f: CredentialsFile = serde_json::from_str(json).unwrap();
        assert!(matches!(f, CredentialsFile::Multiple(_)));
        assert_eq!(f.into_sorted_credentials().len(), 2);
    }

    #[test]
    fn test_credentials_file_priority_sorting() {
        let json = r#"[
            {"refreshToken": "t1", "priority": 2},
            {"refreshToken": "t2", "priority": 0},
            {"refreshToken": "t3", "priority": 1}
        ]"#;
        let f: CredentialsFile = serde_json::from_str(json).unwrap();
        let list = f.into_sorted_credentials();
        assert_eq!(list[0].refresh_token, Some("t2".to_string()));
        assert_eq!(list[1].refresh_token, Some("t3".to_string()));
        assert_eq!(list[2].refresh_token, Some("t1".to_string()));
    }

    #[test]
    fn test_region_field_parsing() {
        let json = r#"{"refreshToken": "test_refresh", "region": "us-east-1"}"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.refresh_token, Some("test_refresh".to_string()));
        assert_eq!(creds.region, Some("us-east-1".to_string()));
    }

    #[test]
    fn test_region_field_missing_backward_compat() {
        let json = r#"{"refreshToken": "test_refresh", "authMethod": "social"}"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.refresh_token, Some("test_refresh".to_string()));
        assert_eq!(creds.region, None);
    }

    #[test]
    fn test_region_field_serialization() {
        let creds = Credential {
            refresh_token: Some("test".to_string()),
            region: Some("eu-west-1".to_string()),
            ..Default::default()
        };
        let json = creds.to_pretty_json().unwrap();
        assert!(json.contains("region"));
        assert!(json.contains("eu-west-1"));
    }

    #[test]
    fn test_region_field_none_not_serialized() {
        let creds = Credential {
            refresh_token: Some("test".to_string()),
            region: None,
            ..Default::default()
        };
        let json = creds.to_pretty_json().unwrap();
        assert!(!json.contains("region"));
    }

    #[test]
    fn test_machine_id_field_parsing() {
        let machine_id = "a".repeat(64);
        let json = format!(
            r#"{{"refreshToken": "test_refresh", "machineId": "{machine_id}"}}"#
        );
        let creds = Credential::from_json(&json).unwrap();
        assert_eq!(creds.refresh_token, Some("test_refresh".to_string()));
        assert_eq!(creds.machine_id, Some(machine_id));
    }

    #[test]
    fn test_machine_id_field_serialization() {
        let creds = Credential {
            refresh_token: Some("test".to_string()),
            machine_id: Some("b".repeat(64)),
            ..Default::default()
        };
        let json = creds.to_pretty_json().unwrap();
        assert!(json.contains("machineId"));
    }

    #[test]
    fn test_machine_id_field_none_not_serialized() {
        let creds = Credential {
            refresh_token: Some("test".to_string()),
            machine_id: None,
            ..Default::default()
        };
        let json = creds.to_pretty_json().unwrap();
        assert!(!json.contains("machineId"));
    }

    #[test]
    fn test_multiple_credentials_with_different_regions() {
        let json = r#"[
            {"refreshToken": "t1", "region": "us-east-1"},
            {"refreshToken": "t2", "region": "eu-west-1"},
            {"refreshToken": "t3"}
        ]"#;
        let f: CredentialsFile = serde_json::from_str(json).unwrap();
        let list = f.into_sorted_credentials();
        assert_eq!(list[0].region, Some("us-east-1".to_string()));
        assert_eq!(list[1].region, Some("eu-west-1".to_string()));
        assert_eq!(list[2].region, None);
    }

    #[test]
    fn test_region_field_with_all_fields() {
        let json = r#"{
            "id": 1,
            "accessToken": "access",
            "refreshToken": "refresh",
            "profileArn": "arn:aws:test",
            "expiresAt": "2025-12-31T00:00:00Z",
            "authMethod": "idc",
            "clientId": "client123",
            "clientSecret": "secret456",
            "priority": 5,
            "region": "ap-northeast-1"
        }"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.id, Some(1));
        assert_eq!(creds.access_token, Some("access".to_string()));
        assert_eq!(creds.refresh_token, Some("refresh".to_string()));
        assert_eq!(creds.profile_arn, Some("arn:aws:test".to_string()));
        assert_eq!(creds.expires_at, Some("2025-12-31T00:00:00Z".to_string()));
        assert_eq!(creds.auth_method, Some("idc".to_string()));
        assert_eq!(creds.client_id, Some("client123".to_string()));
        assert_eq!(creds.client_secret, Some("secret456".to_string()));
        assert_eq!(creds.priority, 5);
        assert_eq!(creds.region, Some("ap-northeast-1".to_string()));
    }

    #[test]
    fn test_region_roundtrip() {
        let original = Credential {
            id: Some(42),
            access_token: Some("token".to_string()),
            refresh_token: Some("refresh".to_string()),
            auth_method: Some("social".to_string()),
            priority: 3,
            region: Some("us-west-2".to_string()),
            machine_id: Some("c".repeat(64)),
            ..Default::default()
        };
        let json = original.to_pretty_json().unwrap();
        let parsed = Credential::from_json(&json).unwrap();
        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.access_token, original.access_token);
        assert_eq!(parsed.refresh_token, original.refresh_token);
        assert_eq!(parsed.priority, original.priority);
        assert_eq!(parsed.region, original.region);
        assert_eq!(parsed.machine_id, original.machine_id);
    }

    #[test]
    fn test_auth_region_field_parsing() {
        let json = r#"{"refreshToken": "test_refresh", "authRegion": "eu-central-1"}"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.auth_region, Some("eu-central-1".to_string()));
        assert_eq!(creds.api_region, None);
    }

    #[test]
    fn test_api_region_field_parsing() {
        let json = r#"{"refreshToken": "test_refresh", "apiRegion": "ap-southeast-1"}"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.api_region, Some("ap-southeast-1".to_string()));
        assert_eq!(creds.auth_region, None);
    }

    #[test]
    fn test_auth_api_region_serialization() {
        let creds = Credential {
            refresh_token: Some("test".to_string()),
            auth_region: Some("eu-west-1".to_string()),
            api_region: Some("us-west-2".to_string()),
            ..Default::default()
        };
        let json = creds.to_pretty_json().unwrap();
        assert!(json.contains("authRegion"));
        assert!(json.contains("eu-west-1"));
        assert!(json.contains("apiRegion"));
        assert!(json.contains("us-west-2"));
    }

    #[test]
    fn test_auth_api_region_none_not_serialized() {
        let creds = Credential {
            refresh_token: Some("test".to_string()),
            auth_region: None,
            api_region: None,
            ..Default::default()
        };
        let json = creds.to_pretty_json().unwrap();
        assert!(!json.contains("authRegion"));
        assert!(!json.contains("apiRegion"));
    }

    #[test]
    fn test_auth_api_region_roundtrip() {
        let original = Credential {
            refresh_token: Some("refresh".to_string()),
            region: Some("us-east-1".to_string()),
            auth_region: Some("eu-west-1".to_string()),
            api_region: Some("ap-northeast-1".to_string()),
            ..Default::default()
        };
        let json = original.to_pretty_json().unwrap();
        let parsed = Credential::from_json(&json).unwrap();
        assert_eq!(parsed.region, original.region);
        assert_eq!(parsed.auth_region, original.auth_region);
        assert_eq!(parsed.api_region, original.api_region);
    }

    #[test]
    fn test_backward_compat_no_auth_api_region() {
        let json = r#"{"refreshToken": "test_refresh", "region": "us-east-1"}"#;
        let creds = Credential::from_json(json).unwrap();
        assert_eq!(creds.region, Some("us-east-1".to_string()));
        assert_eq!(creds.auth_region, None);
        assert_eq!(creds.api_region, None);
    }

    #[test]
    fn test_effective_auth_region_credential_auth_region_highest() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        config.region.auth_region = Some("config-auth-region".to_string());
        let creds = Credential {
            region: Some("cred-region".to_string()),
            auth_region: Some("cred-auth-region".to_string()),
            ..Default::default()
        };
        assert_eq!(creds.effective_auth_region(&config), "cred-auth-region");
    }

    #[test]
    fn test_effective_auth_region_fallback_to_credential_region() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        config.region.auth_region = Some("config-auth-region".to_string());
        let creds = Credential {
            region: Some("cred-region".to_string()),
            ..Default::default()
        };
        assert_eq!(creds.effective_auth_region(&config), "cred-region");
    }

    #[test]
    fn test_effective_auth_region_fallback_to_config_auth_region() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        config.region.auth_region = Some("config-auth-region".to_string());
        let creds = Credential::default();
        assert_eq!(creds.effective_auth_region(&config), "config-auth-region");
    }

    #[test]
    fn test_effective_auth_region_fallback_to_config_region() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        let creds = Credential::default();
        assert_eq!(creds.effective_auth_region(&config), "config-region");
    }

    #[test]
    fn test_effective_api_region_credential_api_region_highest() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        config.region.api_region = Some("config-api-region".to_string());
        let creds = Credential {
            api_region: Some("cred-api-region".to_string()),
            ..Default::default()
        };
        assert_eq!(creds.effective_api_region(&config), "cred-api-region");
    }

    #[test]
    fn test_effective_api_region_fallback_to_config_api_region() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        config.region.api_region = Some("config-api-region".to_string());
        let creds = Credential::default();
        assert_eq!(creds.effective_api_region(&config), "config-api-region");
    }

    #[test]
    fn test_effective_api_region_fallback_to_config_region() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        let creds = Credential::default();
        assert_eq!(creds.effective_api_region(&config), "config-region");
    }

    #[test]
    fn test_effective_api_region_ignores_credential_region() {
        let mut config = Config::default();
        config.region.region = "config-region".to_string();
        let creds = Credential {
            region: Some("cred-region".to_string()),
            ..Default::default()
        };
        assert_eq!(creds.effective_api_region(&config), "config-region");
    }

    #[test]
    fn test_auth_and_api_region_independent() {
        let mut config = Config::default();
        config.region.region = "default".to_string();
        let creds = Credential {
            auth_region: Some("auth-only".to_string()),
            api_region: Some("api-only".to_string()),
            ..Default::default()
        };
        assert_eq!(creds.effective_auth_region(&config), "auth-only");
        assert_eq!(creds.effective_api_region(&config), "api-only");
    }

    #[test]
    fn test_effective_proxy_credential_overrides_global() {
        let global = ProxyConfig::new("http://global:8080");
        let creds = Credential {
            proxy_url: Some("socks5://cred:1080".to_string()),
            ..Default::default()
        };
        let result = creds.effective_proxy(Some(&global));
        assert_eq!(result, Some(ProxyConfig::new("socks5://cred:1080")));
    }

    #[test]
    fn test_effective_proxy_credential_with_auth() {
        let global = ProxyConfig::new("http://global:8080");
        let creds = Credential {
            proxy_url: Some("http://proxy:3128".to_string()),
            proxy_username: Some("user".to_string()),
            proxy_password: Some("pass".to_string()),
            ..Default::default()
        };
        let result = creds.effective_proxy(Some(&global));
        let expected = ProxyConfig::new("http://proxy:3128").with_auth("user", "pass");
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn test_effective_proxy_direct_bypasses_global() {
        let global = ProxyConfig::new("http://global:8080");
        let creds = Credential {
            proxy_url: Some("direct".to_string()),
            ..Default::default()
        };
        let result = creds.effective_proxy(Some(&global));
        assert_eq!(result, None);
    }

    #[test]
    fn test_effective_proxy_direct_case_insensitive() {
        let global = ProxyConfig::new("http://global:8080");
        let creds = Credential {
            proxy_url: Some("DIRECT".to_string()),
            ..Default::default()
        };
        let result = creds.effective_proxy(Some(&global));
        assert_eq!(result, None);
    }

    #[test]
    fn test_effective_proxy_fallback_to_global() {
        let global = ProxyConfig::new("http://global:8080");
        let creds = Credential::default();
        let result = creds.effective_proxy(Some(&global));
        assert_eq!(result, Some(ProxyConfig::new("http://global:8080")));
    }

    #[test]
    fn test_effective_proxy_none_when_no_proxy() {
        let creds = Credential::default();
        let result = creds.effective_proxy(None);
        assert_eq!(result, None);
    }
}
