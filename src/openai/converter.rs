//! OpenAI Chat Completions -> Anthropic Messages 转换

use std::collections::HashMap;

use serde_json::json;

use crate::anthropic::types::{
    Message as AnthropicMessage, MessagesRequest, Metadata, SystemMessage, Tool as AnthropicTool,
};

use super::types::{
    ChatCompletionsRequest, ChatMessage, ContentPart, ImageUrlField, MessageContent,
    ResponseContentPart, ResponseInput, ResponseInputItem, ResponseItemContent,
    ResponseOutputContent, ResponsesRequest, ToolDefinition,
};

/// OpenAI 转换错误
#[derive(Debug)]
pub enum ConversionError {
    EmptyMessages,
    UnsupportedRole(String),
    UnsupportedContentPart(String),
    UnsupportedImageUrl(String),
    UnsupportedToolType(String),
    MissingToolCallId,
    InvalidToolArguments(String),
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessages => write!(f, "messages 不能为空"),
            Self::UnsupportedRole(role) => write!(f, "不支持的消息角色: {}", role),
            Self::UnsupportedContentPart(part) => write!(f, "不支持的内容块类型: {}", part),
            Self::UnsupportedImageUrl(url) => {
                write!(f, "仅支持 data URL 格式图片，收到: {}", url)
            }
            Self::UnsupportedToolType(tool_type) => {
                write!(f, "仅支持 function 工具，收到: {}", tool_type)
            }
            Self::MissingToolCallId => write!(f, "tool 消息缺少 tool_call_id"),
            Self::InvalidToolArguments(msg) => write!(f, "工具参数不是合法 JSON: {}", msg),
        }
    }
}

impl std::error::Error for ConversionError {}

/// 转换 OpenAI Chat Completions 请求
pub fn convert_chat_request(
    req: &ChatCompletionsRequest,
) -> Result<MessagesRequest, ConversionError> {
    if req.messages.is_empty() {
        return Err(ConversionError::EmptyMessages);
    }

    let mut system_messages = Vec::new();
    let mut messages = Vec::new();

    for message in &req.messages {
        match message.role.as_str() {
            "system" | "developer" => {
                let text = flatten_text_content(message.content.as_ref())?;
                if !text.is_empty() {
                    system_messages.push(SystemMessage { text });
                }
            }
            "user" => messages.push(AnthropicMessage {
                role: "user".to_string(),
                content: convert_user_content(message.content.as_ref())?,
            }),
            "assistant" => messages.push(AnthropicMessage {
                role: "assistant".to_string(),
                content: convert_assistant_content(message)?,
            }),
            "tool" => messages.push(AnthropicMessage {
                role: "user".to_string(),
                content: convert_tool_content(message)?,
            }),
            other => return Err(ConversionError::UnsupportedRole(other.to_string())),
        }
    }

    Ok(MessagesRequest {
        model: req.model.clone(),
        max_tokens: req.max_completion_tokens.or(req.max_tokens).unwrap_or(4096),
        messages,
        stream: req.stream,
        system: (!system_messages.is_empty()).then_some(system_messages),
        tools: convert_tools(req.tools.as_ref())?,
        tool_choice: req.tool_choice.clone(),
        thinking: None,
        output_config: None,
        metadata: req.user.as_ref().map(|user| Metadata {
            user_id: Some(user.clone()),
        }),
    })
}

/// 转换 Responses 请求
pub fn convert_responses_request(
    req: &ResponsesRequest,
) -> Result<MessagesRequest, ConversionError> {
    let mut system_messages = Vec::new();
    if let Some(instructions) = &req.instructions {
        if !instructions.is_empty() {
            system_messages.push(SystemMessage {
                text: instructions.clone(),
            });
        }
    }

    let messages = match req.input.as_ref() {
        Some(ResponseInput::Text(text)) => vec![AnthropicMessage {
            role: "user".to_string(),
            content: json!(text),
        }],
        Some(ResponseInput::Items(items)) => convert_responses_items(items, &mut system_messages)?,
        None => Vec::new(),
    };

    if messages.is_empty() {
        return Err(ConversionError::EmptyMessages);
    }

    Ok(MessagesRequest {
        model: req.model.clone(),
        max_tokens: req.max_output_tokens.unwrap_or(4096),
        messages,
        stream: req.stream,
        system: (!system_messages.is_empty()).then_some(system_messages),
        tools: convert_tools(req.tools.as_ref())?,
        tool_choice: req.tool_choice.clone(),
        thinking: None,
        output_config: None,
        metadata: req.user.as_ref().map(|user| Metadata {
            user_id: Some(user.clone()),
        }),
    })
}

fn flatten_text_content(content: Option<&MessageContent>) -> Result<String, ConversionError> {
    let Some(content) = content else {
        return Ok(String::new());
    };

    match content {
        MessageContent::Text(text) => Ok(text.clone()),
        MessageContent::Parts(parts) => {
            let mut text_parts = Vec::new();
            for part in parts {
                match part.part_type.as_str() {
                    "text" => {
                        if let Some(text) = &part.text {
                            text_parts.push(text.clone());
                        }
                    }
                    other => {
                        return Err(ConversionError::UnsupportedContentPart(other.to_string()));
                    }
                }
            }
            Ok(text_parts.join("\n"))
        }
    }
}

fn convert_user_content(
    content: Option<&MessageContent>,
) -> Result<serde_json::Value, ConversionError> {
    let Some(content) = content else {
        return Ok(json!(""));
    };

    match content {
        MessageContent::Text(text) => Ok(json!(text)),
        MessageContent::Parts(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                blocks.push(convert_content_part(part)?);
            }
            Ok(json!(blocks))
        }
    }
}

fn convert_assistant_content(message: &ChatMessage) -> Result<serde_json::Value, ConversionError> {
    let mut blocks = Vec::new();

    if let Some(content) = message.content.as_ref() {
        match content {
            MessageContent::Text(text) => {
                if !text.is_empty() {
                    blocks.push(json!({
                        "type": "text",
                        "text": text
                    }));
                }
            }
            MessageContent::Parts(parts) => {
                for part in parts {
                    match part.part_type.as_str() {
                        "text" => {
                            if let Some(text) = &part.text {
                                blocks.push(json!({
                                    "type": "text",
                                    "text": text
                                }));
                            }
                        }
                        other => {
                            return Err(ConversionError::UnsupportedContentPart(other.to_string()));
                        }
                    }
                }
            }
        }
    }

    if let Some(tool_calls) = &message.tool_calls {
        for tool_call in tool_calls {
            let input = parse_tool_arguments(&tool_call.function.arguments)?;
            blocks.push(json!({
                "type": "tool_use",
                "id": tool_call.id,
                "name": tool_call.function.name,
                "input": input
            }));
        }
    }

    if blocks.is_empty() {
        Ok(json!(""))
    } else if blocks.len() == 1
        && message.tool_calls.is_none()
        && blocks[0].get("type").and_then(|v| v.as_str()) == Some("text")
    {
        Ok(blocks[0]["text"].clone())
    } else {
        Ok(json!(blocks))
    }
}

fn convert_tool_content(message: &ChatMessage) -> Result<serde_json::Value, ConversionError> {
    let tool_call_id = message
        .tool_call_id
        .as_ref()
        .ok_or(ConversionError::MissingToolCallId)?;
    let content = convert_tool_result_content(message.content.as_ref())?;

    Ok(json!([
        {
            "type": "tool_result",
            "tool_use_id": tool_call_id,
            "content": content,
            "is_error": false
        }
    ]))
}

fn convert_tool_result_content(
    content: Option<&MessageContent>,
) -> Result<serde_json::Value, ConversionError> {
    let Some(content) = content else {
        return Ok(json!(""));
    };

    match content {
        MessageContent::Text(text) => Ok(json!(text)),
        MessageContent::Parts(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                match part.part_type.as_str() {
                    "text" => {
                        if let Some(text) = &part.text {
                            blocks.push(json!({
                                "type": "text",
                                "text": text
                            }));
                        }
                    }
                    other => {
                        return Err(ConversionError::UnsupportedContentPart(other.to_string()));
                    }
                }
            }
            Ok(json!(blocks))
        }
    }
}

fn convert_content_part(part: &ContentPart) -> Result<serde_json::Value, ConversionError> {
    match part.part_type.as_str() {
        "text" => Ok(json!({
            "type": "text",
            "text": part.text.clone().unwrap_or_default()
        })),
        "image_url" => convert_image_part(part.image_url.as_ref()),
        other => Err(ConversionError::UnsupportedContentPart(other.to_string())),
    }
}

fn convert_responses_items(
    items: &[ResponseInputItem],
    system_messages: &mut Vec<SystemMessage>,
) -> Result<Vec<AnthropicMessage>, ConversionError> {
    let mut messages = Vec::new();

    for item in items {
        match item.item_type.as_deref() {
            Some("function_call_output") => messages.push(AnthropicMessage {
                role: "user".to_string(),
                content: convert_function_call_output(item)?,
            }),
            Some("function_call") => messages.push(AnthropicMessage {
                role: "assistant".to_string(),
                content: convert_function_call_item(item)?,
            }),
            Some("message") | None => {
                let role = item
                    .role
                    .as_deref()
                    .ok_or_else(|| ConversionError::UnsupportedRole("missing role".to_string()))?;

                match role {
                    "system" | "developer" => {
                        let text = flatten_response_text(item.content.as_ref())?;
                        if !text.is_empty() {
                            system_messages.push(SystemMessage { text });
                        }
                    }
                    "user" => messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: convert_response_message_content(item.content.as_ref())?,
                    }),
                    "assistant" => messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: convert_response_assistant_content(item)?,
                    }),
                    other => return Err(ConversionError::UnsupportedRole(other.to_string())),
                }
            }
            Some(other) => return Err(ConversionError::UnsupportedContentPart(other.to_string())),
        }
    }

    Ok(messages)
}

fn convert_response_message_content(
    content: Option<&ResponseItemContent>,
) -> Result<serde_json::Value, ConversionError> {
    let Some(content) = content else {
        return Ok(json!(""));
    };

    match content {
        ResponseItemContent::Text(text) => Ok(json!(text)),
        ResponseItemContent::Parts(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                blocks.push(convert_response_content_part(part)?);
            }
            Ok(json!(blocks))
        }
    }
}

fn convert_response_assistant_content(
    item: &ResponseInputItem,
) -> Result<serde_json::Value, ConversionError> {
    let Some(content) = item.content.as_ref() else {
        return Ok(json!(""));
    };

    match content {
        ResponseItemContent::Text(text) => Ok(json!(text)),
        ResponseItemContent::Parts(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                match part.part_type.as_str() {
                    "text" | "input_text" | "output_text" => {
                        if let Some(text) = &part.text {
                            blocks.push(json!({
                                "type": "text",
                                "text": text
                            }));
                        }
                    }
                    other => {
                        return Err(ConversionError::UnsupportedContentPart(other.to_string()));
                    }
                }
            }
            Ok(json!(blocks))
        }
    }
}

fn convert_function_call_item(
    item: &ResponseInputItem,
) -> Result<serde_json::Value, ConversionError> {
    let call_id = item
        .call_id
        .as_ref()
        .ok_or(ConversionError::MissingToolCallId)?;
    let name = item.name.as_ref().ok_or_else(|| {
        ConversionError::UnsupportedContentPart("function_call missing name".to_string())
    })?;
    let arguments = parse_tool_arguments(item.arguments.as_deref().unwrap_or(""))?;

    Ok(json!([
        {
            "type": "tool_use",
            "id": call_id,
            "name": name,
            "input": arguments
        }
    ]))
}

fn convert_function_call_output(
    item: &ResponseInputItem,
) -> Result<serde_json::Value, ConversionError> {
    let call_id = item
        .call_id
        .as_ref()
        .ok_or(ConversionError::MissingToolCallId)?;
    let output = convert_response_output_content(item.output.as_ref())?;

    Ok(json!([
        {
            "type": "tool_result",
            "tool_use_id": call_id,
            "content": output,
            "is_error": false
        }
    ]))
}

fn flatten_response_text(content: Option<&ResponseItemContent>) -> Result<String, ConversionError> {
    let Some(content) = content else {
        return Ok(String::new());
    };

    match content {
        ResponseItemContent::Text(text) => Ok(text.clone()),
        ResponseItemContent::Parts(parts) => {
            let mut values = Vec::new();
            for part in parts {
                match part.part_type.as_str() {
                    "text" | "input_text" | "output_text" => {
                        if let Some(text) = &part.text {
                            values.push(text.clone());
                        }
                    }
                    other => {
                        return Err(ConversionError::UnsupportedContentPart(other.to_string()));
                    }
                }
            }
            Ok(values.join("\n"))
        }
    }
}

fn convert_response_content_part(
    part: &ResponseContentPart,
) -> Result<serde_json::Value, ConversionError> {
    match part.part_type.as_str() {
        "text" | "input_text" | "output_text" => Ok(json!({
            "type": "text",
            "text": part.text.clone().unwrap_or_default()
        })),
        "input_image" | "image_url" => convert_image_part(part.image_url.as_ref()),
        other => Err(ConversionError::UnsupportedContentPart(other.to_string())),
    }
}

fn convert_response_output_content(
    output: Option<&ResponseOutputContent>,
) -> Result<serde_json::Value, ConversionError> {
    let Some(output) = output else {
        return Ok(json!(""));
    };

    match output {
        ResponseOutputContent::Text(text) => Ok(json!(text)),
        ResponseOutputContent::Parts(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                match part.part_type.as_str() {
                    "text" | "output_text" => {
                        if let Some(text) = &part.text {
                            blocks.push(json!({
                                "type": "text",
                                "text": text
                            }));
                        }
                    }
                    other => {
                        return Err(ConversionError::UnsupportedContentPart(other.to_string()));
                    }
                }
            }
            Ok(json!(blocks))
        }
    }
}

fn convert_image_part(
    image_url: Option<&ImageUrlField>,
) -> Result<serde_json::Value, ConversionError> {
    let Some(image_url) = image_url else {
        return Err(ConversionError::UnsupportedImageUrl(String::new()));
    };

    let url = image_url.url();
    let Some(rest) = url.strip_prefix("data:") else {
        return Err(ConversionError::UnsupportedImageUrl(url.to_string()));
    };

    let Some((media_type, data)) = rest.split_once(";base64,") else {
        return Err(ConversionError::UnsupportedImageUrl(url.to_string()));
    };

    Ok(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data
        }
    }))
}

fn convert_tools(
    tools: Option<&Vec<ToolDefinition>>,
) -> Result<Option<Vec<AnthropicTool>>, ConversionError> {
    let Some(tools) = tools else {
        return Ok(None);
    };

    let mut anthropic_tools = Vec::new();
    for tool in tools {
        if tool.tool_type != "function" {
            return Err(ConversionError::UnsupportedToolType(tool.tool_type.clone()));
        }

        anthropic_tools.push(AnthropicTool {
            tool_type: None,
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            input_schema: schema_to_hashmap(&tool.function.parameters),
            max_uses: None,
        });
    }

    Ok(Some(anthropic_tools))
}

fn schema_to_hashmap(schema: &serde_json::Value) -> HashMap<String, serde_json::Value> {
    use crate::anthropic::converter::normalize_json_schema;

    // 规范化 JSON Schema，修复 required: null、properties: null 等问题
    let normalized = normalize_json_schema(schema.clone());

    match normalized {
        serde_json::Value::Object(map) => map.into_iter().collect(),
        _ => json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": true
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect(),
    }
}

fn parse_tool_arguments(arguments: &str) -> Result<serde_json::Value, ConversionError> {
    if arguments.trim().is_empty() {
        return Ok(json!({}));
    }

    serde_json::from_str(arguments)
        .map_err(|e| ConversionError::InvalidToolArguments(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::types::{
        ChatMessage, FunctionDefinition, ResponseInput, ResponseInputItem, ResponseItemContent,
        ResponseOutputContent, ResponsesRequest, ToolCall, ToolCallFunction,
    };

    #[test]
    fn test_convert_chat_request_with_tool_history() {
        let req = ChatCompletionsRequest {
            model: "claude-sonnet-4-6".to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: Some(MessageContent::Text("You are helpful.".to_string())),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: Some(MessageContent::Text("Read the file".to_string())),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                ChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_123".to_string(),
                        call_type: "function".to_string(),
                        function: ToolCallFunction {
                            name: "read_file".to_string(),
                            arguments: "{\"path\":\"/tmp/a.txt\"}".to_string(),
                        },
                    }]),
                    tool_call_id: None,
                    name: None,
                },
                ChatMessage {
                    role: "tool".to_string(),
                    content: Some(MessageContent::Text("file content".to_string())),
                    tool_calls: None,
                    tool_call_id: Some("call_123".to_string()),
                    name: Some("read_file".to_string()),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: Some(MessageContent::Text("Summarize it".to_string())),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            ],
            stream: false,
            max_tokens: Some(1024),
            max_completion_tokens: None,
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "read_file".to_string(),
                    description: "Read a file".to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        },
                        "required": ["path"]
                    }),
                },
            }]),
            tool_choice: None,
            user: Some("user-1".to_string()),
            stream_options: None,
            reasoning_effort: None,
        };

        let converted = convert_chat_request(&req).expect("should convert");

        assert_eq!(converted.system.as_ref().map(|v| v.len()), Some(1));
        assert_eq!(converted.messages.len(), 4);
        assert_eq!(converted.messages[1].role, "assistant");
        assert_eq!(converted.messages[2].role, "user");
        assert_eq!(
            converted.messages[2].content[0]["type"],
            serde_json::Value::String("tool_result".to_string())
        );
    }

    #[test]
    fn test_convert_chat_request_rejects_remote_image() {
        let req = ChatCompletionsRequest {
            model: "claude-sonnet-4-6".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some(MessageContent::Parts(vec![ContentPart {
                    part_type: "image_url".to_string(),
                    text: None,
                    image_url: Some(ImageUrlField::Object {
                        url: "https://example.com/a.png".to_string(),
                        detail: None,
                    }),
                }])),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
            stream: false,
            max_tokens: None,
            max_completion_tokens: None,
            tools: None,
            tool_choice: None,
            user: None,
            stream_options: None,
            reasoning_effort: None,
        };

        let err = convert_chat_request(&req).expect_err("should reject");
        assert!(err.to_string().contains("data URL"));
    }

    #[test]
    fn test_convert_responses_request_with_function_items() {
        let req = ResponsesRequest {
            model: "claude-sonnet-4-6".to_string(),
            input: Some(ResponseInput::Items(vec![
                ResponseInputItem {
                    item_type: Some("message".to_string()),
                    role: Some("user".to_string()),
                    content: Some(ResponseItemContent::Text("Read file".to_string())),
                    call_id: None,
                    output: None,
                    name: None,
                    arguments: None,
                },
                ResponseInputItem {
                    item_type: Some("function_call".to_string()),
                    role: None,
                    content: None,
                    call_id: Some("call_1".to_string()),
                    output: None,
                    name: Some("read_file".to_string()),
                    arguments: Some("{\"path\":\"/tmp/a.txt\"}".to_string()),
                },
                ResponseInputItem {
                    item_type: Some("function_call_output".to_string()),
                    role: None,
                    content: None,
                    call_id: Some("call_1".to_string()),
                    output: Some(ResponseOutputContent::Text("hello".to_string())),
                    name: None,
                    arguments: None,
                },
                ResponseInputItem {
                    item_type: Some("message".to_string()),
                    role: Some("user".to_string()),
                    content: Some(ResponseItemContent::Text("Summarize".to_string())),
                    call_id: None,
                    output: None,
                    name: None,
                    arguments: None,
                },
            ])),
            instructions: Some("Be concise".to_string()),
            stream: false,
            max_output_tokens: Some(512),
            tools: None,
            tool_choice: None,
            user: None,
            stream_options: None,
        };

        let converted = convert_responses_request(&req).expect("should convert");
        assert_eq!(converted.system.as_ref().map(|v| v.len()), Some(1));
        assert_eq!(converted.messages.len(), 4);
        assert_eq!(converted.messages[1].role, "assistant");
        assert_eq!(converted.messages[2].role, "user");
        assert_eq!(
            converted.messages[2].content[0]["type"],
            serde_json::Value::String("tool_result".to_string())
        );
    }
}
