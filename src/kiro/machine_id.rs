//! 设备指纹生成器
//!

use sha2::{Digest, Sha256};

use crate::kiro::model::credentials::KiroCredentials;
use crate::model::config::Config;

/// 根据凭证信息生成唯一的 Machine ID
///
/// 优先使用自定义配置，然后使用 refreshToken 生成
pub fn generate_from_credentials(credentials: &KiroCredentials, config: &Config) -> Option<String> {
    // 如果配置了自定义 machineId 且长度为 64，优先使用
    if let Some(ref machine_id) = config.machine_id {
        if machine_id.len() == 64 {
            return Some(machine_id.clone());
        }
    }

    // 使用 refreshToken 生成
    if let Some(ref refresh_token) = credentials.refresh_token {
        if !refresh_token.is_empty() {
            return Some(sha256_hex(&format!("KotlinNativeAPI/{}", refresh_token)));
        }
    }

    // 没有有效的凭证
    None
}

/// SHA256 哈希实现（返回十六进制字符串）
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
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
    fn test_generate_with_custom_machine_id() {
        let credentials = KiroCredentials::default();
        let mut config = Config::default();
        config.machine_id = Some("a".repeat(64));

        let result = generate_from_credentials(&credentials, &config);
        assert_eq!(result, Some("a".repeat(64)));
    }

    #[test]
    fn test_generate_with_refresh_token() {
        let mut credentials = KiroCredentials::default();
        credentials.refresh_token = Some("test_refresh_token".to_string());
        let config = Config::default();

        let result = generate_from_credentials(&credentials, &config);
        assert!(result.is_some());
        assert_eq!(result.as_ref().unwrap().len(), 64);
    }

    #[test]
    fn test_generate_without_credentials() {
        let credentials = KiroCredentials::default();
        let config = Config::default();

        let result = generate_from_credentials(&credentials, &config);
        assert!(result.is_none());
    }
}
