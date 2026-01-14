//! Admin API 类型定义
//!
//! Input: serde
//! Output: API 请求/响应类型
//! Pos: Admin API 数据类型定义

use serde::{Deserialize, Serialize};

// ============ 认证相关 ============

/// 登录请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// 登录响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub success: bool,
    pub message: String,
    pub username: Option<String>,
}

/// 修改密码请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// 用户信息响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfoResponse {
    pub id: i64,
    pub username: String,
}

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
    /// 禁用原因（suspended: 账号暂停, quota: 额度用尽, manual: 手动禁用, failures: 连续失败）
    pub disabled_reason: Option<String>,
    /// 连续失败次数
    pub failure_count: u32,
    /// 是否为当前活跃凭据
    pub is_current: bool,
    /// Token 过期时间（RFC3339 格式）
    pub expires_at: Option<String>,
    /// 认证方式
    pub auth_method: Option<String>,
    /// AWS Region
    pub region: Option<String>,
    /// Machine ID
    pub machine_id: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// 用户邮箱
    pub email: Option<String>,
    /// 订阅类型
    pub subscription_title: Option<String>,
    /// 当前使用量
    pub current_usage: f64,
    /// 使用限额
    pub usage_limit: f64,
    /// 代理 URL
    pub proxy_url: Option<String>,
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

    /// AWS Region（必填）
    #[serde(default = "default_region")]
    pub region: String,

    /// 凭据级 Machine ID（可选，64 位字符串）
    pub machine_id: Option<String>,

    /// 代理 URL（可选，支持 http/https/socks5）
    pub proxy_url: Option<String>,
}

fn default_auth_method() -> String {
    "social".to_string()
}

fn default_region() -> String {
    "us-east-1".to_string()
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

/// 更新凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCredentialRequest {
    /// 优先级（可选）
    pub priority: Option<u32>,

    /// AWS Region（可选）
    pub region: Option<String>,

    /// Machine ID（可选）
    pub machine_id: Option<String>,

    /// Refresh Token（可选）
    pub refresh_token: Option<String>,

    /// Client ID（可选，IdC 认证）
    pub client_id: Option<String>,

    /// Client Secret（可选，IdC 认证）
    pub client_secret: Option<String>,

    /// 代理 URL（可选，支持 http/https/socks5）
    pub proxy_url: Option<String>,
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
    /// 下次重置时间（Unix 时间戳）
    pub next_reset_at: Option<f64>,
}

/// 刷新所有凭据响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshAllResponse {
    pub success: bool,
    pub message: String,
    /// 成功刷新的凭据数量
    pub success_count: usize,
    /// 刷新失败的凭据列表
    pub failures: Vec<RefreshFailure>,
}

/// 刷新失败信息
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshFailure {
    /// 凭据 ID
    pub id: u64,
    /// 错误信息
    pub error: String,
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

// ============ 系统设置 ============

/// 系统设置响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsResponse {
    pub kiro_version: String,
    pub system_version: String,
    pub node_version: String,
    pub min_usage_threshold: f64,
    pub count_tokens_api_url: Option<String>,
    pub count_tokens_api_key: Option<String>,
    pub count_tokens_auth_type: String,
}

/// 更新系统设置请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsRequest {
    pub kiro_version: Option<String>,
    pub system_version: Option<String>,
    pub node_version: Option<String>,
    pub min_usage_threshold: Option<f64>,
    pub count_tokens_api_url: Option<String>,
    pub count_tokens_api_key: Option<String>,
    pub count_tokens_auth_type: Option<String>,
}

// ============ 导入导出 ============

/// 导出凭据项（用于导出 JSON）
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCredentialItem {
    /// 刷新令牌
    pub refresh_token: String,
    /// OIDC Client ID（IdC 认证需要）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OIDC Client Secret（IdC 认证需要）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// AWS Region
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// 代理 URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

/// 导出凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCredentialsRequest {
    /// 要导出的凭据 ID 列表（为空则导出全部）
    pub ids: Option<Vec<u64>>,
}

/// 导出凭据响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCredentialsResponse {
    pub success: bool,
    pub message: String,
    /// 导出的凭据数量
    pub count: usize,
    /// 导出的凭据数据
    pub credentials: Vec<ExportCredentialItem>,
}

/// 导入凭据项（用于导入 JSON）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCredentialItem {
    /// 刷新令牌（必填）
    pub refresh_token: String,
    /// OIDC Client ID（可选，有值则为 IdC 模式）
    #[serde(default)]
    pub client_id: Option<String>,
    /// OIDC Client Secret（可选，有值则为 IdC 模式）
    #[serde(default)]
    pub client_secret: Option<String>,
    /// AWS Region（可选，默认 us-east-1）
    #[serde(default)]
    pub region: Option<String>,
    /// 代理 URL（可选，默认为空）
    #[serde(default)]
    pub proxy_url: Option<String>,
}

/// 导入凭据请求（支持单个对象或数组）
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ImportCredentialsRequest {
    /// 单个凭据
    Single(ImportCredentialItem),
    /// 多个凭据
    Multiple(Vec<ImportCredentialItem>),
}

impl ImportCredentialsRequest {
    /// 转换为凭据列表
    pub fn into_vec(self) -> Vec<ImportCredentialItem> {
        match self {
            ImportCredentialsRequest::Single(item) => vec![item],
            ImportCredentialsRequest::Multiple(items) => items,
        }
    }
}

/// 导入凭据响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCredentialsResponse {
    pub success: bool,
    pub message: String,
    /// 成功导入的数量
    pub success_count: usize,
    /// 导入失败的列表
    pub failures: Vec<ImportFailure>,
}

/// 导入失败信息
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    /// 索引（从 0 开始）
    pub index: usize,
    /// 错误信息
    pub error: String,
}
