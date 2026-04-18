//! Kiro 端点抽象
//!
//! 不同 Kiro 端点（如 `ide` / `cli`）在 URL、请求头、请求体上存在差异，
//! 但共享凭据池、Token 刷新、重试逻辑和 AWS event-stream 响应解码。
//!
//! [`KiroEndpoint`] 抽象了请求侧的差异点；`KiroProvider` 持有一个 endpoint 注册表，
//! 按凭据的 `endpoint` 字段选择对应实现。

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::{Client, RequestBuilder};

use crate::kiro::model::credentials::KiroCredentials;
use crate::model::config::Config;

pub mod ide;

pub use ide::IdeEndpoint;

/// 端点名 → 实现的注册表 + 默认端点名
///
/// 统一承载原先散落在 `main.rs` / `KiroProvider.endpoints` / `AdminService.known_endpoints`
/// 的三份副本。Provider 重试循环、AdminService 校验、TokenManager 预解析 CallContext
/// 均从同一个 `Arc<EndpointRegistry>` 取用。
pub struct EndpointRegistry {
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    default: String,
}

impl EndpointRegistry {
    /// 构造注册表，若 `default` 未在 `endpoints` 中会失败
    pub fn new(
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default: String,
    ) -> anyhow::Result<Self> {
        if !endpoints.contains_key(&default) {
            anyhow::bail!(
                "默认端点 \"{}\" 未注册（已注册: {:?}）",
                default,
                endpoints.keys().collect::<Vec<_>>()
            );
        }
        Ok(Self { endpoints, default })
    }

    /// 根据凭据 `endpoint` 字段解析端点实现
    ///
    /// - 凭据指定 endpoint 且已注册 → 对应实现
    /// - 凭据未指定 → 默认端点
    /// - 凭据指定但未注册 → 回退到默认端点并 warn（保持运行时路由不中断；
    ///   启动时由 main.rs 做严格校验保证凭据一致性）
    pub fn resolve(&self, credentials: &KiroCredentials) -> Arc<dyn KiroEndpoint> {
        let name = credentials.endpoint.as_deref().unwrap_or(&self.default);
        if let Some(ep) = self.endpoints.get(name) {
            return Arc::clone(ep);
        }
        tracing::warn!(
            "凭据指定端点 \"{}\" 未注册，回退到默认 \"{}\"",
            name,
            self.default
        );
        Arc::clone(
            self.endpoints
                .get(&self.default)
                .expect("默认端点在 new() 中已校验存在"),
        )
    }

    /// 判断是否存在指定名称的端点
    pub fn contains(&self, name: &str) -> bool {
        self.endpoints.contains_key(name)
    }

    /// 所有已注册的端点名称（顺序未定义）
    pub fn names(&self) -> Vec<&str> {
        self.endpoints.keys().map(|s| s.as_str()).collect()
    }
}

/// Kiro 端点上的单次请求类型
///
/// 上层统一通过 [`KiroEndpoint::build_request`] 构造 `RequestBuilder`，
/// 由 endpoint 负责根据变体决定 URL / method / body 变换 / header 装饰。
///
/// 注：`stream` 与 `model` 字段在阶段 1 仅占位，Provider 将在阶段 4 读取它们。
#[allow(dead_code)]
pub enum KiroRequest<'a> {
    /// 生成式 Assistant 请求（流式或非流式，视 `stream` 字段）
    GenerateAssistant {
        body: &'a str,
        stream: bool,
        model: Option<&'a str>,
    },
    /// MCP 工具调用请求
    Mcp { body: &'a str },
    /// 获取使用额度（GET 请求，无请求体）
    UsageLimits,
}

/// Endpoint 层错误语义分类
///
/// Provider 重试循环根据分类决定：禁用凭据 / bail / 瞬态重试 / 强制刷新 token。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointErrorKind {
    /// 402 + 月度配额用尽标记：禁用凭据 + 故障转移
    MonthlyQuotaExhausted,
    /// 401/403 + bearer token 失效标记：强制刷新 token（每凭据一次机会）
    BearerTokenInvalid,
    /// 400 Bad Request：直接 bail，不重试、不计入失败
    BadRequest,
    /// 其他未分类 4xx（经 401/402/403 特殊分支处理后的剩余 4xx）：bail
    ClientError,
    /// 408/429/5xx：瞬态错误，sleep + 重试，不禁用
    Transient,
    /// 兜底：当作可重试瞬态错误
    Unknown,
}

/// Kiro 端点
///
/// 同一个 `KiroProvider` 可持有多个 endpoint 实现，按凭据级字段切换。
pub trait KiroEndpoint: Send + Sync {
    /// 端点名称（对应 credentials.endpoint / config.defaultEndpoint 的取值）
    fn name(&self) -> &'static str;

    /// 基于 `client` 构造一个未发送的 `RequestBuilder`
    ///
    /// 实现负责根据 `req` 变体确定 URL、method、请求体加工、所有 header（包括 Authorization、
    /// content-type、Connection、host、user-agent 等端点相关项）。
    ///
    /// 默认实现为 `unimplemented!`，具体端点必须 override。
    #[allow(dead_code)]
    fn build_request(
        &self,
        _client: &Client,
        _ctx: &RequestContext<'_>,
        _req: &KiroRequest<'_>,
    ) -> anyhow::Result<RequestBuilder> {
        unimplemented!("endpoint {} 未实现 build_request", self.name())
    }

    /// 根据上游响应 status + body 分类错误类型
    ///
    /// 默认实现覆盖 IDE 通用语义；有特殊错误格式的端点可 override。
    #[allow(dead_code)]
    fn classify_error(&self, status: u16, body: &str) -> EndpointErrorKind {
        if status == 402 && self.is_monthly_request_limit(body) {
            return EndpointErrorKind::MonthlyQuotaExhausted;
        }
        if matches!(status, 401 | 403) && self.is_bearer_token_invalid(body) {
            return EndpointErrorKind::BearerTokenInvalid;
        }
        if status == 400 {
            return EndpointErrorKind::BadRequest;
        }
        if matches!(status, 408 | 429) || (500..600).contains(&status) {
            return EndpointErrorKind::Transient;
        }
        if (400..500).contains(&status) {
            return EndpointErrorKind::ClientError;
        }
        EndpointErrorKind::Unknown
    }

    /// API endpoint URL
    fn api_url(&self, ctx: &RequestContext<'_>) -> String;

    /// MCP endpoint URL
    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String;

    /// 装饰 API 请求的端点特有 header
    ///
    /// Provider 已经设置好 URL、content-type、Connection 和 body；
    /// 实现负责追加 Authorization、host、user-agent 等端点相关头。
    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder;

    /// 装饰 MCP 请求的端点特有 header
    fn decorate_mcp(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder;

    /// 对已序列化的 API 请求体做端点特有加工（如注入 profileArn）
    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> String;

    /// 对已序列化的 MCP 请求体做端点特有加工（默认不变）
    fn transform_mcp_body(&self, body: &str, _ctx: &RequestContext<'_>) -> String {
        body.to_string()
    }

    /// 判断响应体是否表示"月度配额用尽"（禁用凭据并转移）
    fn is_monthly_request_limit(&self, body: &str) -> bool {
        default_is_monthly_request_limit(body)
    }

    /// 判断响应体是否表示"上游 bearer token 失效"（触发强制刷新）
    fn is_bearer_token_invalid(&self, body: &str) -> bool {
        default_is_bearer_token_invalid(body)
    }
}

/// 装饰请求时可用的上下文
///
/// 包含单次调用已确定的所有运行时信息。引用形式避免无谓 clone。
pub struct RequestContext<'a> {
    /// 当前凭据
    pub credentials: &'a KiroCredentials,
    /// 有效的 access token（API Key 凭据下即 kiroApiKey）
    pub token: &'a str,
    /// 当前凭据对应的 machineId
    pub machine_id: &'a str,
    /// 全局配置
    pub config: &'a Config,
}

/// 默认的 MONTHLY_REQUEST_COUNT 判断逻辑
///
/// 同时识别顶层 `reason` 字段和嵌套 `error.reason` 字段。
pub fn default_is_monthly_request_limit(body: &str) -> bool {
    if body.contains("MONTHLY_REQUEST_COUNT") {
        return true;
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };

    if value
        .get("reason")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
    {
        return true;
    }

    value
        .pointer("/error/reason")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
}

/// 默认的 bearer token 失效判断逻辑
pub fn default_is_bearer_token_invalid(body: &str) -> bool {
    body.contains("The bearer token included in the request is invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_monthly_request_limit_detects_reason() {
        let body = r#"{"message":"You have reached the limit.","reason":"MONTHLY_REQUEST_COUNT"}"#;
        assert!(default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_default_monthly_request_limit_nested_reason() {
        let body = r#"{"error":{"reason":"MONTHLY_REQUEST_COUNT"}}"#;
        assert!(default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_default_monthly_request_limit_false() {
        let body = r#"{"message":"nope","reason":"DAILY_REQUEST_COUNT"}"#;
        assert!(!default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_default_bearer_token_invalid() {
        assert!(default_is_bearer_token_invalid(
            "The bearer token included in the request is invalid"
        ));
        assert!(!default_is_bearer_token_invalid("unrelated error"));
    }

    /// 仅依赖默认实现的探针端点，专门用来覆盖 trait 的默认 `classify_error`
    struct ProbeEndpoint;

    impl KiroEndpoint for ProbeEndpoint {
        fn name(&self) -> &'static str {
            "probe"
        }
        fn api_url(&self, _ctx: &RequestContext<'_>) -> String {
            String::new()
        }
        fn mcp_url(&self, _ctx: &RequestContext<'_>) -> String {
            String::new()
        }
        fn decorate_api(&self, req: RequestBuilder, _ctx: &RequestContext<'_>) -> RequestBuilder {
            req
        }
        fn decorate_mcp(&self, req: RequestBuilder, _ctx: &RequestContext<'_>) -> RequestBuilder {
            req
        }
        fn transform_api_body(&self, body: &str, _ctx: &RequestContext<'_>) -> String {
            body.to_string()
        }
    }

    #[test]
    fn test_classify_error_monthly_quota_exhausted() {
        let probe = ProbeEndpoint;
        let body = r#"{"reason":"MONTHLY_REQUEST_COUNT"}"#;
        assert_eq!(
            probe.classify_error(402, body),
            EndpointErrorKind::MonthlyQuotaExhausted
        );
    }

    #[test]
    fn test_classify_error_bearer_token_invalid_401() {
        let probe = ProbeEndpoint;
        let body = "The bearer token included in the request is invalid";
        assert_eq!(
            probe.classify_error(401, body),
            EndpointErrorKind::BearerTokenInvalid
        );
    }

    #[test]
    fn test_classify_error_bearer_token_invalid_403() {
        let probe = ProbeEndpoint;
        let body = "The bearer token included in the request is invalid";
        assert_eq!(
            probe.classify_error(403, body),
            EndpointErrorKind::BearerTokenInvalid
        );
    }

    #[test]
    fn test_classify_error_bad_request() {
        let probe = ProbeEndpoint;
        assert_eq!(
            probe.classify_error(400, "{}"),
            EndpointErrorKind::BadRequest
        );
    }

    #[test]
    fn test_classify_error_transient_429_500() {
        let probe = ProbeEndpoint;
        assert_eq!(probe.classify_error(429, "{}"), EndpointErrorKind::Transient);
        assert_eq!(probe.classify_error(408, "{}"), EndpointErrorKind::Transient);
        assert_eq!(probe.classify_error(500, "{}"), EndpointErrorKind::Transient);
        assert_eq!(probe.classify_error(502, "{}"), EndpointErrorKind::Transient);
        assert_eq!(probe.classify_error(599, "{}"), EndpointErrorKind::Transient);
    }

    #[test]
    fn test_classify_error_402_without_marker_is_client_error() {
        let probe = ProbeEndpoint;
        assert_eq!(
            probe.classify_error(402, "{}"),
            EndpointErrorKind::ClientError
        );
    }

    #[test]
    fn test_classify_error_404_is_client_error() {
        let probe = ProbeEndpoint;
        assert_eq!(
            probe.classify_error(404, "{}"),
            EndpointErrorKind::ClientError
        );
    }

    #[test]
    fn test_classify_error_401_without_marker_is_client_error() {
        // 401 但 body 不含 bearer 失效标记 → 走普通 4xx arm
        let probe = ProbeEndpoint;
        assert_eq!(
            probe.classify_error(401, "{}"),
            EndpointErrorKind::ClientError
        );
    }

    #[test]
    fn test_classify_error_200_is_unknown() {
        let probe = ProbeEndpoint;
        assert_eq!(probe.classify_error(200, "{}"), EndpointErrorKind::Unknown);
    }

    #[test]
    fn test_kiro_request_variants_constructible() {
        let _g = KiroRequest::GenerateAssistant {
            body: "{}",
            stream: true,
            model: Some("claude"),
        };
        let _m = KiroRequest::Mcp { body: "{}" };
        let _u = KiroRequest::UsageLimits;
    }

    struct NamedProbeEndpoint(&'static str);
    impl KiroEndpoint for NamedProbeEndpoint {
        fn name(&self) -> &'static str {
            self.0
        }
        fn api_url(&self, _ctx: &RequestContext<'_>) -> String {
            String::new()
        }
        fn mcp_url(&self, _ctx: &RequestContext<'_>) -> String {
            String::new()
        }
        fn decorate_api(&self, req: RequestBuilder, _ctx: &RequestContext<'_>) -> RequestBuilder {
            req
        }
        fn decorate_mcp(&self, req: RequestBuilder, _ctx: &RequestContext<'_>) -> RequestBuilder {
            req
        }
        fn transform_api_body(&self, body: &str, _ctx: &RequestContext<'_>) -> String {
            body.to_string()
        }
    }

    fn registry_with(names: &[&'static str], default: &str) -> HashMap<String, Arc<dyn KiroEndpoint>> {
        let mut map: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        for name in names {
            map.insert((*name).to_string(), Arc::new(NamedProbeEndpoint(name)));
        }
        // default 参数只是为了签名一致，不在此处校验
        let _ = default;
        map
    }

    #[test]
    fn test_registry_new_rejects_missing_default() {
        let endpoints = registry_with(&["ide"], "cli");
        assert!(EndpointRegistry::new(endpoints, "cli".to_string()).is_err());
    }

    #[test]
    fn test_registry_new_accepts_present_default() {
        let endpoints = registry_with(&["ide"], "ide");
        assert!(EndpointRegistry::new(endpoints, "ide".to_string()).is_ok());
    }

    #[test]
    fn test_registry_resolve_explicit() {
        let endpoints = registry_with(&["ide"], "ide");
        let reg = EndpointRegistry::new(endpoints, "ide".to_string()).unwrap();
        let creds = KiroCredentials {
            endpoint: Some("ide".to_string()),
            ..Default::default()
        };
        assert_eq!(reg.resolve(&creds).name(), "ide");
    }

    #[test]
    fn test_registry_resolve_default_when_none() {
        let endpoints = registry_with(&["ide"], "ide");
        let reg = EndpointRegistry::new(endpoints, "ide".to_string()).unwrap();
        let creds = KiroCredentials {
            endpoint: None,
            ..Default::default()
        };
        assert_eq!(reg.resolve(&creds).name(), "ide");
    }

    #[test]
    fn test_registry_resolve_unknown_falls_back_to_default() {
        let endpoints = registry_with(&["ide"], "ide");
        let reg = EndpointRegistry::new(endpoints, "ide".to_string()).unwrap();
        let creds = KiroCredentials {
            endpoint: Some("nonexistent".to_string()),
            ..Default::default()
        };
        // 不应 panic，返回默认
        assert_eq!(reg.resolve(&creds).name(), "ide");
    }

    #[test]
    fn test_registry_contains_and_names() {
        let endpoints = registry_with(&["ide"], "ide");
        let reg = EndpointRegistry::new(endpoints, "ide".to_string()).unwrap();
        assert!(reg.contains("ide"));
        assert!(!reg.contains("cli"));
        let mut names = reg.names();
        names.sort();
        assert_eq!(names, vec!["ide"]);
    }

    #[test]
    fn test_endpoint_error_kind_debug() {
        assert!(format!("{:?}", EndpointErrorKind::MonthlyQuotaExhausted).contains("Monthly"));
        assert!(format!("{:?}", EndpointErrorKind::BearerTokenInvalid).contains("Bearer"));
        assert!(format!("{:?}", EndpointErrorKind::BadRequest).contains("Bad"));
        assert!(format!("{:?}", EndpointErrorKind::ClientError).contains("Client"));
        assert!(format!("{:?}", EndpointErrorKind::Transient).contains("Transient"));
        assert!(format!("{:?}", EndpointErrorKind::Unknown).contains("Unknown"));
    }
}
