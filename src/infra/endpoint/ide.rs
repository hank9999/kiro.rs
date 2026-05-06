//! Kiro IDE 端点。
//!
//! `ide` 是配置层面的端点名称，为了兼容现有 `defaultEndpoint` / 凭据 `endpoint`
//! 配置继续保留；实际请求地址使用新的 Kiro runtime host。
//!
//! - API: `https://runtime.{api_region}.kiro.dev/generateAssistantResponse`
//! - MCP: `https://runtime.{api_region}.kiro.dev/mcp`

use uuid::Uuid;

use crate::domain::endpoint::{KiroEndpoint, RequestContext};

pub const IDE_ENDPOINT_NAME: &str = "ide";

pub struct IdeEndpoint;

impl IdeEndpoint {
    pub fn new() -> Self {
        Self
    }

    fn api_region<'a>(&self, ctx: &'a RequestContext<'_>) -> &'a str {
        ctx.credentials.effective_api_region(ctx.config)
    }

    fn host(&self, ctx: &RequestContext<'_>) -> String {
        format!("runtime.{}.kiro.dev", self.api_region(ctx))
    }

    fn x_amz_user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 KiroIDE-{}-{}",
            ctx.config.kiro.kiro_version, ctx.machine_id
        )
    }

    fn user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{}-{}",
            ctx.config.kiro.system_version,
            ctx.config.kiro.node_version,
            ctx.config.kiro.kiro_version,
            ctx.machine_id
        )
    }
}

impl Default for IdeEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl KiroEndpoint for IdeEndpoint {
    fn name(&self) -> &'static str {
        IDE_ENDPOINT_NAME
    }

    fn api_url(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "https://runtime.{}.kiro.dev/generateAssistantResponse",
            self.api_region(ctx)
        )
    }

    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String {
        format!("https://runtime.{}.kiro.dev/mcp", self.api_region(ctx))
    }

    fn api_headers(&self, ctx: &RequestContext<'_>) -> Vec<(String, String)> {
        let mut h = vec![
            ("x-amzn-codewhisperer-optout".into(), "true".into()),
            ("x-amzn-kiro-agent-mode".into(), "vibe".into()),
            ("x-amz-user-agent".into(), self.x_amz_user_agent(ctx)),
            ("user-agent".into(), self.user_agent(ctx)),
            ("host".into(), self.host(ctx)),
            ("amz-sdk-invocation-id".into(), Uuid::new_v4().to_string()),
            ("amz-sdk-request".into(), "attempt=1; max=3".into()),
            ("Authorization".into(), format!("Bearer {}", ctx.token)),
        ];
        if ctx.credentials.is_api_key_credential() {
            h.push(("tokentype".into(), "API_KEY".into()));
        }
        h
    }

    fn mcp_headers(&self, ctx: &RequestContext<'_>) -> Vec<(String, String)> {
        let mut h = vec![
            ("x-amz-user-agent".into(), self.x_amz_user_agent(ctx)),
            ("user-agent".into(), self.user_agent(ctx)),
            ("host".into(), self.host(ctx)),
            ("amz-sdk-invocation-id".into(), Uuid::new_v4().to_string()),
            ("amz-sdk-request".into(), "attempt=1; max=3".into()),
            ("Authorization".into(), format!("Bearer {}", ctx.token)),
        ];
        if let Some(ref arn) = ctx.credentials.profile_arn {
            h.push(("x-amzn-kiro-profile-arn".into(), arn.clone()));
        }
        if ctx.credentials.is_api_key_credential() {
            h.push(("tokentype".into(), "API_KEY".into()));
        }
        h
    }

    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> String {
        inject_profile_arn(body, &ctx.credentials.profile_arn)
    }
}

fn inject_profile_arn(request_body: &str, profile_arn: &Option<String>) -> String {
    if let Some(arn) = profile_arn
        && let Ok(mut json) = serde_json::from_str::<serde_json::Value>(request_body)
    {
        json["profileArn"] = serde_json::Value::String(arn.clone());
        if let Ok(body) = serde_json::to_string(&json) {
            return body;
        }
    }
    request_body.to_string()
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::domain::credential::Credential;
    use crate::domain::endpoint::{KiroEndpoint, RequestContext};

    use super::*;

    fn ctx<'a>(cred: &'a Credential, config: &'a Config) -> RequestContext<'a> {
        RequestContext {
            credentials: cred,
            token: "token",
            machine_id: "machine",
            config,
        }
    }

    #[test]
    fn endpoint_name_remains_ide() {
        assert_eq!(IdeEndpoint::new().name(), "ide");
    }

    #[test]
    fn urls_use_runtime_host() {
        let endpoint = IdeEndpoint::new();
        let config = Config::default();
        let cred = Credential::default();
        let ctx = ctx(&cred, &config);

        assert_eq!(
            endpoint.api_url(&ctx),
            "https://runtime.us-east-1.kiro.dev/generateAssistantResponse"
        );
        assert_eq!(
            endpoint.mcp_url(&ctx),
            "https://runtime.us-east-1.kiro.dev/mcp"
        );
    }

    #[test]
    fn credential_api_region_overrides_config_region() {
        let endpoint = IdeEndpoint::new();
        let config = Config::default();
        let cred = Credential {
            api_region: Some("eu-central-1".into()),
            ..Default::default()
        };
        let ctx = ctx(&cred, &config);

        assert_eq!(
            endpoint.api_url(&ctx),
            "https://runtime.eu-central-1.kiro.dev/generateAssistantResponse"
        );
    }

    #[test]
    fn injects_profile_arn() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let arn = Some("arn:test".to_string());
        let result = inject_profile_arn(body, &arn);
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["profileArn"], "arn:test");
    }

    #[test]
    fn inject_profile_arn_with_none_keeps_body_without_profile() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let result = inject_profile_arn(body, &None);
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(json.get("profileArn").is_none());
        assert_eq!(json["conversationState"]["conversationId"], "c1");
    }

    #[test]
    fn inject_profile_arn_overwrites_existing() {
        let body = r#"{"conversationState":{},"profileArn":"old-arn"}"#;
        let arn = Some("new-arn".to_string());
        let result = inject_profile_arn(body, &arn);
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["profileArn"], "new-arn");
    }

    #[test]
    fn inject_profile_arn_invalid_json_returns_original_body() {
        let body = "not-valid-json";
        let arn = Some("arn:test".to_string());
        let result = inject_profile_arn(body, &arn);
        assert_eq!(result, "not-valid-json");
    }
}
