//! ListAvailableProfiles API 数据模型
//!
//! 用于 IdC/builder-id 凭证在刷新后获取 profileArn。

use serde::Deserialize;

/// ListAvailableProfiles 响应体
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAvailableProfilesResponse {
    /// 分页 token（可能为 null）
    #[serde(default)]
    pub next_token: Option<String>,

    /// 可用 profiles 列表（可能为 null 或缺失）
    #[serde(default)]
    pub profiles: Option<Vec<AvailableProfile>>,
}

/// 单个 Profile 信息
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableProfile {
    /// Profile ARN（可能为 null）
    #[serde(default)]
    pub arn: Option<String>,

    /// Profile 名称（可能为 null）
    #[serde(default)]
    pub profile_name: Option<String>,
}
