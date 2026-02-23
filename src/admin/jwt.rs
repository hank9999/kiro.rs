//! JWT Token 管理模块
//!
//! 提供 JWT Token 的生成和验证功能

use anyhow::{anyhow, Result};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// JWT Claims 结构
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (固定为 "admin")
    pub sub: String,
    /// 签发时间 (Unix timestamp)
    pub iat: usize,
    /// 过期时间 (Unix timestamp)
    pub exp: usize,
}

/// JWT Token 默认有效期（7 天）
const TOKEN_EXPIRY_SECONDS: u64 = 7 * 24 * 60 * 60;

/// 从 adminApiKey 生成 HMAC 密钥
///
/// 使用 SHA256 哈希 adminApiKey 作为 JWT 签名密钥
fn derive_secret_key(admin_api_key: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(admin_api_key.as_bytes());
    hasher.finalize().to_vec()
}

/// 生成 JWT Token
///
/// # Arguments
/// * `admin_api_key` - Admin API Key（用于派生签名密钥）
///
/// # Returns
/// * `Ok((token, expires_in))` - JWT Token 字符串和过期秒数
pub fn generate_token(admin_api_key: &str) -> Result<(String, u64)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let claims = Claims {
        sub: "admin".to_string(),
        iat: now as usize,
        exp: (now + TOKEN_EXPIRY_SECONDS) as usize,
    };

    let secret = derive_secret_key(admin_api_key);
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(&secret),
    )?;

    Ok((token, TOKEN_EXPIRY_SECONDS))
}

/// 验证 JWT Token
///
/// # Arguments
/// * `token` - JWT Token 字符串
/// * `admin_api_key` - Admin API Key（用于派生签名密钥）
///
/// # Returns
/// * `Ok(Claims)` - 验证成功，返回 Claims
/// * `Err(_)` - 验证失败（过期、签名错误等）
pub fn verify_token(token: &str, admin_api_key: &str) -> Result<Claims> {
    let secret = derive_secret_key(admin_api_key);
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(&secret),
        &Validation::default(),
    )
    .map_err(|e| anyhow!("Invalid token: {}", e))?;

    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_verify_token() {
        let admin_key = "test-admin-key-12345";

        // 生成 token
        let (token, expires_in) = generate_token(admin_key).unwrap();
        assert!(!token.is_empty());
        assert_eq!(expires_in, TOKEN_EXPIRY_SECONDS);

        // 验证 token
        let claims = verify_token(&token, admin_key).unwrap();
        assert_eq!(claims.sub, "admin");
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn test_verify_token_with_wrong_key() {
        let admin_key = "test-admin-key-12345";
        let wrong_key = "wrong-key";

        let (token, _) = generate_token(admin_key).unwrap();

        // 使用错误的密钥验证应该失败
        let result = verify_token(&token, wrong_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_invalid_token() {
        let admin_key = "test-admin-key-12345";
        let invalid_token = "invalid.token.here";

        let result = verify_token(invalid_token, admin_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_derive_secret_key_consistency() {
        let admin_key = "test-key";
        let secret1 = derive_secret_key(admin_key);
        let secret2 = derive_secret_key(admin_key);

        // 相同的输入应该产生相同的密钥
        assert_eq!(secret1, secret2);
        assert_eq!(secret1.len(), 32); // SHA256 输出 32 字节
    }
}
