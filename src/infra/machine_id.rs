//! 设备指纹生成器（MachineIdResolver，无全局静态）
//!
//! 由 [`crate::service::credential_pool::CredentialPool`] 持有；不同 pool 实例的
//! fallback 缓存互相隔离。

use std::collections::HashMap;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::Config;
use crate::domain::credential::Credential;

/// 标准化 machineId 格式
///
/// 支持：
/// - 64 字符十六进制（直接返回）
/// - UUID 格式（移除连字符后补齐到 64 字符）
pub fn normalize_machine_id(machine_id: &str) -> Option<String> {
    let trimmed = machine_id.trim();
    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(trimmed.to_string());
    }
    let without_dashes: String = trimmed.chars().filter(|c| *c != '-').collect();
    if without_dashes.len() == 32 && without_dashes.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("{without_dashes}{without_dashes}"));
    }
    None
}

/// SHA256 hex
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// MachineId 解析器
///
/// 持有按 `credential.id` 分桶的兜底缓存（进程内稳定，不持久化）。
#[derive(Default)]
pub struct MachineIdResolver {
    fallback: Mutex<HashMap<Option<u64>, String>>,
}

impl MachineIdResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// 优先级：
    /// 1. 凭据级 `machineId`（合法格式）
    /// 2. 全局 `config.kiro.machine_id`（合法格式）
    /// 3. 派生：API Key 凭据基于 `kiroApiKey`；OAuth 凭据基于 `refreshToken`
    /// 4. 兜底：随机种子派生，按凭据 id 在本 resolver 内缓存
    pub fn resolve(&self, credentials: &Credential, config: &Config) -> String {
        if let Some(ref machine_id) = credentials.machine_id {
            if let Some(normalized) = normalize_machine_id(machine_id) {
                return normalized;
            }
        }
        if let Some(ref machine_id) = config.kiro.machine_id {
            if let Some(normalized) = normalize_machine_id(machine_id) {
                return normalized;
            }
        }

        if credentials.is_api_key_credential() {
            if let Some(ref api_key) = credentials.kiro_api_key {
                if !api_key.is_empty() {
                    return sha256_hex(&format!("KiroAPIKey/{api_key}"));
                }
            }
        } else if let Some(ref refresh_token) = credentials.refresh_token {
            if !refresh_token.is_empty() {
                return sha256_hex(&format!("KotlinNativeAPI/{refresh_token}"));
            }
        }

        self.fallback_for(credentials)
    }

    fn fallback_for(&self, credentials: &Credential) -> String {
        let mut map = self.fallback.lock();
        if let Some(existing) = map.get(&credentials.id) {
            return existing.clone();
        }
        let seed = Uuid::new_v4();
        let derived = sha256_hex(&format!("KiroFallback/{seed}"));
        tracing::warn!(
            credential_id = ?credentials.id,
            "凭据缺少派生材料（kiroApiKey/refreshToken 均不可用），使用随机兜底 machineId（进程内稳定）"
        );
        map.insert(credentials.id, derived.clone());
        derived
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hex() {
        let result = sha256_hex("test");
        assert_eq!(result.len(), 64);
        assert_eq!(
            result,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }

    #[test]
    fn test_resolve_with_custom_machine_id() {
        let credentials = Credential::default();
        let mut config = Config::default();
        config.kiro.machine_id = Some("a".repeat(64));
        let resolver = MachineIdResolver::new();
        assert_eq!(resolver.resolve(&credentials, &config), "a".repeat(64));
    }

    #[test]
    fn test_resolve_credential_machine_id_overrides_config() {
        let credentials = Credential {
            machine_id: Some("b".repeat(64)),
            ..Default::default()
        };
        let mut config = Config::default();
        config.kiro.machine_id = Some("a".repeat(64));
        let resolver = MachineIdResolver::new();
        assert_eq!(resolver.resolve(&credentials, &config), "b".repeat(64));
    }

    #[test]
    fn test_resolve_with_refresh_token() {
        let credentials = Credential {
            refresh_token: Some("test_refresh_token".to_string()),
            ..Default::default()
        };
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        assert_eq!(resolver.resolve(&credentials, &config).len(), 64);
    }

    #[test]
    fn test_resolve_without_credentials_uses_fallback() {
        let credentials = Credential::default();
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        let result = resolver.resolve(&credentials, &config);
        assert_eq!(result.len(), 64);
        assert!(result.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_resolve_with_api_key() {
        let credentials = Credential {
            kiro_api_key: Some("ksk_test_api_key".to_string()),
            ..Default::default()
        };
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        let result = resolver.resolve(&credentials, &config);
        assert_eq!(result.len(), 64);
        assert_eq!(result, sha256_hex("KiroAPIKey/ksk_test_api_key"));
    }

    #[test]
    fn test_api_key_and_refresh_token_are_mutually_exclusive() {
        let credentials = Credential {
            kiro_api_key: Some("ksk_test".to_string()),
            refresh_token: Some("should_not_be_used".to_string()),
            ..Default::default()
        };
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        assert_eq!(
            resolver.resolve(&credentials, &config),
            sha256_hex("KiroAPIKey/ksk_test")
        );
    }

    #[test]
    fn test_api_key_auth_method_empty_uses_fallback_not_refresh_token() {
        let credentials = Credential {
            id: Some(u64::MAX - 1),
            auth_method: Some("api_key".to_string()),
            refresh_token: Some("should_not_be_used".to_string()),
            ..Default::default()
        };
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        let result = resolver.resolve(&credentials, &config);
        assert_eq!(result.len(), 64);
        assert_ne!(result, sha256_hex("KotlinNativeAPI/should_not_be_used"));
    }

    #[test]
    fn test_fallback_is_stable_per_credential_in_same_resolver() {
        let credentials = Credential {
            id: Some(u64::MAX - 10),
            ..Default::default()
        };
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        let first = resolver.resolve(&credentials, &config);
        let second = resolver.resolve(&credentials, &config);
        assert_eq!(first, second);
    }

    #[test]
    fn test_fallback_differs_across_credentials() {
        let cred_a = Credential {
            id: Some(u64::MAX - 20),
            ..Default::default()
        };
        let cred_b = Credential {
            id: Some(u64::MAX - 21),
            ..Default::default()
        };
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        let id_a = resolver.resolve(&cred_a, &config);
        let id_b = resolver.resolve(&cred_b, &config);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_fallback_isolated_between_resolvers() {
        // 不同 resolver 实例的 fallback 不共享：同一 cred 在两个 resolver 上得到不同的兜底值
        let credentials = Credential {
            id: Some(u64::MAX - 30),
            ..Default::default()
        };
        let config = Config::default();
        let r1 = MachineIdResolver::new();
        let r2 = MachineIdResolver::new();
        let id1 = r1.resolve(&credentials, &config);
        let id2 = r2.resolve(&credentials, &config);
        assert_ne!(id1, id2, "两个独立 resolver 的兜底必须互相隔离");
    }

    #[test]
    fn test_normalize_uuid_format() {
        let uuid = "2582956e-cc88-4669-b546-07adbffcb894";
        let result = normalize_machine_id(uuid).unwrap();
        assert_eq!(result.len(), 64);
        assert_eq!(
            result,
            "2582956ecc884669b54607adbffcb8942582956ecc884669b54607adbffcb894"
        );
    }

    #[test]
    fn test_normalize_64_char_hex() {
        let hex64 = "a".repeat(64);
        assert_eq!(normalize_machine_id(&hex64), Some(hex64));
    }

    #[test]
    fn test_normalize_invalid_format() {
        assert!(normalize_machine_id("invalid").is_none());
        assert!(normalize_machine_id("too-short").is_none());
        assert!(normalize_machine_id(&"g".repeat(64)).is_none());
    }

    #[test]
    fn test_resolve_with_uuid_machine_id() {
        let credentials = Credential {
            machine_id: Some("2582956e-cc88-4669-b546-07adbffcb894".to_string()),
            ..Default::default()
        };
        let config = Config::default();
        let resolver = MachineIdResolver::new();
        assert_eq!(resolver.resolve(&credentials, &config).len(), 64);
    }
}
