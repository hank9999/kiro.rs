//! 上游 ListAvailableModels 接口数据模型
//!
//! 对应 `GET https://q.{api_region}.amazonaws.com/ListAvailableModels?origin=AI_EDITOR`
//! 的响应类型定义。响应结构示例：
//! ```json
//! {
//!   "defaultModel": { "modelId": "auto", "modelName": "Auto", ... },
//!   "models": [
//!     { "modelId": "claude-sonnet-4.5", "modelName": "Claude Sonnet 4.5", ... },
//!     ...
//!   ],
//!   "nextToken": null
//! }
//! ```
//!
//! 上游响应字段较多，这里仅声明使用的字段；未声明字段被 serde 忽略。

use serde::Deserialize;

/// `ListAvailableModels` 响应根对象
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAvailableModelsResponse {
    /// 默认模型（上游推荐的入口模型）
    #[serde(default)]
    pub default_model: Option<UpstreamModel>,

    /// 全部可用模型
    #[serde(default)]
    pub models: Vec<UpstreamModel>,

    /// 分页 token；当前实测始终为 null，保留以兼容未来分页
    #[serde(default)]
    #[allow(dead_code)]
    pub next_token: Option<String>,
}

/// 单个上游模型条目
///
/// 上游响应字段较多，当前对外 `Model` 结构只用到一部分；
/// 其余字段保留为可读字段（标 `dead_code`），便于未来扩展 admin 页面展示。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModel {
    /// 模型 ID（如 "claude-sonnet-4.5" / "auto" / "deepseek-3.2"）
    pub model_id: String,

    /// 展示名称（如 "Claude Sonnet 4.5"）
    #[serde(default)]
    pub model_name: Option<String>,

    /// 模型描述
    #[serde(default)]
    #[allow(dead_code)]
    pub description: Option<String>,

    /// 计费倍数（0.05 ~ 1.3）
    #[serde(default)]
    #[allow(dead_code)]
    pub rate_multiplier: Option<f64>,

    /// 计费单位（如 "Credit"）
    #[serde(default)]
    #[allow(dead_code)]
    pub rate_unit: Option<String>,

    /// 支持的输入类型（如 ["TEXT"] 或 ["TEXT", "IMAGE"]）
    #[serde(default)]
    #[allow(dead_code)]
    pub supported_input_types: Vec<String>,

    /// Token 上下文窗口
    #[serde(default)]
    pub token_limits: Option<UpstreamTokenLimits>,

    /// Prompt Caching 能力
    #[serde(default)]
    #[allow(dead_code)]
    pub prompt_caching: Option<UpstreamPromptCaching>,
}

/// 上下文窗口
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamTokenLimits {
    /// 最大输入 token 数
    #[serde(default)]
    pub max_input_tokens: Option<u64>,

    /// 最大输出 token 数
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
}

/// Prompt Caching 能力描述
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct UpstreamPromptCaching {
    /// 是否支持 prompt caching
    #[serde(default)]
    pub supports_prompt_caching: bool,

    /// 单次请求最大缓存检查点数
    #[serde(default)]
    pub maximum_cache_checkpoints_per_request: Option<u64>,

    /// 单个缓存检查点最少 token 数
    #[serde(default)]
    pub minimum_tokens_per_cache_checkpoint: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证使用真实的上游响应能正确反序列化（来自 scripts/test_list_models.py 的实测响应）
    #[test]
    fn test_deserialize_real_upstream_response() {
        let raw = r#"{
            "defaultModel": {
                "description": "Models chosen by task for optimal usage and consistent quality",
                "modelId": "auto",
                "modelName": "Auto",
                "promptCaching": {
                    "maximumCacheCheckpointsPerRequest": 4,
                    "minimumTokensPerCacheCheckpoint": 1024,
                    "supportsPromptCaching": true
                },
                "rateMultiplier": 1.0,
                "rateUnit": "Credit",
                "supportedInputTypes": ["TEXT", "IMAGE"],
                "tokenLimits": {
                    "maxInputTokens": 1000000,
                    "maxOutputTokens": 64000
                }
            },
            "models": [
                {
                    "description": "The Claude Sonnet 4.5 model",
                    "modelId": "claude-sonnet-4.5",
                    "modelName": "Claude Sonnet 4.5",
                    "promptCaching": {
                        "maximumCacheCheckpointsPerRequest": 4,
                        "minimumTokensPerCacheCheckpoint": 1024,
                        "supportsPromptCaching": true
                    },
                    "rateMultiplier": 1.3,
                    "rateUnit": "Credit",
                    "supportedInputTypes": ["TEXT", "IMAGE"],
                    "tokenLimits": {
                        "maxInputTokens": 200000,
                        "maxOutputTokens": 64000
                    }
                }
            ],
            "nextToken": null
        }"#;

        let parsed: ListAvailableModelsResponse =
            serde_json::from_str(raw).expect("应能解析真实响应");
        assert!(parsed.default_model.is_some());
        assert_eq!(parsed.models.len(), 1);
        let first = &parsed.models[0];
        assert_eq!(first.model_id, "claude-sonnet-4.5");
        assert_eq!(first.model_name.as_deref(), Some("Claude Sonnet 4.5"));
        assert_eq!(first.rate_multiplier, Some(1.3));
        assert_eq!(
            first.token_limits.as_ref().and_then(|t| t.max_output_tokens),
            Some(64000)
        );
        assert_eq!(
            first.supported_input_types,
            vec!["TEXT".to_string(), "IMAGE".to_string()]
        );
    }

    /// 验证缺少可选字段时仍能正常解析
    #[test]
    fn test_deserialize_minimal_model() {
        let raw = r#"{
            "models": [
                { "modelId": "test-model" }
            ]
        }"#;

        let parsed: ListAvailableModelsResponse =
            serde_json::from_str(raw).expect("最小字段也应能解析");
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].model_id, "test-model");
        assert!(parsed.default_model.is_none());
        assert!(parsed.models[0].token_limits.is_none());
    }
}
