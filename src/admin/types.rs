//! Admin API 类型定义

use serde::{Deserialize, Serialize};

// ============ 凭据状态 ============

/// 所有凭据状态响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsStatusResponse {
    /// 凭据总数
    pub total: usize,
    /// 可用凭据数量（未禁用）
    pub available: usize,
    /// 当前活跃凭据 ID
    pub current_id: u64,
    /// 各凭据状态列表
    pub credentials: Vec<CredentialStatusItem>,
}

// ============ 统计（可持久化） ============

/// 单个统计 bucket（按日/按模型）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsBucket {
    /// bucket key：
    /// - 按日：YYYY-MM-DD
    /// - 按模型：model id
    pub key: String,

    pub calls_total: u64,
    pub calls_ok: u64,
    pub calls_err: u64,
    pub input_tokens_total: u64,
    pub output_tokens_total: u64,

    pub last_call_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error_at: Option<String>,
    pub last_error: Option<String>,
}

/// 指定凭据的详细统计
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatsResponse {
    pub id: u64,

    pub calls_total: u64,
    pub calls_ok: u64,
    pub calls_err: u64,
    pub input_tokens_total: u64,
    pub output_tokens_total: u64,

    pub last_call_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error_at: Option<String>,
    pub last_error: Option<String>,

    pub by_day: Vec<StatsBucket>,
    pub by_model: Vec<StatsBucket>,
}

/// 单个凭据的状态信息
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatusItem {
    /// 凭据唯一 ID
    pub id: u64,
    /// 优先级（数字越小优先级越高）
    pub priority: u32,
    /// 是否被禁用
    pub disabled: bool,
    /// 连续失败次数
    pub failure_count: u32,
    /// 是否为当前活跃凭据
    pub is_current: bool,
    /// Token 过期时间（RFC3339 格式）
    pub expires_at: Option<String>,
    /// 认证方式
    pub auth_method: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// 账户邮箱（尽力从 token 中解析，仅用于展示）
    pub account_email: Option<String>,
    /// 用户 ID（从 API 获取，持久化保存）
    pub user_id: Option<String>,

    // ===== 统计（可持久化） =====

    /// 调用次数（对上游发起请求的尝试次数）
    pub calls_total: u64,
    /// 成功次数（上游返回 2xx）
    pub calls_ok: u64,
    /// 失败次数（网络/非 2xx/流读取中断等）
    pub calls_err: u64,
    /// 累计输入 tokens
    pub input_tokens_total: u64,
    /// 累计输出 tokens
    pub output_tokens_total: u64,
    /// 最后一次调用时间（RFC3339）
    pub last_call_at: Option<String>,
    /// 最后一次成功时间（RFC3339）
    pub last_success_at: Option<String>,
    /// 最后一次错误时间（RFC3339）
    pub last_error_at: Option<String>,
    /// 最后一次错误（如果最后一次调用成功则为 None）
    pub last_error: Option<String>,
}

// ============ 操作请求 ============

/// 启用/禁用凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDisabledRequest {
    /// 是否禁用
    pub disabled: bool,
}

/// 修改优先级请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPriorityRequest {
    /// 新优先级值
    pub priority: u32,
}

/// 添加凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialRequest {
    /// 刷新令牌（必填）
    pub refresh_token: String,

    /// 认证方式（可选，默认 social）
    #[serde(default = "default_auth_method")]
    pub auth_method: String,

    /// OIDC Client ID（IdC 认证需要）
    pub client_id: Option<String>,

    /// OIDC Client Secret（IdC 认证需要）
    pub client_secret: Option<String>,

    /// 优先级（可选，默认 0）
    #[serde(default)]
    pub priority: u32,

    /// 身份提供商（BuilderId / Github / Google）
    pub provider: Option<String>,
}

fn default_auth_method() -> String {
    "social".to_string()
}

/// 添加凭据成功响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialResponse {
    pub success: bool,
    pub message: String,
    /// 新添加的凭据 ID
    pub credential_id: u64,
}

// ============ 余额查询 ============

/// 余额查询响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResponse {
    /// 凭据 ID
    pub id: u64,
    /// 订阅类型
    pub subscription_title: Option<String>,
    /// 当前使用量
    pub current_usage: f64,
    /// 使用限额
    pub usage_limit: f64,
    /// 剩余额度
    pub remaining: f64,
    /// 使用百分比
    pub usage_percentage: f64,
    /// 下次重置时间（ISO 8601 日期字符串）
    pub next_reset_at: Option<String>,
}

/// 账号信息（套餐/用量/邮箱等）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialAccountInfoResponse {
    pub id: u64,
    pub account: crate::kiro::web_portal::AccountAggregateInfo,
}

// ============ 通用响应 ============

/// 操作成功响应
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
    pub message: String,
}

impl SuccessResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
        }
    }
}

/// 错误响应
#[derive(Debug, Serialize)]
pub struct AdminErrorResponse {
    pub error: AdminError,
}

#[derive(Debug, Serialize)]
pub struct AdminError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl AdminErrorResponse {
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: AdminError {
                error_type: error_type.into(),
                message: message.into(),
            },
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new("invalid_request", message)
    }

    pub fn authentication_error() -> Self {
        Self::new("authentication_error", "Invalid or missing admin API key")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn api_error(message: impl Into<String>) -> Self {
        Self::new("api_error", message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new("internal_error", message)
    }
}
