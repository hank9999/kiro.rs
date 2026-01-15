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

    /// 凭据级 Region 配置（用于 OIDC token 刷新）
    /// 未配置时回退到 config.json 的全局 region
    pub region: Option<String>,

    /// 凭据级 Machine ID（可选，64 位字符串）
    /// 未配置时回退到 config.json 的 machineId
    pub machine_id: Option<String>,
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
    /// 下次重置时间（Unix 时间戳）
    pub next_reset_at: Option<f64>,
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

// ============ 凭据验证 ============

/// 凭据验证请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateCredentialsRequest {
    /// 要验证的凭据 ID 列表
    pub credential_ids: Vec<u64>,
    /// 验证使用的模型（sonnet/opus/haiku）
    #[serde(default = "default_validate_model")]
    pub model: String,
    /// 超时时间（毫秒），默认 30000
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// 最大并发数，默认 3
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

fn default_validate_model() -> String {
    "sonnet".to_string()
}

fn default_timeout_ms() -> u64 {
    30000
}

fn default_max_concurrency() -> usize {
    3
}

/// 凭据验证响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateCredentialsResponse {
    /// 验证结果列表
    pub results: Vec<CredentialValidationResult>,
    /// 汇总信息
    pub summary: ValidationSummary,
}

/// 单个凭据的验证结果
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialValidationResult {
    /// 凭据 ID
    pub id: u64,
    /// 验证状态
    pub status: ValidationStatus,
    /// 详细信息（错误时提供）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 响应耗时（毫秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// 验证状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationStatus {
    /// 验证成功（2xx 响应）
    Ok,
    /// 凭据被拒绝（401/403）
    Denied,
    /// 凭据无效（400 或 token 刷新失败）
    Invalid,
    /// 瞬态错误（408/429/5xx/网络错误）
    Transient,
    /// 凭据不存在
    NotFound,
}

/// 验证汇总
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationSummary {
    /// 总数
    pub total: usize,
    /// 成功数
    pub ok: usize,
    /// 被拒绝数
    pub denied: usize,
    /// 无效数
    pub invalid: usize,
    /// 瞬态错误数
    pub transient: usize,
    /// 未找到数
    pub not_found: usize,
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
