//! 邮件通知模块
//!
//! 当凭据因连续失败或额度用尽被自动禁用时，发送邮件通知管理员。
//! 使用 mpsc channel 解耦同步调用与异步邮件发送。

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tokio::sync::mpsc;

use crate::model::config::EmailConfig;

/// 凭据禁用事件
#[derive(Debug, Clone)]
pub struct CredentialDisabledEvent {
    /// 凭据 ID
    pub id: u64,
    /// 凭据邮箱（如果有）
    pub email: Option<String>,
    /// 禁用原因
    pub reason: DisableReason,
    /// 剩余可用凭据数
    pub remaining_available: usize,
    /// 凭据总数
    pub total: usize,
}

/// 禁用原因
#[derive(Debug, Clone, Copy)]
pub enum DisableReason {
    /// 连续失败达到阈值
    TooManyFailures,
    /// 额度用尽
    QuotaExceeded,
}

/// 邮件通知器
///
/// 通过 mpsc channel 异步发送邮件，可从同步代码安全调用。
/// 使用有界 channel（容量 64）防止 SMTP 故障时无限堆积。
pub struct EmailNotifier {
    sender: mpsc::Sender<CredentialDisabledEvent>,
}

/// 邮件发送最大重试次数
const MAX_SEND_RETRIES: u32 = 3;

impl EmailNotifier {
    /// 创建新的邮件通知器，启动后台消费任务
    pub fn new(config: EmailConfig) -> Self {
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(Self::consume_loop(config, rx));
        Self { sender: tx }
    }

    /// 发送凭据禁用通知（非阻塞，可从同步代码调用）
    pub fn notify(&self, event: CredentialDisabledEvent) {
        if let Err(e) = self.sender.try_send(event) {
            tracing::warn!("发送邮件通知失败（channel 已满或已关闭）: {}", e);
        }
    }

    /// 后台消费循环
    async fn consume_loop(config: EmailConfig, mut rx: mpsc::Receiver<CredentialDisabledEvent>) {
        while let Some(event) = rx.recv().await {
            // 带重试的邮件发送
            let mut last_err = None;
            for attempt in 1..=MAX_SEND_RETRIES {
                match Self::send_email(&config, &event).await {
                    Ok(()) => {
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "发送凭据禁用通知邮件失败（第 {}/{} 次）: {}",
                            attempt,
                            MAX_SEND_RETRIES,
                            e
                        );
                        last_err = Some(e);
                        if attempt < MAX_SEND_RETRIES {
                            tokio::time::sleep(std::time::Duration::from_secs(
                                2u64.pow(attempt - 1),
                            ))
                            .await;
                        }
                    }
                }
            }
            if let Some(e) = last_err {
                tracing::error!(
                    "凭据 #{} 禁用通知邮件发送失败（已重试 {} 次）: {}",
                    event.id,
                    MAX_SEND_RETRIES,
                    e
                );
            }
        }
        tracing::debug!("邮件通知消费循环已退出");
    }

    /// 发送凭据禁用通知邮件
    async fn send_email(
        config: &EmailConfig,
        event: &CredentialDisabledEvent,
    ) -> anyhow::Result<()> {
        let (subject, body) = Self::build_content(event);
        Self::send_raw(config, &subject, &body).await
    }

    /// 发送测试邮件（静态方法，用于 Admin API 测试端点）
    pub async fn send_test(config: &EmailConfig) -> anyhow::Result<()> {
        let subject = "[Kiro Alert] 测试邮件".to_string();
        let body =
            "这是一封来自 Kiro 的测试邮件。\n\n如果您收到此邮件，说明 SMTP 配置正确。".to_string();
        Self::send_raw(config, &subject, &body).await
    }

    /// 底层邮件发送
    async fn send_raw(config: &EmailConfig, subject: &str, body: &str) -> anyhow::Result<()> {
        use anyhow::Context;

        // 构建收件人列表
        if config.to_addresses.is_empty() {
            anyhow::bail!("收件人列表为空");
        }

        let mailer = Self::build_transport(config)?;

        for to_addr in &config.to_addresses {
            let email = Message::builder()
                .from(config.from_address.parse().context("发件人地址格式无效")?)
                .to(to_addr
                    .parse()
                    .with_context(|| format!("收件人地址格式无效: {}", to_addr))?)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body.to_string())
                .context("构建邮件失败")?;

            mailer
                .send(email)
                .await
                .with_context(|| format!("发送邮件到 {} 失败", to_addr))?;
        }

        tracing::info!("邮件通知已发送到 {} 个收件人", config.to_addresses.len());
        Ok(())
    }

    /// 构建 SMTP 传输
    fn build_transport(config: &EmailConfig) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
        use anyhow::Context;

        let creds = Credentials::new(config.smtp_username.clone(), config.smtp_password.clone());

        let builder = if config.smtp_tls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
                .context("创建 STARTTLS SMTP 传输失败")?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
        };

        Ok(builder.port(config.smtp_port).credentials(creds).build())
    }

    /// 根据事件构建邮件标题和正文
    fn build_content(event: &CredentialDisabledEvent) -> (String, String) {
        let email_display = event.email.as_deref().unwrap_or("未知");

        match event.reason {
            DisableReason::TooManyFailures => {
                let subject = format!("[Kiro Alert] 凭据 #{} 连续失败已被禁用", event.id);
                let body = format!(
                    "凭据 #{id}（{email}）因连续 API 调用失败已被自动禁用。\n\n\
                     禁用原因：连续失败达到阈值\n\
                     剩余可用凭据：{available}/{total}\n\n\
                     请登录 Admin 面板检查凭据状态。",
                    id = event.id,
                    email = email_display,
                    available = event.remaining_available,
                    total = event.total,
                );
                (subject, body)
            }
            DisableReason::QuotaExceeded => {
                let subject = format!("[Kiro Alert] 凭据 #{} 额度用尽已被禁用", event.id);
                let body = format!(
                    "凭据 #{id}（{email}）因额度用尽已被自动禁用。\n\n\
                     禁用原因：月度请求额度已用尽（MONTHLY_REQUEST_COUNT）\n\
                     剩余可用凭据：{available}/{total}\n\n\
                     请登录 Admin 面板检查凭据状态，或等待额度重置。",
                    id = event.id,
                    email = email_display,
                    available = event.remaining_available,
                    total = event.total,
                );
                (subject, body)
            }
        }
    }
}
