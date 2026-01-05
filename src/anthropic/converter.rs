//! Anthropic → Kiro 协议转换器
//!
//! 负责将 Anthropic API 请求格式转换为 Kiro API 请求格式

use uuid::Uuid;

use crate::kiro::model::requests::conversation::{
    AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, KiroImage, Message, UserInputMessage, UserInputMessageContext, UserMessage,
};
use crate::kiro::model::requests::tool::{InputSchema, Tool, ToolResult, ToolSpecification, ToolUseEntry};

use super::types::{ContentBlock, MessagesRequest, Thinking};

/// 模型映射：将 Anthropic 模型名映射到 Kiro 模型 ID
///
/// 按照用户要求：
/// - 所有 sonnet → claude-sonnet-4.5
/// - 所有 opus → claude-opus-4.5
/// - 所有 haiku → claude-haiku-4.5
pub fn map_model(model: &str) -> Option<String> {
    let model_lower = model.to_lowercase();

    if model_lower.contains("sonnet") {
        Some("claude-sonnet-4.5".to_string())
    } else if model_lower.contains("opus") {
        Some("claude-opus-4.5".to_string())
    } else if model_lower.contains("haiku") {
        Some("claude-haiku-4.5".to_string())
    } else {
        None
    }
}

/// 转换结果
#[derive(Debug)]
pub struct ConversionResult {
    /// 转换后的 Kiro 请求
    pub conversation_state: ConversationState
}

/// 转换错误
#[derive(Debug)]
pub enum ConversionError {
    UnsupportedModel(String),
    EmptyMessages
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversionError::UnsupportedModel(model) => write!(f, "模型不支持: {}", model),
            ConversionError::EmptyMessages => write!(f, "消息列表为空"),
        }
    }
}

impl std::error::Error for ConversionError {}

/// 将 Anthropic 请求转换为 Kiro 请求
pub fn convert_request(req: &MessagesRequest) -> Result<ConversionResult, ConversionError> {
    // 1. 映射模型
    let model_id = map_model(&req.model)
        .ok_or_else(|| ConversionError::UnsupportedModel(req.model.clone()))?;

    // 2. 检查消息列表
    if req.messages.is_empty() {
        return Err(ConversionError::EmptyMessages);
    }

    // 3. 生成会话 ID 和代理 ID
    let conversation_id = Uuid::new_v4().to_string();
    let agent_continuation_id = Uuid::new_v4().to_string();

    // 4. 确定触发类型
    let chat_trigger_type = determine_chat_trigger_type(req);

    // 5. 处理最后一条消息作为 current_message
    let last_message = req.messages.last().unwrap();
    let (text_content, images, tool_results) = process_message_content(&last_message.content)?;

    // 6. 转换工具定义
    let tools = convert_tools(&req.tools);

    // 7. 构建 UserInputMessageContext
    let mut context = UserInputMessageContext::new();
    if !tools.is_empty() {
        context = context.with_tools(tools);
    }
    if !tool_results.is_empty() {
        context = context.with_tool_results(tool_results.clone());
    }

    // 8. 构建当前消息
    // 保留文本内容，即使有工具结果也不丢弃用户文本
    let content = text_content;

    let mut user_input = UserInputMessage::new(content, &model_id)
        .with_context(context)
        .with_origin("AI_EDITOR");

    if !images.is_empty() {
        user_input = user_input.with_images(images);
    }

    let current_message = CurrentMessage::new(user_input);

    // 9. 构建历史消息
    let history = build_history(req, &model_id)?;

    // 10. 构建 ConversationState
    let conversation_state = ConversationState::new(conversation_id)
        .with_agent_continuation_id(agent_continuation_id)
        .with_agent_task_type("vibe")
        .with_chat_trigger_type(chat_trigger_type)
        .with_current_message(current_message)
        .with_history(history);

    Ok(ConversionResult {
        conversation_state
    })
}

/// 确定聊天触发类型
fn determine_chat_trigger_type(req: &MessagesRequest) -> String {
    if req.tools.is_some() {
        if let Some(ref tool_choice) = req.tool_choice {
            if let Some(tc_type) = tool_choice.get("type").and_then(|v| v.as_str()) {
                if tc_type == "any" || tc_type == "tool" {
                    return "AUTO".to_string();
                }
            }
        }
    }
    "MANUAL".to_string()
}

/// 处理消息内容，提取文本、图片和工具结果
fn process_message_content(
    content: &serde_json::Value
) -> Result<(String, Vec<KiroImage>, Vec<ToolResult>), ConversionError> {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();

    match content {
        serde_json::Value::String(s) => {
            text_parts.push(s.clone());
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "text" => {
                            if let Some(text) = block.text {
                                text_parts.push(text);
                            }
                        }
                        "image" => {
                            if let Some(source) = block.source {
                                if let Some(format) = get_image_format(&source.media_type) {
                                    images.push(KiroImage::from_base64(format, source.data));
                                }
                            }
                        }
                        "tool_result" => {
                            if let Some(tool_use_id) = block.tool_use_id {
                                let result_content = extract_tool_result_content(&block.content);
                                let is_error = block.is_error.unwrap_or(false);

                                let mut result = if is_error {
                                    ToolResult::error(&tool_use_id, result_content)
                                } else {
                                    ToolResult::success(&tool_use_id, result_content)
                                };
                                result.status = Some(if is_error { "error" } else { "success" }.to_string());

                                tool_results.push(result);
                            }
                        }
                        "tool_use" => {
                            // tool_use 在 assistant 消息中处理，这里忽略
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    Ok((text_parts.join("\n"), images, tool_results))
}

/// 从 media_type 获取图片格式
fn get_image_format(media_type: &str) -> Option<String> {
    match media_type {
        "image/jpeg" => Some("jpeg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        _ => None,
    }
}

/// 提取工具结果内容
fn extract_tool_result_content(content: &Option<serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
            parts.join("\n")
        }
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// 转换工具定义
fn convert_tools(tools: &Option<Vec<super::types::Tool>>) -> Vec<Tool> {
    let Some(tools) = tools else {
        return Vec::new();
    };

    tools
        .iter()
        .filter(|t| !is_unsupported_tool(&t.name))
        .map(|t| {
            let description = t.description.clone();
            // 限制描述长度为 10000 字符（安全截断 UTF-8，单次遍历）
            let description = match description.char_indices().nth(10000) {
                Some((idx, _)) => description[..idx].to_string(),
                None => description,
            };

            Tool {
                tool_specification: ToolSpecification {
                    name: t.name.clone(),
                    description,
                    input_schema: InputSchema::from_json(serde_json::json!(t.input_schema)),
                },
            }
        })
        .collect()
}

/// 检查是否为不支持的工具
fn is_unsupported_tool(name: &str) -> bool {
    matches!(name.to_lowercase().as_str(), "web_search" | "websearch")
}

/// 生成thinking标签前缀
fn generate_thinking_prefix(thinking: &Option<Thinking>) -> Option<String> {
    if let Some(t) = thinking {
        if t.thinking_type == "enabled" {
            return Some(format!(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>{}</max_thinking_length>",
                t.budget_tokens
            ));
        }
    }
    None
}

/// 检查内容是否已包含thinking标签
fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>") || content.contains("<max_thinking_length>")
}

/// 构建历史消息
fn build_history(
    req: &MessagesRequest,
    model_id: &str,
) -> Result<Vec<Message>, ConversionError> {
    let mut history = Vec::new();

    // 生成thinking前缀（如果需要）
    let thinking_prefix = generate_thinking_prefix(&req.thinking);

    // 1. 处理系统消息
    if let Some(ref system) = req.system {
        let system_content: String = system
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
            .join("\n");

        if !system_content.is_empty() {
            // 注入thinking标签到系统消息最前面（如果需要且不存在）
            let final_content = if let Some(ref prefix) = thinking_prefix {
                if !has_thinking_tags(&system_content) {
                    format!("{}\n{}", prefix, system_content)
                } else {
                    system_content
                }
            } else {
                system_content
            };

            // 系统消息作为 user + assistant 配对
            let user_msg = HistoryUserMessage::new(final_content, model_id);
            history.push(Message::User(user_msg));

            let assistant_msg = HistoryAssistantMessage::new("I will follow these instructions.");
            history.push(Message::Assistant(assistant_msg));
        }
    } else if let Some(ref prefix) = thinking_prefix {
        // 没有系统消息但有thinking配置，插入新的系统消息
        let user_msg = HistoryUserMessage::new(prefix.clone(), model_id);
        history.push(Message::User(user_msg));

        let assistant_msg = HistoryAssistantMessage::new("I will follow these instructions.");
        history.push(Message::Assistant(assistant_msg));
    }

    // 2. 处理常规消息历史
    // 最后一条消息作为 currentMessage，不加入历史
    let history_end_index = req.messages.len().saturating_sub(1);

    // 如果最后一条是 assistant，则包含在历史中
    let last_is_assistant = req
        .messages
        .last()
        .map(|m| m.role == "assistant")
        .unwrap_or(false);

    let history_end_index = if last_is_assistant {
        req.messages.len()
    } else {
        history_end_index
    };

    // 收集并配对消息
    let mut user_buffer: Vec<&super::types::Message> = Vec::new();

    for i in 0..history_end_index {
        let msg = &req.messages[i];

        if msg.role == "user" {
            user_buffer.push(msg);
        } else if msg.role == "assistant" {
            // 遇到 assistant，处理累积的 user 消息
            if !user_buffer.is_empty() {
                let merged_user = merge_user_messages(&user_buffer, model_id)?;
                history.push(Message::User(merged_user));
                user_buffer.clear();

                // 添加 assistant 消息
                let assistant = convert_assistant_message(msg)?;
                history.push(Message::Assistant(assistant));
            }
        }
    }

    // 处理结尾的孤立 user 消息
    if !user_buffer.is_empty() {
        let merged_user = merge_user_messages(&user_buffer, model_id)?;
        history.push(Message::User(merged_user));

        // 自动配对一个 "OK" 的 assistant 响应
        let auto_assistant = HistoryAssistantMessage::new("OK");
        history.push(Message::Assistant(auto_assistant));
    }

    Ok(history)
}

/// 合并多个 user 消息
fn merge_user_messages(
    messages: &[&super::types::Message],
    model_id: &str,
) -> Result<HistoryUserMessage, ConversionError> {
    let mut content_parts = Vec::new();
    let mut all_images = Vec::new();
    let mut all_tool_results = Vec::new();

    for msg in messages {
        let (text, images, tool_results) = process_message_content(&msg.content)?;
        if !text.is_empty() {
            content_parts.push(text);
        }
        all_images.extend(images);
        all_tool_results.extend(tool_results);
    }

    let content = content_parts.join("\n");
    // 保留文本内容，即使有工具结果也不丢弃用户文本
    let mut user_msg = UserMessage::new(&content, model_id);

    if !all_images.is_empty() {
        user_msg = user_msg.with_images(all_images);
    }

    if !all_tool_results.is_empty() {
        let mut ctx = UserInputMessageContext::new();
        ctx = ctx.with_tool_results(all_tool_results);
        user_msg = user_msg.with_context(ctx);
    }

    Ok(HistoryUserMessage {
        user_input_message: user_msg,
    })
}

/// 转换 assistant 消息
fn convert_assistant_message(
    msg: &super::types::Message,
) -> Result<HistoryAssistantMessage, ConversionError> {
    let mut thinking_content = String::new();
    let mut text_content = String::new();
    let mut tool_uses = Vec::new();

    match &msg.content {
        serde_json::Value::String(s) => {
            text_content = s.clone();
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "thinking" => {
                            if let Some(thinking) = block.thinking {
                                thinking_content.push_str(&thinking);
                            }
                        }
                        "text" => {
                            if let Some(text) = block.text {
                                text_content.push_str(&text);
                            }
                        }
                        "tool_use" => {
                            // 过滤不支持的工具
                            if let Some(ref name) = block.name {
                                if is_unsupported_tool(name) {
                                    continue;
                                }
                            }

                            if let (Some(id), Some(name)) = (block.id, block.name) {
                                let input = block.input.unwrap_or(serde_json::json!({}));
                                tool_uses.push(ToolUseEntry::new(id, name).with_input(input));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    // 组合 thinking 和 text 内容
    // 格式: <thinking>思考内容</thinking>\n\ntext内容
    let final_content = if !thinking_content.is_empty() {
        if !text_content.is_empty() {
            format!("<thinking>{}</thinking>\n\n{}", thinking_content, text_content)
        } else {
            format!("<thinking>{}</thinking>", thinking_content)
        }
    } else {
        text_content
    };

    let mut assistant = AssistantMessage::new(final_content);
    if !tool_uses.is_empty() {
        assistant = assistant.with_tool_uses(tool_uses);
    }

    Ok(HistoryAssistantMessage {
        assistant_response_message: assistant,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_model_sonnet() {
        assert!(map_model("claude-sonnet-4-20250514").unwrap().contains("sonnet"));
        assert!(map_model("claude-3-5-sonnet-20241022").unwrap().contains("sonnet"));
    }

    #[test]
    fn test_map_model_opus() {
        assert!(map_model("claude-opus-4-20250514").unwrap().contains("opus"));
    }

    #[test]
    fn test_map_model_haiku() {
        assert!(map_model("claude-haiku-4-20250514").unwrap().contains("haiku"));
    }

    #[test]
    fn test_map_model_unsupported() {
        assert!(map_model("gpt-4").is_none());
    }

    #[test]
    fn test_determine_chat_trigger_type() {
        // 无工具时返回 MANUAL
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
        };
        assert_eq!(determine_chat_trigger_type(&req), "MANUAL");
    }

    #[test]
    fn test_is_unsupported_tool() {
        assert!(is_unsupported_tool("web_search"));
        assert!(is_unsupported_tool("websearch"));
        assert!(is_unsupported_tool("WebSearch"));
        assert!(!is_unsupported_tool("read_file"));
    }
}
