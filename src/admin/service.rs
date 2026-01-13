//! Admin API 业务逻辑服务

use std::sync::Arc;

use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;
use crate::stats::{BucketStats, StatsStore};

use super::error::AdminServiceError;
use super::types::{
    AddCredentialRequest, AddCredentialResponse, BalanceResponse, CredentialStatsResponse,
    CredentialStatusItem, CredentialsStatusResponse, StatsBucket,
};

/// Admin 服务
///
/// 封装所有 Admin API 的业务逻辑
pub struct AdminService {
    token_manager: Arc<MultiTokenManager>,
    stats: Option<Arc<StatsStore>>,
}

impl AdminService {
    pub fn new(token_manager: Arc<MultiTokenManager>, stats: Option<Arc<StatsStore>>) -> Self {
        Self { token_manager, stats }
    }

    fn credential_exists(&self, id: u64) -> bool {
        let snapshot = self.token_manager.snapshot();
        snapshot.entries.iter().any(|e| e.id == id)
    }

    fn bucket_to_api(key: String, b: BucketStats) -> StatsBucket {
        StatsBucket {
            key,
            calls_total: b.calls_total,
            calls_ok: b.calls_ok,
            calls_err: b.calls_err,
            input_tokens_total: b.input_tokens_total,
            output_tokens_total: b.output_tokens_total,
            last_call_at: b.last_call_at,
            last_success_at: b.last_success_at,
            last_error_at: b.last_error_at,
            last_error: b.last_error,
        }
    }

    /// 获取所有凭据状态
    pub fn get_all_credentials(&self) -> CredentialsStatusResponse {
        let snapshot = self.token_manager.snapshot();

        let mut credentials: Vec<CredentialStatusItem> = snapshot
            .entries
            .into_iter()
            .map(|entry| {
                let stats = self
                    .stats
                    .as_ref()
                    .map(|s| s.get_or_default(entry.id));

                CredentialStatusItem {
                    id: entry.id,
                    priority: entry.priority,
                    disabled: entry.disabled,
                    failure_count: entry.failure_count,
                    is_current: entry.id == snapshot.current_id,
                    expires_at: entry.expires_at,
                    auth_method: entry.auth_method,
                    has_profile_arn: entry.has_profile_arn,
                    account_email: entry.account_email,
                    user_id: entry.user_id,

                    calls_total: stats.as_ref().map(|s| s.calls_total).unwrap_or(0),
                    calls_ok: stats.as_ref().map(|s| s.calls_ok).unwrap_or(0),
                    calls_err: stats.as_ref().map(|s| s.calls_err).unwrap_or(0),
                    input_tokens_total: stats.as_ref().map(|s| s.input_tokens_total).unwrap_or(0),
                    output_tokens_total: stats.as_ref().map(|s| s.output_tokens_total).unwrap_or(0),
                    last_call_at: stats.as_ref().and_then(|s| s.last_call_at.clone()),
                    last_success_at: stats.as_ref().and_then(|s| s.last_success_at.clone()),
                    last_error_at: stats.as_ref().and_then(|s| s.last_error_at.clone()),
                    last_error: stats.and_then(|s| s.last_error),
                }
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
    ///
    /// 使用 Kiro Web Portal API (GetUserUsageAndLimits) 获取用量信息
    pub async fn get_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        if !self.credential_exists(id) {
            return Err(AdminServiceError::NotFound { id });
        }

        let info = self
            .token_manager
            .get_account_info_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        let current_usage = info.usage.current;
        let usage_limit = info.usage.limit;
        let remaining = (usage_limit - current_usage).max(0.0);
        let usage_percentage = if usage_limit > 0.0 {
            (current_usage / usage_limit * 100.0).min(100.0)
        } else {
            0.0
        };

        Ok(BalanceResponse {
            id,
            subscription_title: info.subscription_title,
            current_usage,
            usage_limit,
            remaining,
            usage_percentage,
            next_reset_at: info.usage.next_reset_date,
        })
    }

    /// 获取指定凭据的账号信息（套餐/用量/邮箱等）
    pub async fn get_account_info(
        &self,
        id: u64,
    ) -> Result<super::types::CredentialAccountInfoResponse, AdminServiceError> {
        if !self.credential_exists(id) {
            return Err(AdminServiceError::NotFound { id });
        }

        let info = self
            .token_manager
            .get_account_info_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        Ok(super::types::CredentialAccountInfoResponse { id, account: info })
    }

    /// 删除指定凭据（并持久化到凭据文件）
    pub async fn delete_credential(&self, id: u64) -> Result<String, AdminServiceError> {
        if !self.credential_exists(id) {
            return Err(AdminServiceError::NotFound { id });
        }

        self.token_manager
            .remove_credential(id)
            .map_err(|e| self.classify_error(e, id))?;

        // 统计清理（尽力而为）
        if let Some(stats) = &self.stats {
            stats.reset_account(id);
            if let Err(e) = stats.flush_now().await {
                tracing::warn!("删除凭据后清理统计落盘失败: {}", e);
                return Ok(format!("凭据 #{} 已删除（统计清理失败: {}）", id, e));
            }
        }

        Ok(format!("凭据 #{} 已删除", id))
    }

    /// 获取指定凭据的统计详情
    pub fn get_credential_stats(&self, id: u64) -> Result<CredentialStatsResponse, AdminServiceError> {
        if !self.credential_exists(id) {
            return Err(AdminServiceError::NotFound { id });
        }

        let stats = match &self.stats {
            Some(s) => s.get_or_default(id),
            None => crate::stats::AccountStats {
                id,
                ..crate::stats::AccountStats::default()
            },
        };

        let mut by_day: Vec<StatsBucket> = stats
            .by_day
            .into_iter()
            .map(|(k, v)| Self::bucket_to_api(k, v))
            .collect();
        // 日期 key 本身按字典序即可（YYYY-MM-DD）
        by_day.sort_by(|a, b| b.key.cmp(&a.key));

        let mut by_model: Vec<StatsBucket> = stats
            .by_model
            .into_iter()
            .map(|(k, v)| Self::bucket_to_api(k, v))
            .collect();
        by_model.sort_by(|a, b| b.calls_total.cmp(&a.calls_total).then_with(|| a.key.cmp(&b.key)));

        Ok(CredentialStatsResponse {
            id,
            calls_total: stats.calls_total,
            calls_ok: stats.calls_ok,
            calls_err: stats.calls_err,
            input_tokens_total: stats.input_tokens_total,
            output_tokens_total: stats.output_tokens_total,
            last_call_at: stats.last_call_at,
            last_success_at: stats.last_success_at,
            last_error_at: stats.last_error_at,
            last_error: stats.last_error,
            by_day,
            by_model,
        })
    }

    /// 清空指定凭据的统计
    pub async fn reset_credential_stats(&self, id: u64) -> Result<(), AdminServiceError> {
        if !self.credential_exists(id) {
            return Err(AdminServiceError::NotFound { id });
        }

        let stats = match &self.stats {
            Some(s) => s,
            None => return Ok(()),
        };

        stats.reset_account(id);
        stats
            .flush_now()
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Ok(())
    }

    /// 清空全部统计
    pub async fn reset_all_stats(&self) -> Result<(), AdminServiceError> {
        let stats = match &self.stats {
            Some(s) => s,
            None => return Ok(()),
        };

        stats.reset_all();
        stats
            .flush_now()
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Ok(())
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
            account_email: None,
            user_id: None,
            provider: req.provider,
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
}
