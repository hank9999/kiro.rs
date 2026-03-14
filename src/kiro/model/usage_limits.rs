//! 使用额度查询数据模型
//!
//! 包含 getUsageLimits API 的响应类型定义

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// 使用额度查询响应
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitsResponse {
    /// 距离下次重置的天数
    #[serde(default)]
    pub days_until_reset: Option<i32>,

    /// 下次重置日期 (ISO 8601 日期字符串，如 "2025-02-01T00:00:00Z")
    #[serde(default)]
    pub next_date_reset: Option<String>,

    /// 订阅信息
    #[serde(default)]
    pub subscription_info: Option<SubscriptionInfo>,

    /// 使用量明细列表
    #[serde(default)]
    pub usage_breakdown_list: Vec<UsageBreakdown>,

    /// 超额配置
    #[serde(default)]
    pub overage_configuration: Option<OverageConfiguration>,

    /// 用户信息
    #[serde(default)]
    pub user_info: Option<UserInfo>,
}

/// 超额配置
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverageConfiguration {
    /// 是否启用超额
    #[serde(default)]
    pub overage_enabled: bool,
}

/// 用户信息
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    /// 用户邮箱
    #[serde(default)]
    pub email: Option<String>,

    /// 用户 ID
    #[serde(default)]
    pub user_id: Option<String>,
}

/// 订阅信息
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionInfo {
    /// 订阅标题 (KIRO PRO+ / KIRO FREE 等)
    #[serde(default)]
    pub subscription_title: Option<String>,

    /// 订阅类型 (如 Q_DEVELOPER_STANDALONE_PRO_PLUS)
    #[serde(default, rename = "type")]
    pub subscription_type: Option<String>,

    /// 可升级能力
    #[serde(default)]
    pub upgrade_capability: Option<String>,

    /// 超额能力
    #[serde(default)]
    pub overage_capability: Option<String>,

    /// 订阅管理目标
    #[serde(default)]
    pub subscription_management_target: Option<String>,
}

/// 使用量明细
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBreakdown {
    /// 当前使用量
    #[serde(default)]
    pub current_usage: i64,

    /// 当前使用量（精确值）
    #[serde(default)]
    pub current_usage_with_precision: f64,

    /// 免费试用信息
    #[serde(default)]
    pub free_trial_info: Option<FreeTrialInfo>,

    /// 下次重置日期 (ISO 8601 日期字符串)
    #[serde(default)]
    pub next_date_reset: Option<String>,

    /// 使用限额
    #[serde(default)]
    pub usage_limit: i64,

    /// 使用限额（精确值）
    #[serde(default)]
    pub usage_limit_with_precision: f64,

    /// 额外用量包（如 GIFT 类型）
    #[serde(default)]
    pub bonuses: Vec<Bonus>,

    /// 资源类型 (CREDIT 等)
    #[serde(default)]
    pub resource_type: Option<String>,

    /// 显示名称
    #[serde(default)]
    pub display_name: Option<String>,

    /// 显示名称（复数形式）
    #[serde(default)]
    pub display_name_plural: Option<String>,

    /// 货币类型 (USD 等)
    #[serde(default)]
    pub currency: Option<String>,

    /// 单位 (INVOCATIONS 等)
    #[serde(default)]
    pub unit: Option<String>,

    /// 超额费率
    #[serde(default)]
    pub overage_rate: Option<f64>,

    /// 超额上限
    #[serde(default)]
    pub overage_cap: Option<f64>,
}

/// 额外用量包信息（如 GIFT 类型）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bonus {
    /// 用量包代码
    #[serde(default)]
    pub bonus_code: Option<String>,

    /// 显示名称
    #[serde(default)]
    pub display_name: Option<String>,

    /// 当前使用量
    #[serde(default)]
    pub current_usage: i64,

    /// 当前使用量（精确值）
    #[serde(default)]
    pub current_usage_with_precision: f64,

    /// 使用限额
    #[serde(default)]
    pub usage_limit: i64,

    /// 使用限额（精确值）
    #[serde(default)]
    pub usage_limit_with_precision: f64,

    /// 状态 (ACTIVE / EXPIRED)
    #[serde(default)]
    pub status: Option<String>,

    /// 过期时间 (RFC3339 格式)
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// 免费试用信息
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeTrialInfo {
    /// 当前使用量
    #[serde(default)]
    pub current_usage: i64,

    /// 当前使用量（精确值）
    #[serde(default)]
    pub current_usage_with_precision: f64,

    /// 免费试用过期时间 (ISO 8601 日期字符串)
    #[serde(default)]
    pub free_trial_expiry: Option<String>,

    /// 免费试用状态 (ACTIVE / EXPIRED)
    #[serde(default)]
    pub free_trial_status: Option<String>,

    /// 使用限额
    #[serde(default)]
    pub usage_limit: i64,

    /// 使用限额（精确值）
    #[serde(default)]
    pub usage_limit_with_precision: f64,
}

// ============ 便捷方法实现 ============

impl Bonus {
    /// 检查用量包是否处于激活状态
    pub fn is_active(&self) -> bool {
        match self.status.as_deref() {
            Some(s) => s.eq_ignore_ascii_case("ACTIVE"),
            None => {
                // 没有 status 时：优先用 expires_at 判断是否仍有效；再用 limit/current 兜底。
                if let Some(exp) = self.expires_at.as_deref() {
                    if let Ok(dt) = DateTime::parse_from_rfc3339(exp) {
                        return dt > Utc::now();
                    }
                }
                let limit = self.usage_limit_with_precision;
                let current = self.current_usage_with_precision;
                limit > 0.0 || current > 0.0
            }
        }
    }
}

impl FreeTrialInfo {
    /// 检查免费试用是否处于激活状态
    pub fn is_active(&self) -> bool {
        self.free_trial_status
            .as_deref()
            .map(|s| s == "ACTIVE")
            .unwrap_or(false)
    }
}

impl UsageLimitsResponse {
    /// 获取订阅标题
    pub fn subscription_title(&self) -> Option<&str> {
        self.subscription_info
            .as_ref()
            .and_then(|info| info.subscription_title.as_deref())
    }

    /// 获取第一个使用量明细
    fn primary_breakdown(&self) -> Option<&UsageBreakdown> {
        self.usage_breakdown_list.first()
    }

    /// 获取总使用限额（精确值）
    ///
    /// 合并基础额度、免费试用额度（如激活）以及所有激活的用量包（如 GIFT）
    pub fn usage_limit(&self) -> f64 {
        let Some(breakdown) = self.primary_breakdown() else {
            return 0.0;
        };

        let base_limit = breakdown.usage_limit_with_precision;

        // 如果 free trial 处于激活状态，合并额度
        let free_trial_limit = breakdown
            .free_trial_info
            .as_ref()
            .filter(|t| t.is_active())
            .map(|t| t.usage_limit_with_precision)
            .unwrap_or(0.0);

        // 合并所有激活的 bonuses 额度
        let bonuses_limit: f64 = breakdown
            .bonuses
            .iter()
            .filter(|b| b.is_active())
            .map(|b| b.usage_limit_with_precision)
            .sum();

        base_limit + free_trial_limit + bonuses_limit
    }

    /// 获取总当前使用量（精确值）
    ///
    /// 合并基础使用量、免费试用使用量（如激活）以及所有激活的用量包（如 GIFT）
    pub fn current_usage(&self) -> f64 {
        let Some(breakdown) = self.primary_breakdown() else {
            return 0.0;
        };

        let base_usage = breakdown.current_usage_with_precision;

        // 如果 free trial 处于激活状态，合并使用量
        let free_trial_usage = breakdown
            .free_trial_info
            .as_ref()
            .filter(|t| t.is_active())
            .map(|t| t.current_usage_with_precision)
            .unwrap_or(0.0);

        // 合并所有激活的 bonuses 使用量
        let bonuses_usage: f64 = breakdown
            .bonuses
            .iter()
            .filter(|b| b.is_active())
            .map(|b| b.current_usage_with_precision)
            .sum();

        base_usage + free_trial_usage + bonuses_usage
    }
}
