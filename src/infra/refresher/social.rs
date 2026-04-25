//! Social refresh：`https://prod.{region}.auth.desktop.kiro.dev/refreshToken`
//!
//! refresh() 主体仅做发请求 + 调纯函数；HTTP 端到端行为靠 Phase 8 冒烟。

use std::sync::Arc;

use crate::config::Config;
use crate::domain::credential::Credential;
use crate::domain::error::RefreshError;
use crate::domain::token::{RefreshOutcome, TokenSource};
use crate::infra::machine_id::MachineIdResolver;

use super::{
    build_refresh_client, build_social_request_body, classify_refresh_http_error,
    parse_refresh_response,
};

pub struct SocialRefresher {
    config: Arc<Config>,
    resolver: Arc<MachineIdResolver>,
}

impl SocialRefresher {
    pub fn new(config: Arc<Config>, resolver: Arc<MachineIdResolver>) -> Self {
        Self { config, resolver }
    }
}

impl TokenSource for SocialRefresher {
    async fn refresh(&self, cred: &Credential) -> Result<RefreshOutcome, RefreshError> {
        tracing::info!("正在刷新 Social Token...");

        let body = build_social_request_body(cred)?;
        let region = cred.effective_auth_region(&self.config);
        let url = format!("https://prod.{region}.auth.desktop.kiro.dev/refreshToken");
        let domain = format!("prod.{region}.auth.desktop.kiro.dev");
        let machine_id = self.resolver.resolve(cred, &self.config);
        let kiro_version = &self.config.kiro.kiro_version;

        let client = build_refresh_client(cred, &self.config)?;

        let response = client
            .post(&url)
            .header("Accept", "application/json, text/plain, */*")
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                format!("KiroIDE-{kiro_version}-{machine_id}"),
            )
            .header("Accept-Encoding", "gzip, compress, deflate, br")
            .header("host", &domain)
            .header("Connection", "close")
            .body(body)
            .send()
            .await
            .map_err(RefreshError::Network)?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(classify_refresh_http_error(status, &body_text));
        }
        let json = response.text().await.map_err(RefreshError::Network)?;
        parse_refresh_response(&json)
    }
}
