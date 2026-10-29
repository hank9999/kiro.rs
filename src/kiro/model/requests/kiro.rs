//! Kiro 请求类型定义
//!
//! 定义 Kiro API 的主请求结构

use serde::{Deserialize, Serialize};

use super::conversation::ConversationState;

/// Kiro API 请求
///
/// 用于构建发送给 Kiro API 的请求
///
/// # 示例
///
/// ```rust
/// use kiro_rs::kiro::model::requests::{
///     KiroRequest, ConversationState, CurrentMessage, UserInputMessage, Tool
/// };
///
/// // 创建简单请求
/// let state = ConversationState::new("conv-123")
///     .with_agent_task_type("vibe")
///     .with_current_message(CurrentMessage::new(
///         UserInputMessage::new("Hello", "claude-3-5-sonnet")
///     ));
///
/// let request = KiroRequest::new(state);
/// let json = request.to_json().unwrap();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroRequest {
    /// 对话状态
    pub conversation_state: ConversationState,
    /// Profile ARN（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,
    /// 模型附加参数（推理强度、思考模式），与 Kiro IDE 发送的原生字段一致
    ///
    /// 上游按模型 schema 严格校验：不支持该字段的模型、或取值不在枚举内都会返回 400，
    /// 因此只由 converter 在确认模型支持时设置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_model_request_fields: Option<AdditionalModelRequestFields>,
}

/// `additionalModelRequestFields` 的内容
///
/// 外层字段名是 camelCase，内层（`output_config`、`thinking`）保持 snake_case，
/// 因此不能使用 `rename_all = "camelCase"`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AdditionalModelRequestFields {
    /// 推理强度，对应 Anthropic 的 `output_config.effort`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<ModelOutputConfig>,
    /// 思考模式，对应 Anthropic 的 `thinking.type`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ModelThinking>,
}

/// 推理强度（low / medium / high / xhigh / max，具体可选值由模型决定）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelOutputConfig {
    pub effort: String,
}

/// 思考模式
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelThinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_kiro_request_deserialize() {
        let json = r#"{
            "conversationState": {
                "conversationId": "conv-456",
                "currentMessage": {
                    "userInputMessage": {
                        "content": "Test message",
                        "modelId": "claude-3-5-sonnet",
                        "userInputMessageContext": {}
                    }
                }
            }
        }"#;

        let request: KiroRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.conversation_state.conversation_id, "conv-456");
        assert!(request.additional_model_request_fields.is_none());
        assert_eq!(
            request
                .conversation_state
                .current_message
                .user_input_message
                .content,
            "Test message"
        );
    }

    #[test]
    fn test_additional_model_request_fields_wire_format() {
        let json = r#"{"conversationState":{"conversationId":"c1","currentMessage":{"userInputMessage":{"content":"hi","modelId":"claude-opus-5.5","userInputMessageContext":{}}}}}"#;
        let mut request: KiroRequest = serde_json::from_str(json).unwrap();

        // 未设置时不出现在请求体里
        let value = serde_json::to_value(&request).unwrap();
        assert!(value.get("additionalModelRequestFields").is_none());

        request.additional_model_request_fields = Some(AdditionalModelRequestFields {
            output_config: Some(ModelOutputConfig {
                effort: "max".to_string(),
            }),
            thinking: Some(ModelThinking {
                thinking_type: "adaptive".to_string(),
            }),
        });
        let value = serde_json::to_value(&request).unwrap();
        // 外层 camelCase，内层保持 snake_case
        assert_eq!(
            value["additionalModelRequestFields"],
            serde_json::json!({
                "output_config": {"effort": "max"},
                "thinking": {"type": "adaptive"}
            })
        );
    }
}
