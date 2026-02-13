//! 工具调用截断检测模块
//!
//! 当 Kiro API 达到输出 token 上限时，工具调用的 JSON 可能被截断，
//! 导致参数不完整或无法解析。此模块检测截断并生成软失败消息引导重试。

use std::collections::{HashMap, HashSet};

/// 截断类型
#[derive(Debug, Clone, PartialEq)]
pub enum TruncationType {
    /// 无截断
    None,
    /// 输入完全为空
    EmptyInput,
    /// JSON 语法无效（截断在值中间）
    InvalidJson,
    /// JSON 解析成功但缺少关键字段
    MissingFields,
    /// 字符串值被截断
    IncompleteString,
}

/// 截断检测信息
#[derive(Debug, Clone)]
pub struct TruncationInfo {
    pub is_truncated: bool,
    pub truncation_type: TruncationType,
    pub tool_name: String,
    pub tool_use_id: String,
    pub raw_input: String,
    pub parsed_fields: HashMap<String, String>,
    pub error_message: String,
}

/// 已知的写入工具
fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "Write"
            | "write_to_file"
            | "fsWrite"
            | "create_file"
            | "edit_file"
            | "apply_diff"
            | "str_replace_editor"
            | "insert"
    )
}

/// 工具必需字段映射
fn required_fields(tool_name: &str) -> Option<&[&str]> {
    match tool_name {
        "Write" => Some(&["file_path", "content"]),
        "write_to_file" | "fsWrite" | "create_file" => Some(&["path", "content"]),
        "edit_file" => Some(&["path"]),
        "apply_diff" => Some(&["path", "diff"]),
        "str_replace_editor" => Some(&["path", "old_str", "new_str"]),
        "Bash" | "execute" | "run_command" => Some(&["command"]),
        _ => None,
    }
}

/// 检测工具输入是否被截断
pub fn detect_truncation(
    tool_name: &str,
    tool_use_id: &str,
    raw_input: &str,
    parsed_input: Option<&serde_json::Value>,
) -> TruncationInfo {
    let mut info = TruncationInfo {
        is_truncated: false,
        truncation_type: TruncationType::None,
        tool_name: tool_name.to_string(),
        tool_use_id: tool_use_id.to_string(),
        raw_input: raw_input.to_string(),
        parsed_fields: HashMap::new(),
        error_message: String::new(),
    };

    // 场景 1: 输入完全为空
    if raw_input.trim().is_empty() {
        info.is_truncated = true;
        info.truncation_type = TruncationType::EmptyInput;
        info.error_message =
            "Tool input was completely empty - API response may have been truncated".to_string();
        tracing::warn!(
            "截断检测 [empty_input] tool={} id={}: 输入为空",
            tool_name,
            tool_use_id
        );
        return info;
    }

    // 场景 2: JSON 解析失败
    let parsed = match parsed_input {
        Some(v) if v.is_object() && !v.as_object().unwrap().is_empty() => Some(v),
        _ => None,
    };

    if parsed.is_none() && looks_like_truncated_json(raw_input) {
        info.is_truncated = true;
        info.truncation_type = TruncationType::InvalidJson;
        info.parsed_fields = extract_partial_fields(raw_input);
        info.error_message = format!(
            "Tool input JSON was truncated mid-transmission ({} bytes received)",
            raw_input.len()
        );
        tracing::warn!(
            "截断检测 [invalid_json] tool={} id={}: JSON 解析失败, raw_len={}",
            tool_name,
            tool_use_id,
            raw_input.len()
        );
        return info;
    }

    // 场景 3: JSON 解析成功但缺少必需字段
    if let Some(parsed_val) = parsed {
        if let Some(obj) = parsed_val.as_object() {
            if let Some(required) = required_fields(tool_name) {
                let existing: HashSet<&str> = obj.keys().map(|k| k.as_str()).collect();
                let missing: Vec<&&str> = required
                    .iter()
                    .filter(|f| !existing.contains(**f))
                    .collect();

                if !missing.is_empty() {
                    info.is_truncated = true;
                    info.truncation_type = TruncationType::MissingFields;
                    info.parsed_fields = extract_parsed_field_names(obj);
                    info.error_message = format!(
                        "Tool '{}' missing required fields: {}",
                        tool_name,
                        missing.iter().map(|f| **f).collect::<Vec<_>>().join(", ")
                    );
                    tracing::warn!(
                        "截断检测 [missing_fields] tool={} id={}: 缺少字段 {:?}",
                        tool_name,
                        tool_use_id,
                        missing
                    );
                    return info;
                }
            }

            // 场景 4: 写入工具的内容字段被截断
            if is_write_tool(tool_name) {
                if let Some(msg) = detect_content_truncation(obj, raw_input) {
                    info.is_truncated = true;
                    info.truncation_type = TruncationType::IncompleteString;
                    info.parsed_fields = extract_parsed_field_names(obj);
                    info.error_message = msg;
                    tracing::warn!(
                        "截断检测 [incomplete_string] tool={} id={}: {}",
                        tool_name,
                        tool_use_id,
                        info.error_message
                    );
                    return info;
                }
            }
        }
    }

    info
}

/// 检查原始字符串是否看起来像被截断的 JSON
fn looks_like_truncated_json(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return false;
    }

    // 括号不平衡
    let open_braces = trimmed.matches('{').count();
    let close_braces = trimmed.matches('}').count();
    let open_brackets = trimmed.matches('[').count();
    let close_brackets = trimmed.matches(']').count();

    if open_braces > close_braces || open_brackets > close_brackets {
        return true;
    }

    // 末尾字符异常
    if let Some(last) = trimmed.bytes().last() {
        if last != b'}' && last != b']' && (last == b'"' || last == b':' || last == b',') {
            return true;
        }
    }

    // 未闭合的字符串（奇数个未转义引号）
    let mut in_string = false;
    let mut escaped = false;
    for b in trimmed.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if b == b'\\' {
            escaped = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
        }
    }
    if in_string {
        return true;
    }

    false
}

/// 从格式错误的 JSON 中提取部分字段名
fn extract_partial_fields(raw: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let trimmed = raw.trim().strip_prefix('{').unwrap_or(raw);

    for part in trimmed.split(',') {
        let part = part.trim();
        if let Some(colon_idx) = part.find(':') {
            let key = part[..colon_idx].trim().trim_matches('"');
            let value = part[colon_idx + 1..].trim();
            let display_value = if value.len() > 50 {
                value.chars().take(50).collect::<String>() + "..."
            } else {
                value.to_string()
            };
            fields.insert(key.to_string(), display_value);
        }
    }

    fields
}

/// 从已解析的 JSON 对象中提取字段名
fn extract_parsed_field_names(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for (key, val) in obj {
        let display = match val {
            serde_json::Value::String(s) => {
                if s.len() > 50 {
                    format!("{}...", &s[..50])
                } else {
                    s.clone()
                }
            }
            serde_json::Value::Null => "<null>".to_string(),
            _ => "<present>".to_string(),
        };
        fields.insert(key.clone(), display);
    }
    fields
}

/// 检测写入工具的内容字段是否被截断
fn detect_content_truncation(
    obj: &serde_json::Map<String, serde_json::Value>,
    raw_input: &str,
) -> Option<String> {
    let content = obj.get("content")?.as_str()?;

    // 启发式：原始输入很大但内容字段异常短
    if raw_input.len() > 1000 && content.len() < 100 {
        return Some(
            "content field appears suspiciously short compared to raw input size".to_string(),
        );
    }

    // 检查未闭合的代码块
    if content.contains("```") {
        let fence_count = content.matches("```").count();
        if fence_count % 2 != 0 {
            return Some(
                "content contains unclosed code fence (```) suggesting truncation".to_string(),
            );
        }
    }

    None
}

/// 构建软失败工具结果消息
///
/// 当检测到截断时，返回此消息作为 tool_result 引导 Claude 重试
pub fn build_soft_failure_result(info: &TruncationInfo) -> String {
    let max_line_hint = match info.truncation_type {
        TruncationType::EmptyInput => 200,
        TruncationType::InvalidJson => 250,
        TruncationType::MissingFields => 300,
        TruncationType::IncompleteString => 350,
        TruncationType::None => 300,
    };

    let reason = match info.truncation_type {
        TruncationType::EmptyInput => {
            "Your tool call was too large and the input was completely lost during transmission."
        }
        TruncationType::InvalidJson => {
            "Your tool call was truncated mid-transmission, resulting in incomplete JSON."
        }
        TruncationType::MissingFields => {
            "Your tool call was partially received but critical fields were cut off."
        }
        TruncationType::IncompleteString => {
            "Your tool call content was truncated - the full content did not arrive."
        }
        TruncationType::None => {
            "Your tool call was truncated by the API due to output size limits."
        }
    };

    let mut result = format!(
        "TOOL_CALL_INCOMPLETE\nstatus: incomplete\nreason: {}\n",
        reason
    );

    if !info.parsed_fields.is_empty() {
        let fields: Vec<String> = info
            .parsed_fields
            .iter()
            .map(|(k, v)| {
                let display_v: String = if v.len() > 30 {
                    v.chars().take(30).collect::<String>() + "..."
                } else {
                    v.clone()
                };
                format!("{}={}", k, display_v)
            })
            .collect();
        result.push_str(&format!(
            "context: Received partial data: {}\n",
            fields.join(", ")
        ));
    }

    result.push_str(&format!(
        "\nCONCLUSION: Split your output into smaller chunks and retry.\n\
         \n\
         REQUIRED APPROACH:\n\
         1. For file writes: Write in chunks of ~{} lines maximum\n\
         2. For new files: First create with initial chunk, then append remaining sections\n\
         3. For edits: Make surgical, targeted changes - avoid rewriting entire files\n\
         \n\
         DO NOT attempt to write the full content again in a single call.\n\
         The API has a hard output limit that cannot be bypassed.\n",
        max_line_hint
    ));

    result
}
