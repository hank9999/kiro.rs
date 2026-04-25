//! ApiKey passthrough refresher
//!
//! API Key 凭据没有刷新流程：直接把 `kiro_api_key` 包成 RefreshOutcome 返回。

use crate::domain::credential::Credential;
use crate::domain::error::RefreshError;
use crate::domain::token::{RefreshOutcome, TokenSource};

#[derive(Default)]
pub struct ApiKeyRefresher;

impl ApiKeyRefresher {
    pub fn new() -> Self {
        Self
    }
}

impl TokenSource for ApiKeyRefresher {
    async fn refresh(&self, cred: &Credential) -> Result<RefreshOutcome, RefreshError> {
        let api_key = cred
            .kiro_api_key
            .as_deref()
            .ok_or(RefreshError::Unauthorized)?;
        Ok(RefreshOutcome {
            access_token: api_key.to_string(),
            refresh_token: None,
            profile_arn: None,
            expires_at: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refresh_returns_kiro_api_key_as_access_token() {
        let cred = Credential {
            kiro_api_key: Some("ksk_test".to_string()),
            ..Default::default()
        };
        let r = ApiKeyRefresher::new();
        let outcome = r.refresh(&cred).await.unwrap();
        assert_eq!(outcome.access_token, "ksk_test");
        assert!(outcome.refresh_token.is_none());
        assert!(outcome.expires_at.is_none());
    }

    #[tokio::test]
    async fn refresh_missing_kiro_api_key_returns_unauthorized() {
        let cred = Credential::default();
        let r = ApiKeyRefresher::new();
        assert!(matches!(r.refresh(&cred).await, Err(RefreshError::Unauthorized)));
    }
}
