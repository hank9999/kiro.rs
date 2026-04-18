//! Kiro IDE 端点
//!
//! 对应 Kiro IDE 客户端目前使用的 AWS CodeWhisperer 端点：
//! - API: `https://q.{api_region}.amazonaws.com/generateAssistantResponse`
//! - MCP: `https://q.{api_region}.amazonaws.com/mcp`
//!
//! 请求头使用 aws-sdk-js User-Agent 标识。请求体会在根对象上注入 `profileArn`。

use reqwest::{Client, RequestBuilder};
use uuid::Uuid;

use super::{KiroEndpoint, KiroRequest, RequestContext};

/// Kiro IDE 端点名称
pub const IDE_ENDPOINT_NAME: &str = "ide";

/// Kiro IDE 端点
pub struct IdeEndpoint;

impl IdeEndpoint {
    pub fn new() -> Self {
        Self
    }

    fn api_region<'a>(&self, ctx: &'a RequestContext<'_>) -> &'a str {
        ctx.credentials.effective_api_region(ctx.config)
    }

    fn host(&self, ctx: &RequestContext<'_>) -> String {
        format!("q.{}.amazonaws.com", self.api_region(ctx))
    }

    fn x_amz_user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 KiroIDE-{}-{}",
            ctx.config.kiro_version, ctx.machine_id
        )
    }

    fn user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{}-{}",
            ctx.config.system_version,
            ctx.config.node_version,
            ctx.config.kiro_version,
            ctx.machine_id
        )
    }
}

impl Default for IdeEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl IdeEndpoint {
    fn usage_limits_url(&self, ctx: &RequestContext<'_>) -> String {
        let host = self.host(ctx);
        let mut url = format!(
            "https://{}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST",
            host
        );
        if let Some(profile_arn) = &ctx.credentials.profile_arn {
            url.push_str(&format!(
                "&profileArn={}",
                urlencoding::encode(profile_arn)
            ));
        }
        url
    }

    fn usage_limits_x_amz_user_agent(&self, ctx: &RequestContext<'_>) -> String {
        // 历史 get_usage_limits 使用 aws-sdk-js/1.0.0（区别于 API/MCP 的 1.0.34）
        format!(
            "aws-sdk-js/1.0.0 KiroIDE-{}-{}",
            ctx.config.kiro_version, ctx.machine_id
        )
    }

    fn usage_limits_user_agent(&self, ctx: &RequestContext<'_>) -> String {
        // 历史 get_usage_limits 使用 codewhispererruntime + m/N,E（区别于 API/MCP 的 codewhispererstreaming + m/E）
        format!(
            "aws-sdk-js/1.0.0 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{}-{}",
            ctx.config.system_version,
            ctx.config.node_version,
            ctx.config.kiro_version,
            ctx.machine_id
        )
    }
}

impl KiroEndpoint for IdeEndpoint {
    fn name(&self) -> &'static str {
        IDE_ENDPOINT_NAME
    }

    fn build_request(
        &self,
        client: &Client,
        ctx: &RequestContext<'_>,
        req: &KiroRequest<'_>,
    ) -> anyhow::Result<RequestBuilder> {
        match req {
            KiroRequest::GenerateAssistant { body, .. } => {
                let url = format!(
                    "https://q.{}.amazonaws.com/generateAssistantResponse",
                    self.api_region(ctx)
                );
                let body = inject_profile_arn(body, &ctx.credentials.profile_arn);
                let mut builder = client
                    .post(&url)
                    .body(body)
                    .header("content-type", "application/json")
                    .header("Connection", "close")
                    .header("x-amzn-codewhisperer-optout", "true")
                    .header("x-amzn-kiro-agent-mode", "vibe")
                    .header("x-amz-user-agent", self.x_amz_user_agent(ctx))
                    .header("user-agent", self.user_agent(ctx))
                    .header("host", self.host(ctx))
                    .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
                    .header("amz-sdk-request", "attempt=1; max=3")
                    .header("Authorization", format!("Bearer {}", ctx.token));
                if ctx.credentials.is_api_key_credential() {
                    builder = builder.header("tokentype", "API_KEY");
                }
                Ok(builder)
            }
            KiroRequest::Mcp { body } => {
                let url = format!("https://q.{}.amazonaws.com/mcp", self.api_region(ctx));
                let mut builder = client
                    .post(&url)
                    .body((*body).to_string())
                    .header("content-type", "application/json")
                    .header("Connection", "close")
                    .header("x-amz-user-agent", self.x_amz_user_agent(ctx))
                    .header("user-agent", self.user_agent(ctx))
                    .header("host", self.host(ctx))
                    .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
                    .header("amz-sdk-request", "attempt=1; max=3")
                    .header("Authorization", format!("Bearer {}", ctx.token));
                if let Some(ref arn) = ctx.credentials.profile_arn {
                    builder = builder.header("x-amzn-kiro-profile-arn", arn);
                }
                if ctx.credentials.is_api_key_credential() {
                    builder = builder.header("tokentype", "API_KEY");
                }
                Ok(builder)
            }
            KiroRequest::UsageLimits => {
                let url = self.usage_limits_url(ctx);
                let mut builder = client
                    .get(&url)
                    .header("x-amz-user-agent", self.usage_limits_x_amz_user_agent(ctx))
                    .header("user-agent", self.usage_limits_user_agent(ctx))
                    .header("host", self.host(ctx))
                    .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
                    .header("amz-sdk-request", "attempt=1; max=1")
                    .header("Authorization", format!("Bearer {}", ctx.token))
                    .header("Connection", "close");
                if ctx.credentials.is_api_key_credential() {
                    builder = builder.header("tokentype", "API_KEY");
                }
                Ok(builder)
            }
        }
    }

    fn api_url(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            self.api_region(ctx)
        )
    }

    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String {
        format!("https://q.{}.amazonaws.com/mcp", self.api_region(ctx))
    }

    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        let mut req = req
            .header("x-amzn-codewhisperer-optout", "true")
            .header("x-amzn-kiro-agent-mode", "vibe")
            .header("x-amz-user-agent", self.x_amz_user_agent(ctx))
            .header("user-agent", self.user_agent(ctx))
            .header("host", self.host(ctx))
            .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=3")
            .header("Authorization", format!("Bearer {}", ctx.token));

        if ctx.credentials.is_api_key_credential() {
            req = req.header("tokentype", "API_KEY");
        }
        req
    }

    fn decorate_mcp(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        let mut req = req
            .header("x-amz-user-agent", self.x_amz_user_agent(ctx))
            .header("user-agent", self.user_agent(ctx))
            .header("host", self.host(ctx))
            .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=3")
            .header("Authorization", format!("Bearer {}", ctx.token));

        if let Some(ref arn) = ctx.credentials.profile_arn {
            req = req.header("x-amzn-kiro-profile-arn", arn);
        }
        if ctx.credentials.is_api_key_credential() {
            req = req.header("tokentype", "API_KEY");
        }
        req
    }

    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> String {
        inject_profile_arn(body, &ctx.credentials.profile_arn)
    }
}

/// 将 profile_arn 注入到请求体 JSON 根对象
fn inject_profile_arn(request_body: &str, profile_arn: &Option<String>) -> String {
    if let Some(arn) = profile_arn {
        if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(request_body) {
            json["profileArn"] = serde_json::Value::String(arn.clone());
            if let Ok(body) = serde_json::to_string(&json) {
                return body;
            }
        }
    }
    request_body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::inject_profile_arn;
    use crate::kiro::model::credentials::KiroCredentials;
    use crate::model::config::Config;
    use serde_json::Value;

    fn sample_config() -> Config {
        let mut c = Config::default();
        c.region = "us-east-1".to_string();
        c.api_region = None;
        c.kiro_version = "0.1.0".to_string();
        c.system_version = "linux-x86_64".to_string();
        c.node_version = "20.0.0".to_string();
        c
    }

    fn sample_credentials_with_arn() -> KiroCredentials {
        KiroCredentials {
            profile_arn: Some(
                "arn:aws:codewhisperer:us-east-1:123:profile/ABC".to_string(),
            ),
            ..Default::default()
        }
    }

    fn sample_credentials_no_arn() -> KiroCredentials {
        KiroCredentials::default()
    }

    #[test]
    fn test_build_request_generate_assistant_url_and_auth() {
        let endpoint = IdeEndpoint::new();
        let client = reqwest::Client::new();
        let creds = sample_credentials_with_arn();
        let config = sample_config();
        let ctx = RequestContext {
            credentials: &creds,
            token: "T0KEN",
            machine_id: "MID123",
            config: &config,
        };
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let req = KiroRequest::GenerateAssistant {
            body,
            stream: true,
            model: Some("claude-3-5-sonnet"),
        };
        let built = endpoint
            .build_request(&client, &ctx, &req)
            .expect("build ok")
            .build()
            .expect("to request");
        assert_eq!(built.method(), reqwest::Method::POST);
        assert!(built.url().as_str().contains("generateAssistantResponse"));
        assert_eq!(
            built.headers().get("Authorization").unwrap(),
            "Bearer T0KEN"
        );
        // profile_arn 已注入 body
        let sent_body_bytes = built
            .body()
            .unwrap()
            .as_bytes()
            .expect("bytes body");
        let sent_body: Value = serde_json::from_slice(sent_body_bytes).unwrap();
        assert_eq!(
            sent_body["profileArn"],
            "arn:aws:codewhisperer:us-east-1:123:profile/ABC"
        );
    }

    #[test]
    fn test_build_request_mcp_url_and_auth() {
        let endpoint = IdeEndpoint::new();
        let client = reqwest::Client::new();
        let creds = sample_credentials_no_arn();
        let config = sample_config();
        let ctx = RequestContext {
            credentials: &creds,
            token: "TK",
            machine_id: "MID",
            config: &config,
        };
        let body = r#"{"tool":"web_search"}"#;
        let req = KiroRequest::Mcp { body };
        let built = endpoint
            .build_request(&client, &ctx, &req)
            .expect("build ok")
            .build()
            .expect("to request");
        assert_eq!(built.method(), reqwest::Method::POST);
        assert!(built.url().path().ends_with("/mcp"));
        assert_eq!(built.headers().get("Authorization").unwrap(), "Bearer TK");
    }

    #[test]
    fn test_build_request_usage_limits_matches_legacy_format() {
        let endpoint = IdeEndpoint::new();
        let client = reqwest::Client::new();
        let creds = sample_credentials_with_arn();
        let config = sample_config();
        let ctx = RequestContext {
            credentials: &creds,
            token: "TK",
            machine_id: "MID",
            config: &config,
        };
        let req = KiroRequest::UsageLimits;
        let built = endpoint
            .build_request(&client, &ctx, &req)
            .expect("build ok")
            .build()
            .expect("to request");
        assert_eq!(built.method(), reqwest::Method::GET);
        // 与历史 token_manager::get_usage_limits 构造的 URL 一致（含 origin / resourceType / profileArn）
        let url = built.url().as_str();
        assert!(url.contains("/getUsageLimits"));
        assert!(url.contains("origin=AI_EDITOR"));
        assert!(url.contains("resourceType=AGENTIC_REQUEST"));
        assert!(url.contains("profileArn=arn%3A"));
        // headers 使用 codewhispererruntime 变体（区别于 generate/mcp 的 streaming）
        let ua = built
            .headers()
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ua.contains("codewhispererruntime"));
        assert!(ua.contains("m/N,E"));
        let amz_ua = built
            .headers()
            .get("x-amz-user-agent")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(amz_ua.starts_with("aws-sdk-js/1.0.0 "));
        assert_eq!(
            built
                .headers()
                .get("amz-sdk-request")
                .unwrap()
                .to_str()
                .unwrap(),
            "attempt=1; max=1"
        );
    }

    #[test]
    fn test_build_request_api_key_credential_sets_tokentype_header() {
        let endpoint = IdeEndpoint::new();
        let client = reqwest::Client::new();
        let mut creds = sample_credentials_no_arn();
        creds.auth_method = Some("api_key".to_string());
        creds.kiro_api_key = Some("kk-test".to_string());
        let config = sample_config();
        let ctx = RequestContext {
            credentials: &creds,
            token: "kk-test",
            machine_id: "MID",
            config: &config,
        };
        for req in [
            KiroRequest::GenerateAssistant {
                body: "{}",
                stream: false,
                model: None,
            },
            KiroRequest::Mcp { body: "{}" },
            KiroRequest::UsageLimits,
        ] {
            let built = endpoint
                .build_request(&client, &ctx, &req)
                .expect("build ok")
                .build()
                .expect("to request");
            assert_eq!(
                built.headers().get("tokentype").unwrap(),
                "API_KEY",
                "tokentype header missing for variant"
            );
        }
    }


    #[test]
    fn test_inject_profile_arn_with_some() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let arn = Some("arn:aws:codewhisperer:us-east-1:123:profile/ABC".to_string());
        let result = inject_profile_arn(body, &arn);
        let json: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            json["profileArn"],
            "arn:aws:codewhisperer:us-east-1:123:profile/ABC"
        );
        assert_eq!(json["conversationState"]["conversationId"], "c1");
    }

    #[test]
    fn test_inject_profile_arn_with_none() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let result = inject_profile_arn(body, &None);
        let json: Value = serde_json::from_str(&result).unwrap();
        assert!(json.get("profileArn").is_none());
        assert_eq!(json["conversationState"]["conversationId"], "c1");
    }

    #[test]
    fn test_inject_profile_arn_overwrites_existing() {
        let body = r#"{"conversationState":{},"profileArn":"old-arn"}"#;
        let arn = Some("new-arn".to_string());
        let result = inject_profile_arn(body, &arn);
        let json: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["profileArn"], "new-arn");
    }

    #[test]
    fn test_inject_profile_arn_invalid_json() {
        let body = "not-valid-json";
        let arn = Some("arn:test".to_string());
        let result = inject_profile_arn(body, &arn);
        assert_eq!(result, "not-valid-json");
    }
}
