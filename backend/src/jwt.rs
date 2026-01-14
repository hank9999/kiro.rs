//! JWT 认证模块
//!
//! Input: jsonwebtoken, 用户信息
//! Output: JWT token 生成和验证
//! Pos: 认证层，处理 JWT token 的创建和验证

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// JWT Claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// 用户 ID
    pub sub: i64,
    /// 用户名
    pub username: String,
    /// 过期时间 (Unix timestamp)
    pub exp: i64,
    /// 签发时间 (Unix timestamp)
    pub iat: i64,
}

/// JWT 管理器
pub struct JwtManager {
    secret: String,
    /// Token 有效期（小时）
    expiry_hours: i64,
}

impl JwtManager {
    /// 创建 JWT 管理器
    pub fn new(secret: impl Into<String>, expiry_hours: i64) -> Self {
        Self {
            secret: secret.into(),
            expiry_hours,
        }
    }

    /// 生成 JWT token
    pub fn generate_token(&self, user_id: i64, username: &str) -> anyhow::Result<String> {
        let now = Utc::now();
        let exp = now + Duration::hours(self.expiry_hours);

        let claims = Claims {
            sub: user_id,
            username: username.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )?;

        Ok(token)
    }

    /// 验证 JWT token
    pub fn verify_token(&self, token: &str) -> anyhow::Result<Claims> {
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &Validation::default(),
        )?;

        Ok(token_data.claims)
    }
}

impl Clone for JwtManager {
    fn clone(&self) -> Self {
        Self {
            secret: self.secret.clone(),
            expiry_hours: self.expiry_hours,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_generate_and_verify() {
        let jwt = JwtManager::new("test_secret_key_12345", 24);

        let token = jwt.generate_token(1, "admin").unwrap();
        assert!(!token.is_empty());

        let claims = jwt.verify_token(&token).unwrap();
        assert_eq!(claims.sub, 1);
        assert_eq!(claims.username, "admin");
    }

    #[test]
    fn test_jwt_invalid_token() {
        let jwt = JwtManager::new("test_secret_key_12345", 24);

        let result = jwt.verify_token("invalid_token");
        assert!(result.is_err());
    }
}
