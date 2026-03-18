//! Webhook 通知模块
//!
//! 凭据被禁用时，通过 HTTP POST 发送 JSON 通知到配置的 webhook URL。
//! 支持自定义 JSON 模板，通过变量占位符渲染最终 payload。
//! 复用 `CredentialDisabledEvent`，与邮件通知并行工作。

use serde::Serialize;
use tokio::sync::mpsc;

use crate::admin::email::{CredentialDisabledEvent, DisableReason};
use crate::http_client::{ProxyConfig, build_client};
use crate::model::config::TlsBackend;

/// 内置默认 payload（JSON 序列化后 POST 到 webhook URL）
#[derive(Debug, Clone, Serialize)]
pub struct DefaultPayload {
    pub event: &'static str,
    pub credential_id: u64,
    pub reason: &'static str,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub available_credentials: usize,
    pub total_credentials: usize,
}

/// 发送上下文（传入 consume_loop）
struct SendContext {
    url: String,
    /// 用户自定义 JSON 模板（None 时使用内置默认格式）
    template: Option<String>,
    client: reqwest::Client,
}

/// Webhook 通知器
///
/// 通过 mpsc channel 异步发送，可从同步代码安全调用。
pub struct WebhookNotifier {
    sender: mpsc::Sender<CredentialDisabledEvent>,
}

/// 可用的模板变量
pub const TEMPLATE_VARIABLES: &[(&str, &str)] = &[
    ("{{credential_id}}", "凭据 ID"),
    ("{{email}}", "凭据邮箱"),
    ("{{reason}}", "禁用原因（英文）"),
    ("{{reason_zh}}", "禁用原因（中文）"),
    ("{{available}}", "剩余可用凭据数"),
    ("{{total}}", "凭据总数"),
    ("{{timestamp}}", "事件时间（RFC3339）"),
];

impl WebhookNotifier {
    /// 创建新的 WebhookNotifier，启动后台消费任务
    pub fn new(
        url: String,
        template: Option<String>,
        proxy: Option<&ProxyConfig>,
        tls_backend: TlsBackend,
    ) -> anyhow::Result<Self> {
        let client = build_client(proxy, 10, tls_backend)?;
        let (tx, rx) = mpsc::channel(64);
        let ctx = SendContext {
            url,
            template,
            client,
        };
        tokio::spawn(Self::consume_loop(ctx, rx));
        Ok(Self { sender: tx })
    }

    /// 发送凭据禁用通知（非阻塞，可从同步代码调用）
    pub fn notify(&self, event: CredentialDisabledEvent) {
        if let Err(e) = self.sender.try_send(event) {
            tracing::warn!("Webhook 通知发送失败（channel 已满或已关闭）: {}", e);
        }
    }

    /// 后台消费循环
    async fn consume_loop(
        ctx: SendContext,
        mut rx: mpsc::Receiver<CredentialDisabledEvent>,
    ) {
        while let Some(event) = rx.recv().await {
            let body = Self::render_body(&ctx.template, &event);
            let request = ctx
                .client
                .post(&ctx.url)
                .header("Content-Type", "application/json")
                .body(body);

            match request.send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        tracing::warn!(
                            "Webhook 发送失败: HTTP {} (凭据 #{})",
                            resp.status(),
                            event.id
                        );
                    } else {
                        tracing::info!("Webhook 通知已发送: 凭据 #{} 已禁用", event.id);
                    }
                }
                Err(e) => {
                    tracing::warn!("Webhook 发送失败: {} (凭据 #{})", e, event.id);
                }
            }
        }
        tracing::debug!("Webhook 通知消费循环已退出");
    }

    /// 发送测试 webhook（用于验证 URL 和模板配置）
    ///
    /// 构造一个模拟的 `CredentialDisabledEvent`，渲染模板后 POST 到指定 URL。
    /// 同步等待响应，返回错误信息供前端展示。
    pub async fn send_test(
        url: &str,
        template: Option<String>,
        proxy: Option<&ProxyConfig>,
        tls_backend: TlsBackend,
    ) -> anyhow::Result<()> {
        let client = build_client(proxy, 10, tls_backend)?;

        // 构造模拟事件
        let event = CredentialDisabledEvent {
            id: 0,
            reason: DisableReason::TooManyFailures,
            email: Some("test@example.com".to_string()),
            remaining_available: 2,
            total: 3,
        };

        let body = Self::render_body(&template, &event);

        let resp = client
            .post(url)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("请求发送失败: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("HTTP {} — {}", status, resp_body);
        }

        Ok(())
    }

    /// 渲染 webhook body
    ///
    /// 有自定义模板时做变量替换，否则使用内置默认 JSON
    fn render_body(template: &Option<String>, event: &CredentialDisabledEvent) -> String {
        let reason_en = match event.reason {
            DisableReason::TooManyFailures => "too_many_failures",
            DisableReason::QuotaExceeded => "quota_exceeded",
        };
        let reason_zh = match event.reason {
            DisableReason::TooManyFailures => "连续失败达到阈值",
            DisableReason::QuotaExceeded => "额度用尽",
        };
        let timestamp = chrono::Utc::now().to_rfc3339();
        let email = event.email.as_deref().unwrap_or("");

        match template {
            Some(tpl) if !tpl.trim().is_empty() => {
                tpl.replace("{{credential_id}}", &event.id.to_string())
                    .replace("{{email}}", email)
                    .replace("{{reason}}", reason_en)
                    .replace("{{reason_zh}}", reason_zh)
                    .replace("{{available}}", &event.remaining_available.to_string())
                    .replace("{{total}}", &event.total.to_string())
                    .replace("{{timestamp}}", &timestamp)
            }
            _ => {
                // 内置默认格式
                let payload = DefaultPayload {
                    event: "credential_disabled",
                    credential_id: event.id,
                    reason: reason_en,
                    timestamp,
                    email: event.email.clone(),
                    available_credentials: event.remaining_available,
                    total_credentials: event.total,
                };
                serde_json::to_string(&payload).unwrap_or_default()
            }
        }
    }
}