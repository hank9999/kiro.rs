//! 邮件通知模块
//!
//! 在凭据被自动禁用时发送邮件通知

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use parking_lot::RwLock;

use crate::model::config::EmailConfig;

/// 凭据禁用原因
pub enum DisableReason {
    /// 连续失败达到阈值
    TooManyFailures {
        credential_id: u64,
        failure_count: u32,
    },
    /// 额度已用尽
    QuotaExhausted { credential_id: u64 },
}

/// 邮件通知服务
///
/// 即使未配置邮件也可创建实例，支持运行时热更新配置
pub struct NotificationService {
    config: RwLock<Option<EmailConfig>>,
}

impl NotificationService {
    pub fn new(config: Option<EmailConfig>) -> Self {
        Self {
            config: RwLock::new(config),
        }
    }

    /// 更新邮件配置（运行时热更新）
    pub fn update_config(&self, config: Option<EmailConfig>) {
        *self.config.write() = config;
    }

    /// 获取当前邮件配置的克隆
    pub fn get_config(&self) -> Option<EmailConfig> {
        self.config.read().clone()
    }

    /// 通知凭据被禁用（fire-and-forget）
    pub fn notify_credential_disabled(&self, reason: DisableReason) {
        let config = match self.config.read().clone() {
            Some(c) => c,
            None => return, // 未配置邮件，静默忽略
        };

        let (subject, body) = match &reason {
            DisableReason::TooManyFailures {
                credential_id,
                failure_count,
            } => (
                format!("[kiro-rs] 凭据 #{} 已被自动禁用（连续失败）", credential_id),
                format!(
                    "凭据 #{} 已连续失败 {} 次，已被自动禁用。\n\n\
                     请检查该凭据状态，必要时通过 Admin API 重置。",
                    credential_id, failure_count
                ),
            ),
            DisableReason::QuotaExhausted { credential_id } => (
                format!("[kiro-rs] 凭据 #{} 已被自动禁用（额度用尽）", credential_id),
                format!(
                    "凭据 #{} 的月度请求额度已用尽（MONTHLY_REQUEST_COUNT），已被自动禁用。\n\n\
                     请等待额度重置或通过 Admin API 切换到其他凭据。",
                    credential_id
                ),
            ),
        };

        tokio::spawn(async move {
            if let Err(e) = send_email(&config, &subject, &body).await {
                tracing::warn!("发送通知邮件失败: {}", e);
            }
        });
    }

    /// 发送测试邮件（同步等待结果）
    pub async fn send_test_email(&self) -> anyhow::Result<()> {
        let config = self
            .config
            .read()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("邮件通知未配置"))?;

        send_email(
            &config,
            "[kiro-rs] 测试邮件",
            "这是一封来自 kiro-rs 的测试邮件。\n\n如果您收到此邮件，说明邮件通知功能配置正确。",
        )
        .await
    }
}

/// 发送邮件到所有收件人
async fn send_email(config: &EmailConfig, subject: &str, body: &str) -> anyhow::Result<()> {
    let from: Mailbox = config
        .from_address
        .parse()
        .map_err(|e| anyhow::anyhow!("无效的发件人地址 '{}': {}", config.from_address, e))?;

    let creds = Credentials::new(config.smtp_username.clone(), config.smtp_password.clone());

    let mailer: AsyncSmtpTransport<Tokio1Executor> =
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)?
            .port(config.smtp_port)
            .credentials(creds)
            .build();

    for recipient in &config.recipients {
        let to: Mailbox = recipient
            .parse()
            .map_err(|e| anyhow::anyhow!("无效的收件人地址 '{}': {}", recipient, e))?;

        let email = Message::builder()
            .from(from.clone())
            .to(to)
            .subject(subject)
            .body(body.to_string())
            .map_err(|e| anyhow::anyhow!("构建邮件失败: {}", e))?;

        mailer
            .send(email)
            .await
            .map_err(|e| anyhow::anyhow!("发送邮件到 {} 失败: {}", recipient, e))?;
    }

    tracing::info!("邮件已发送: {} -> {:?}", subject, config.recipients);
    Ok(())
}