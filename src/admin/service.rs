//! Admin API 业务逻辑服务

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};

use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::requests::conversation::{
    ConversationState, CurrentMessage, UserInputMessage,
};
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::provider::KiroProvider;
use crate::kiro::token_manager::MultiTokenManager;

use super::error::AdminServiceError;
use super::types::{
    AddCredentialRequest, AddCredentialResponse, BalanceResponse, CredentialStatusItem,
    CredentialValidationResult, CredentialsStatusResponse, ValidateCredentialsRequest,
    ValidateCredentialsResponse, ValidationStatus, ValidationSummary,
};

/// Admin 服务
///
/// 封装所有 Admin API 的业务逻辑
pub struct AdminService {
    token_manager: Arc<MultiTokenManager>,
    provider: Arc<KiroProvider>,
}

impl AdminService {
    pub fn new(token_manager: Arc<MultiTokenManager>, provider: Arc<KiroProvider>) -> Self {
        Self {
            token_manager,
            provider,
        }
    }

    /// 获取所有凭据状态
    pub fn get_all_credentials(&self) -> CredentialsStatusResponse {
        let snapshot = self.token_manager.snapshot();

        let mut credentials: Vec<CredentialStatusItem> = snapshot
            .entries
            .into_iter()
            .map(|entry| CredentialStatusItem {
                id: entry.id,
                priority: entry.priority,
                disabled: entry.disabled,
                failure_count: entry.failure_count,
                is_current: entry.id == snapshot.current_id,
                expires_at: entry.expires_at,
                auth_method: entry.auth_method,
                has_profile_arn: entry.has_profile_arn,
            })
            .collect();

        // 按优先级排序（数字越小优先级越高）
        credentials.sort_by_key(|c| c.priority);

        CredentialsStatusResponse {
            total: snapshot.total,
            available: snapshot.available,
            current_id: snapshot.current_id,
            credentials,
        }
    }

    /// 设置凭据禁用状态
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<(), AdminServiceError> {
        // 先获取当前凭据 ID，用于判断是否需要切换
        let snapshot = self.token_manager.snapshot();
        let current_id = snapshot.current_id;

        self.token_manager
            .set_disabled(id, disabled)
            .map_err(|e| self.classify_error(e, id))?;

        // 只有禁用的是当前凭据时才尝试切换到下一个
        if disabled && id == current_id {
            let _ = self.token_manager.switch_to_next();
        }
        Ok(())
    }

    /// 设置凭据优先级
    pub fn set_priority(&self, id: u64, priority: u32) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_priority(id, priority)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 重置失败计数并重新启用
    pub fn reset_and_enable(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .reset_and_enable(id)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 获取凭据余额
    pub async fn get_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        let usage = self
            .token_manager
            .get_usage_limits_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        let current_usage = usage.current_usage();
        let usage_limit = usage.usage_limit();
        let remaining = (usage_limit - current_usage).max(0.0);
        let usage_percentage = if usage_limit > 0.0 {
            (current_usage / usage_limit * 100.0).min(100.0)
        } else {
            0.0
        };

        Ok(BalanceResponse {
            id,
            subscription_title: usage.subscription_title().map(|s| s.to_string()),
            current_usage,
            usage_limit,
            remaining,
            usage_percentage,
            next_reset_at: usage.next_date_reset,
        })
    }

    /// 添加新凭据
    pub async fn add_credential(
        &self,
        req: AddCredentialRequest,
    ) -> Result<AddCredentialResponse, AdminServiceError> {
        // 构建凭据对象
        let new_cred = KiroCredentials {
            id: None,
            access_token: None,
            refresh_token: Some(req.refresh_token),
            profile_arn: None,
            expires_at: None,
            auth_method: Some(req.auth_method),
            client_id: req.client_id,
            client_secret: req.client_secret,
            priority: req.priority,
            region: req.region,
            machine_id: req.machine_id,
        };

        // 调用 token_manager 添加凭据
        let credential_id = self
            .token_manager
            .add_credential(new_cred)
            .await
            .map_err(|e| self.classify_add_error(e))?;

        Ok(AddCredentialResponse {
            success: true,
            message: format!("凭据添加成功，ID: {}", credential_id),
            credential_id,
        })
    }

    /// 删除凭据
    pub fn delete_credential(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .delete_credential(id)
            .map_err(|e| self.classify_delete_error(e, id))
    }

    /// 分类简单操作错误（set_disabled, set_priority, reset_and_enable）
    fn classify_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类余额查询错误（可能涉及上游 API 调用）
    fn classify_balance_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();

        // 1. 凭据不存在
        if msg.contains("不存在") {
            return AdminServiceError::NotFound { id };
        }

        // 2. 上游服务错误特征：HTTP 响应错误或网络错误
        let is_upstream_error =
            // HTTP 响应错误（来自 refresh_*_token 的错误消息）
            msg.contains("凭证已过期或无效") ||
            msg.contains("权限不足") ||
            msg.contains("已被限流") ||
            msg.contains("服务器错误") ||
            msg.contains("Token 刷新失败") ||
            msg.contains("暂时不可用") ||
            // 网络错误（reqwest 错误）
            msg.contains("error trying to connect") ||
            msg.contains("connection") ||
            msg.contains("timeout") ||
            msg.contains("timed out");

        if is_upstream_error {
            AdminServiceError::UpstreamError(msg)
        } else {
            // 3. 默认归类为内部错误（本地验证失败、配置错误等）
            // 包括：缺少 refreshToken、refreshToken 已被截断、无法生成 machineId 等
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类添加凭据错误
    fn classify_add_error(&self, e: anyhow::Error) -> AdminServiceError {
        let msg = e.to_string();

        // 凭据验证失败（refreshToken 无效、格式错误等）
        let is_invalid_credential = msg.contains("缺少 refreshToken")
            || msg.contains("refreshToken 为空")
            || msg.contains("refreshToken 已被截断")
            || msg.contains("凭证已过期或无效")
            || msg.contains("权限不足")
            || msg.contains("已被限流");

        if is_invalid_credential {
            AdminServiceError::InvalidCredential(msg)
        } else if msg.contains("error trying to connect")
            || msg.contains("connection")
            || msg.contains("timeout")
        {
            AdminServiceError::UpstreamError(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类删除凭据错误
    fn classify_delete_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else if msg.contains("只能删除已禁用的凭据") {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 批量验证凭据
    ///
    /// 对指定的凭据列表发送最小化 API 请求，验证凭据是否可用。
    /// 不影响凭据的失败计数和禁用状态。
    pub async fn validate_credentials(
        &self,
        req: ValidateCredentialsRequest,
    ) -> Result<ValidateCredentialsResponse, AdminServiceError> {
        let model_id = Self::map_validate_model(&req.model);
        let timeout = Duration::from_millis(req.timeout_ms);
        let max_concurrency = req.max_concurrency.max(1).min(10);

        // 构建最小化请求体
        let request_body = Self::build_minimal_request(&model_id);

        // 并发验证
        let results: Vec<CredentialValidationResult> = stream::iter(req.credential_ids)
            .map(|id| {
                let request_body = request_body.clone();
                let timeout = timeout;
                async move {
                    self.validate_single_credential(id, &request_body, timeout)
                        .await
                }
            })
            .buffer_unordered(max_concurrency)
            .collect()
            .await;

        // 统计汇总
        let mut ok = 0;
        let mut denied = 0;
        let mut invalid = 0;
        let mut transient = 0;
        let mut not_found = 0;

        for result in &results {
            match result.status {
                ValidationStatus::Ok => ok += 1,
                ValidationStatus::Denied => denied += 1,
                ValidationStatus::Invalid => invalid += 1,
                ValidationStatus::Transient => transient += 1,
                ValidationStatus::NotFound => not_found += 1,
            }
        }

        Ok(ValidateCredentialsResponse {
            results,
            summary: ValidationSummary {
                total: ok + denied + invalid + transient + not_found,
                ok,
                denied,
                invalid,
                transient,
                not_found,
            },
        })
    }

    async fn validate_single_credential(
        &self,
        id: u64,
        request_body: &str,
        timeout: Duration,
    ) -> CredentialValidationResult {
        let start = Instant::now();

        let ctx = match self.token_manager.acquire_context_for(id).await {
            Ok(ctx) => ctx,
            Err(e) => {
                let msg = e.to_string();
                let status = if msg.contains("不存在") {
                    ValidationStatus::NotFound
                } else {
                    ValidationStatus::Invalid
                };
                return CredentialValidationResult {
                    id,
                    status,
                    message: Some(msg),
                    latency_ms: Some(start.elapsed().as_millis() as u64),
                };
            }
        };

        match self
            .provider
            .validate_credential(&ctx, request_body, timeout)
            .await
        {
            Ok(status_code) => {
                let (status, message) = Self::classify_status_code(status_code);
                CredentialValidationResult {
                    id,
                    status,
                    message,
                    latency_ms: Some(start.elapsed().as_millis() as u64),
                }
            }
            Err(e) => CredentialValidationResult {
                id,
                status: ValidationStatus::Transient,
                message: Some(e.to_string()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }

    fn classify_status_code(status_code: u16) -> (ValidationStatus, Option<String>) {
        match status_code {
            200..=299 => (ValidationStatus::Ok, None),
            400 => (ValidationStatus::Invalid, Some("请求格式错误".to_string())),
            401 | 403 => (ValidationStatus::Denied, Some("凭据被拒绝".to_string())),
            402 => (ValidationStatus::Denied, Some("额度已用尽".to_string())),
            408 | 429 => (
                ValidationStatus::Transient,
                Some(format!("服务暂时不可用 ({})", status_code)),
            ),
            500..=599 => (
                ValidationStatus::Transient,
                Some(format!("服务器错误 ({})", status_code)),
            ),
            _ => (
                ValidationStatus::Invalid,
                Some(format!("未知状态码 ({})", status_code)),
            ),
        }
    }

    fn map_validate_model(model: &str) -> String {
        match model.to_lowercase().as_str() {
            "opus" => "claude-opus-4.5".to_string(),
            "haiku" => "claude-haiku-4.5".to_string(),
            _ => "claude-sonnet-4.5".to_string(),
        }
    }

    fn build_minimal_request(model_id: &str) -> String {
        let state = ConversationState::new(uuid::Uuid::new_v4().to_string())
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(CurrentMessage::new(UserInputMessage::new("ping", model_id)));

        let request = KiroRequest {
            conversation_state: state,
            profile_arn: None,
        };

        serde_json::to_string(&request).unwrap_or_default()
    }
}
