//! 工具压缩模块
//!
//! 当工具定义总大小超过目标阈值时，动态压缩工具 payload 以防止 Kiro API 500 错误。
//! 压缩策略：
//! 1. 简化 input_schema（仅保留 type/enum/required）
//! 2. 按比例压缩 description（最小 50 字符）

use crate::kiro::model::requests::tool::{InputSchema, Tool, ToolSpecification};

/// 工具压缩目标大小（20KB）
const TOOL_COMPRESSION_TARGET_SIZE: usize = 20 * 1024;

/// 压缩后描述最小长度
const MIN_TOOL_DESCRIPTION_LENGTH: usize = 50;

/// 计算工具列表的 JSON 序列化大小
fn calculate_tools_size(tools: &[Tool]) -> usize {
    serde_json::to_string(tools).map(|s| s.len()).unwrap_or(0)
}

/// 简化 input_schema，仅保留 type/enum/required/properties/items 等必要字段
fn simplify_input_schema(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(map) => {
            let mut simplified = serde_json::Map::new();

            // 保留必要字段
            for key in &["type", "enum", "required"] {
                if let Some(v) = map.get(*key) {
                    simplified.insert(key.to_string(), v.clone());
                }
            }

            // 递归处理 properties
            if let Some(serde_json::Value::Object(props)) = map.get("properties") {
                let mut simplified_props = serde_json::Map::new();
                for (key, value) in props {
                    simplified_props.insert(key.clone(), simplify_input_schema(value));
                }
                simplified.insert(
                    "properties".to_string(),
                    serde_json::Value::Object(simplified_props),
                );
            }

            // 处理 items（数组类型）
            if let Some(items) = map.get("items") {
                simplified.insert("items".to_string(), simplify_input_schema(items));
            }

            // 处理 additionalProperties
            if let Some(ap) = map.get("additionalProperties") {
                simplified.insert(
                    "additionalProperties".to_string(),
                    simplify_input_schema(ap),
                );
            }

            // 处理 anyOf/oneOf/allOf
            for key in &["anyOf", "oneOf", "allOf"] {
                if let Some(serde_json::Value::Array(arr)) = map.get(*key) {
                    let simplified_arr: Vec<serde_json::Value> =
                        arr.iter().map(simplify_input_schema).collect();
                    simplified.insert(key.to_string(), serde_json::Value::Array(simplified_arr));
                }
            }

            serde_json::Value::Object(simplified)
        }
        other => other.clone(),
    }
}

/// 压缩工具描述到目标长度（UTF-8 安全截断）
fn compress_description(description: &str, target_length: usize) -> String {
    let target = target_length.max(MIN_TOOL_DESCRIPTION_LENGTH);

    if description.len() <= target {
        return description.to_string();
    }

    let trunc_len = target.saturating_sub(3); // 留空间给 "..."

    // 找到有效的 UTF-8 字符边界
    let safe_len = description
        .char_indices()
        .take_while(|(i, _)| *i < trunc_len)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);

    if safe_len == 0 {
        return description
            .chars()
            .take(MIN_TOOL_DESCRIPTION_LENGTH)
            .collect();
    }

    format!("{}...", &description[..safe_len])
}

/// 如果工具总大小超过阈值则压缩
///
/// 返回压缩后的工具列表（如果不需要压缩则返回原列表的克隆）
pub fn compress_tools_if_needed(tools: &[Tool]) -> Vec<Tool> {
    if tools.is_empty() {
        return tools.to_vec();
    }

    let original_size = calculate_tools_size(tools);
    if original_size <= TOOL_COMPRESSION_TARGET_SIZE {
        tracing::debug!(
            "工具大小 {} 字节在目标 {} 字节内，无需压缩",
            original_size,
            TOOL_COMPRESSION_TARGET_SIZE
        );
        return tools.to_vec();
    }

    tracing::info!(
        "工具大小 {} 字节超过目标 {} 字节，开始压缩",
        original_size,
        TOOL_COMPRESSION_TARGET_SIZE
    );

    // 第一步：简化 input_schema
    let mut compressed: Vec<Tool> = tools
        .iter()
        .map(|t| {
            let simplified_schema = simplify_input_schema(&t.tool_specification.input_schema.json);
            Tool {
                tool_specification: ToolSpecification {
                    name: t.tool_specification.name.clone(),
                    description: t.tool_specification.description.clone(),
                    input_schema: InputSchema {
                        json: simplified_schema,
                    },
                },
            }
        })
        .collect();

    let size_after_schema = calculate_tools_size(&compressed);
    tracing::debug!(
        "schema 简化后大小: {} 字节 (减少 {} 字节)",
        size_after_schema,
        original_size - size_after_schema
    );

    if size_after_schema <= TOOL_COMPRESSION_TARGET_SIZE {
        tracing::info!("schema 简化后已达标，最终大小: {} 字节", size_after_schema);
        return compressed;
    }

    // 第二步：按比例压缩 description
    let size_to_reduce = size_after_schema - TOOL_COMPRESSION_TARGET_SIZE;
    let total_desc_len: usize = compressed
        .iter()
        .map(|t| t.tool_specification.description.len())
        .sum();

    if total_desc_len > 0 {
        let keep_ratio = 1.0 - (size_to_reduce as f64 / total_desc_len as f64);
        let keep_ratio = keep_ratio.clamp(0.0, 1.0);

        for tool in &mut compressed {
            let desc = &tool.tool_specification.description;
            let target_len = (desc.len() as f64 * keep_ratio) as usize;
            tool.tool_specification.description = compress_description(desc, target_len);
        }
    }

    let final_size = calculate_tools_size(&compressed);
    tracing::info!(
        "压缩完成，原始: {} 字节, 最终: {} 字节 ({:.1}% 减少)",
        original_size,
        final_size,
        (original_size - final_size) as f64 / original_size as f64 * 100.0
    );

    compressed
}
