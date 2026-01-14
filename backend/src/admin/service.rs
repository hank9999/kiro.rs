//! Admin API 业务逻辑服务

use std::sync::Arc;

use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;

use super::error::AdminServiceError;
use super::types::{
    AddCredentialRequest, AddCredentialResponse, BalanceResponse, CredentialStatusItem,
    CredentialsStatusResponse, ExportCredentialItem, ExportCredentialsRequest,
    ExportCredentialsResponse, ImportCredentialItem, ImportCredentialsResponse, ImportFailure,
    RefreshAllResponse, RefreshFailure, UpdateCredentialRequest,
};

/// Admin 服务
///
/// 封装所有 Admin API 的业务逻辑
pub struct AdminService {
    token_manager: Arc<MultiTokenManager>,
}

impl AdminService {
    pub fn new(token_manager: Arc<MultiTokenManager>) -> Self {
        Self { token_manager }
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
                disabled_reason: entry.disabled_reason,
                failure_count: entry.failure_count,
                is_current: entry.id == snapshot.current_id,
                expires_at: entry.expires_at,
                auth_method: entry.auth_method,
                region: entry.region,
                machine_id: entry.machine_id,
                has_profile_arn: entry.has_profile_arn,
                email: entry.email,
                subscription_title: entry.subscription_title,
                current_usage: entry.current_usage,
                usage_limit: entry.usage_limit,
                proxy_url: entry.proxy_url,
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
            region: Some(req.region),
            machine_id: req.machine_id,
            proxy_url: req.proxy_url,
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

    /// 更新凭据元数据
    pub fn update_credential(
        &self,
        id: u64,
        req: UpdateCredentialRequest,
    ) -> Result<(), AdminServiceError> {
        self.token_manager
            .update_credential_metadata(
                id,
                req.priority,
                req.region,
                req.machine_id,
                req.refresh_token,
                req.client_id,
                req.client_secret,
                req.proxy_url,
            )
            .map_err(|e| self.classify_error(e, id))
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

    /// 刷新所有凭据的余额信息
    pub async fn refresh_all_balances(&self) -> RefreshAllResponse {
        let (success_count, failures) = self.token_manager.refresh_all_balances().await;
        RefreshAllResponse {
            success: failures.is_empty(),
            message: if failures.is_empty() {
                format!("成功刷新 {} 个凭据", success_count)
            } else {
                format!(
                    "刷新完成: 成功 {}, 失败 {}",
                    success_count,
                    failures.len()
                )
            },
            success_count,
            failures: failures
                .into_iter()
                .map(|(id, error)| RefreshFailure { id, error })
                .collect(),
        }
    }

    /// 设置当前使用的凭据
    pub fn set_current(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_current(id)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 导出凭据
    pub fn export_credentials(&self, req: ExportCredentialsRequest) -> ExportCredentialsResponse {
        let all_creds = self.token_manager.get_all_credentials_for_export();

        let credentials: Vec<ExportCredentialItem> = all_creds
            .into_iter()
            .filter(|cred| {
                // 如果指定了 ID 列表，则只导出指定的凭据
                if let Some(ref ids) = req.ids {
                    cred.id.map_or(false, |id| ids.contains(&id))
                } else {
                    true
                }
            })
            .filter_map(|cred| {
                // 必须有 refresh_token 才能导出
                cred.refresh_token.map(|refresh_token| ExportCredentialItem {
                    refresh_token,
                    client_id: cred.client_id.filter(|s| !s.is_empty()),
                    client_secret: cred.client_secret.filter(|s| !s.is_empty()),
                    region: cred.region,
                    proxy_url: cred.proxy_url.filter(|s| !s.is_empty()),
                })
            })
            .collect();

        let count = credentials.len();
        ExportCredentialsResponse {
            success: true,
            message: format!("成功导出 {} 个凭据", count),
            count,
            credentials,
        }
    }

    /// 批量导入凭据
    pub async fn import_credentials(
        &self,
        items: Vec<ImportCredentialItem>,
    ) -> ImportCredentialsResponse {
        let mut success_count = 0;
        let mut failures = Vec::new();

        for (index, item) in items.into_iter().enumerate() {
            // 根据 client_id 和 client_secret 判断认证方式
            let auth_method = if item.client_id.as_ref().map_or(false, |s| !s.is_empty())
                && item.client_secret.as_ref().map_or(false, |s| !s.is_empty())
            {
                "idc".to_string()
            } else {
                "social".to_string()
            };

            let new_cred = KiroCredentials {
                id: None,
                access_token: None,
                refresh_token: Some(item.refresh_token),
                profile_arn: None,
                expires_at: None,
                auth_method: Some(auth_method),
                client_id: item.client_id.filter(|s| !s.is_empty()),
                client_secret: item.client_secret.filter(|s| !s.is_empty()),
                priority: 0,
                region: Some(item.region.unwrap_or_else(|| "us-east-1".to_string())),
                machine_id: None,
                proxy_url: item.proxy_url.filter(|s| !s.is_empty()),
            };

            match self.token_manager.add_credential(new_cred).await {
                Ok(_) => success_count += 1,
                Err(e) => failures.push(ImportFailure {
                    index,
                    error: e.to_string(),
                }),
            }
        }

        ImportCredentialsResponse {
            success: failures.is_empty(),
            message: if failures.is_empty() {
                format!("成功导入 {} 个凭据", success_count)
            } else {
                format!(
                    "导入完成: 成功 {}, 失败 {}",
                    success_count,
                    failures.len()
                )
            },
            success_count,
            failures,
        }
    }
}
