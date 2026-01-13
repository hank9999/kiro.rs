//! 运行时配置模块
//!
//! Input: 数据库 settings 表
//! Output: RuntimeConfig 结构体
//! Pos: 存储从数据库读取的运行时配置

/// 运行时配置（从数据库 settings 表读取）
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub kiro_version: String,
    pub system_version: String,
    pub node_version: String,
    pub count_tokens_api_url: Option<String>,
    pub count_tokens_api_key: Option<String>,
    pub count_tokens_auth_type: String,
    pub min_usage_threshold: f64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            kiro_version: "0.8.0".to_string(),
            system_version: "darwin#24.6.0".to_string(),
            node_version: "v22.12.0".to_string(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: "x-api-key".to_string(),
            min_usage_threshold: 5.0,
        }
    }
}
