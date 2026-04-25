//! IdC refresh（AWS SSO OIDC）：`https://oidc.{region}.amazonaws.com/token`
//!
//! refresh() 主体仅做发请求 + 调纯函数；HTTP 端到端行为靠 Phase 8 冒烟。

use std::sync::Arc;

use uuid::Uuid;

use crate::config::Config;
use crate::domain::credential::Credential;
use crate::domain::error::RefreshError;
use crate::domain::token::{RefreshOutcome, TokenSource};
use crate::infra::machine_id::MachineIdResolver;

use super::{
    build_idc_request_body, build_refresh_client, classify_refresh_http_error,
    parse_refresh_response,
};

pub struct IdcRefresher {
    config: Arc<Config>,
    #[allow(dead_code)]
    resolver: Arc<MachineIdResolver>,
}

impl IdcRefresher {
    pub fn new(config: Arc<Config>, resolver: Arc<MachineIdResolver>) -> Self {
        Self { config, resolver }
    }
}

impl TokenSource for IdcRefresher {
    async fn refresh(&self, cred: &Credential) -> Result<RefreshOutcome, RefreshError> {
        tracing::info!("正在刷新 IdC Token...");

        let body = build_idc_request_body(cred)?;
        let region = cred.effective_auth_region(&self.config);
        let url = format!("https://oidc.{region}.amazonaws.com/token");
        let host = format!("oidc.{region}.amazonaws.com");
        let os_name = &self.config.kiro.system_version;
        let node_version = &self.config.kiro.node_version;

        let user_agent = format!(
            "aws-sdk-js/3.980.0 ua/2.1 os/{os_name} lang/js md/nodejs#{node_version} api/sso-oidc#3.980.0 m/E KiroIDE"
        );

        let client = build_refresh_client(cred, &self.config)?;

        let response = client
            .post(&url)
            .header("content-type", "application/json")
            .header("x-amz-user-agent", "aws-sdk-js/3.980.0 KiroIDE")
            .header("user-agent", user_agent)
            .header("host", host)
            .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=4")
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
