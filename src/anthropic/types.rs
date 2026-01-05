//! Anthropic API 类型定义

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// === 错误响应 ===

/// API 错误响应
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

/// 错误详情
#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl ErrorResponse {
    /// 创建新的错误响应
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: ErrorDetail {
                error_type: error_type.into(),
                message: message.into(),
            },
        }
    }

    /// 创建认证错误响应
    pub fn authentication_error() -> Self {
        Self::new("authentication_error", "Invalid API key")
    }
}

// === Models 端点类型 ===

/// 模型信息
#[derive(Debug, Serialize)]
pub struct Model {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub model_type: String,
    pub max_tokens: i32,
}

/// 模型列表响应
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub object: String,
    pub data: Vec<Model>,
}

// === Messages 端点类型 ===

/// 最大思考预算 tokens
const MAX_BUDGET_TOKENS: i32 = 24576;

/// Thinking 配置
#[derive(Debug, Deserialize, Clone)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(default = "default_budget_tokens", deserialize_with = "deserialize_budget_tokens")]
    pub budget_tokens: i32,
}

fn default_budget_tokens() -> i32 {
    20000
}
fn deserialize_budget_tokens<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = i32::deserialize(deserializer)?;
    Ok(value.min(MAX_BUDGET_TOKENS))
}

/// Messages 请求体
#[derive(Debug, Deserialize)]
pub struct MessagesRequest {
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: i32,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    pub system: Option<Vec<SystemMessage>>,
    /// tools 可以是普通 Tool 或 WebSearchTool 等多种格式，使用 Value 灵活处理
    pub tools: Option<Vec<serde_json::Value>>,
    pub tool_choice: Option<serde_json::Value>,
    pub thinking: Option<Thinking>,
}

fn default_max_tokens() -> i32 {
    4096
}

/// 消息
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub role: String,
    /// 可以是 string 或 ContentBlock 数组
    pub content: serde_json::Value,
}

/// 系统消息
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SystemMessage {
    #[serde(rename = "type", default = "default_message_type")]
    pub message_type: String,
    pub text: String,
}

fn default_message_type() -> String {
    "text".to_string()
}

/// 工具定义
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub input_schema: HashMap<String, serde_json::Value>,
}

/// 内容块
#[derive(Debug, Deserialize, Serialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ImageSource>,
}

/// 图片数据源
#[derive(Debug, Deserialize, Serialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

// === Count Tokens 端点类型 ===

/// Token 计数请求
#[derive(Debug, Serialize, Deserialize)]
pub struct CountTokensRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
}

/// Token 计数响应
#[derive(Debug, Serialize, Deserialize)]
pub struct CountTokensResponse {
    pub input_tokens: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 new-api 发送的完整 Claude 请求格式
    /// 包含：system 数组格式、普通 Tool、WebSearchTool、omitempty 字段缺失等情况
    #[test]
    fn test_new_api_claude_request_format() {
        // 模拟 new-api 发送的真实请求
        let json = r#"{
            "model": "claude-sonnet-4-5-20250929",
            "messages": [
                {"role": "user", "content": "Hello"}
            ],
            "system": [
                {"type": "text", "text": "You are a helpful assistant"}
            ],
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Get weather info",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "location": {"type": "string"}
                        }
                    }
                },
                {
                    "type": "web_search_20250305",
                    "name": "web_search",
                    "max_uses": 5
                }
            ],
            "stream": true
        }"#;

        let req: MessagesRequest = serde_json::from_str(json).expect("应该能解析 new-api 请求");

        assert_eq!(req.model, "claude-sonnet-4-5-20250929");
        assert_eq!(req.max_tokens, 4096); // 默认值
        assert!(req.stream);
        assert_eq!(req.messages.len(), 1);

        // 验证 system
        let system = req.system.expect("应该有 system");
        assert_eq!(system.len(), 1);
        assert_eq!(system[0].message_type, "text");
        assert_eq!(system[0].text, "You are a helpful assistant");

        // 验证 tools（包含普通 Tool 和 WebSearchTool）
        let tools = req.tools.expect("应该有 tools");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].get("name").unwrap().as_str().unwrap(), "get_weather");
        assert_eq!(tools[1].get("type").unwrap().as_str().unwrap(), "web_search_20250305");
    }

    /// 测试 max_tokens 缺失时使用默认值
    #[test]
    fn test_max_tokens_default() {
        let json = r#"{
            "model": "claude-sonnet-4-5-20250929",
            "messages": [{"role": "user", "content": "Hi"}]
        }"#;

        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.max_tokens, 4096);
    }

    /// 测试 system 中 type 字段缺失时使用默认值
    #[test]
    fn test_system_message_type_default() {
        let json = r#"{
            "model": "claude-sonnet-4-5-20250929",
            "messages": [{"role": "user", "content": "Hi"}],
            "system": [{"text": "Be helpful"}]
        }"#;

        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        let system = req.system.unwrap();
        assert_eq!(system[0].message_type, "text");
    }

    /// 测试 Tool 的 description 可选
    #[test]
    fn test_tool_description_optional() {
        let json = r#"{
            "model": "claude-sonnet-4-5-20250929",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{
                "name": "my_tool",
                "input_schema": {"type": "object"}
            }]
        }"#;

        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        let tools = req.tools.unwrap();
        assert!(tools[0].get("description").is_none());
    }

    /// 测试 SystemMessage 序列化时 type 字段存在
    #[test]
    fn test_system_message_serialization() {
        let msg = SystemMessage {
            message_type: "text".to_string(),
            text: "Hello".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"text""#));
    }

    /// 测试复杂消息内容（数组格式）
    #[test]
    fn test_complex_message_content() {
        let json = r#"{
            "model": "claude-sonnet-4-5-20250929",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What is this?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "..."}}
                ]
            }]
        }"#;

        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        assert!(req.messages[0].content.is_array());
    }

    /// 测试 thinking 配置
    #[test]
    fn test_thinking_config() {
        let json = r#"{
            "model": "claude-sonnet-4-5-20250929",
            "messages": [{"role": "user", "content": "Hi"}],
            "thinking": {
                "type": "enabled",
                "budget_tokens": 10000
            }
        }"#;

        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        let thinking = req.thinking.unwrap();
        assert_eq!(thinking.thinking_type, "enabled");
        assert_eq!(thinking.budget_tokens, 10000);
    }

    /// 测试 thinking budget_tokens 超过最大值时被截断
    #[test]
    fn test_thinking_budget_tokens_capped() {
        let json = r#"{
            "model": "claude-sonnet-4-5-20250929",
            "messages": [{"role": "user", "content": "Hi"}],
            "thinking": {
                "type": "enabled",
                "budget_tokens": 100000
            }
        }"#;

        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        let thinking = req.thinking.unwrap();
        assert_eq!(thinking.budget_tokens, 24576); // MAX_BUDGET_TOKENS
    }
}
