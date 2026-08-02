//! Anthropic API 支持的模型列表常量
//!
//! 单独成模块以便：
//! 1. 让 `handlers::get_models` 退化为薄壳；
//! 2. 让模型清单的版本化变更（增删字段、新增模型）不再修改 handler。

use super::dto::Model;

/// Anthropic 兼容协议下当前支持的模型列表（顺序与对外 JSON 一致）。
///
/// 每次调用都会重新构造一份 owned `Vec<Model>`，避免 `Lazy` 的 `Send + Sync` 噪音
/// 也避免与 axum `Json` 共享所有权时的不必要约束。
/// 列表本身只有 18 项，重复构造的代价远低于运行时 lock。
pub fn supported_models() -> Vec<Model> {
    vec![
        Model {
            id: "claude-opus-5".to_string(),
            object: "model".to_string(),
            created: 1782777600, // Jun 30, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 128_000,
        },
        Model {
            id: "claude-opus-5-thinking".to_string(),
            object: "model".to_string(),
            created: 1782777600, // Jun 30, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 128_000,
        },
        Model {
            id: "claude-sonnet-5".to_string(),
            object: "model".to_string(),
            created: 1782777600, // Jun 30, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 128_000,
        },
        Model {
            id: "claude-sonnet-5-thinking".to_string(),
            object: "model".to_string(),
            created: 1782777600, // Jun 30, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 128_000,
        },
        Model {
            id: "claude-opus-4-8".to_string(),
            object: "model".to_string(),
            created: 1779897600, // May 28, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.8".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 128_000,
        },
        Model {
            id: "claude-opus-4-8-thinking".to_string(),
            object: "model".to_string(),
            created: 1779897600, // May 28, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.8 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 128_000,
        },
        Model {
            id: "claude-opus-4-7".to_string(),
            object: "model".to_string(),
            created: 1776276000, // Apr 16, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64_000,
        },
        Model {
            id: "claude-opus-4-7-thinking".to_string(),
            object: "model".to_string(),
            created: 1776276000, // Apr 16, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64_000,
        },
        Model {
            id: "claude-opus-4-6".to_string(),
            object: "model".to_string(),
            created: 1770163200, // Feb 4, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1770163200, // Feb 4, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-6".to_string(),
            object: "model".to_string(),
            created: 1771286400, // Feb 17, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1771286400, // Feb 17, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-5-20251101".to_string(),
            object: "model".to_string(),
            created: 1763942400, // Nov 24, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-opus-4-5-20251101-thinking".to_string(),
            object: "model".to_string(),
            created: 1763942400, // Nov 24, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-5-20250929".to_string(),
            object: "model".to_string(),
            created: 1759104000, // Sep 29, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-sonnet-4-5-20250929-thinking".to_string(),
            object: "model".to_string(),
            created: 1759104000, // Sep 29, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-haiku-4-5-20251001".to_string(),
            object: "model".to_string(),
            created: 1760486400, // Oct 15, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
        Model {
            id: "claude-haiku-4-5-20251001-thinking".to_string(),
            object: "model".to_string(),
            created: 1760486400, // Oct 15, 2025
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn supported_models_contains_expected_ids() {
        let models = supported_models();
        let ids: HashSet<&str> = models.iter().map(|m| m.id.as_str()).collect();
        for expected in [
            "claude-opus-5",
            "claude-opus-5-thinking",
            "claude-sonnet-5",
            "claude-sonnet-5-thinking",
            "claude-opus-4-8",
            "claude-opus-4-8-thinking",
            "claude-opus-4-7",
            "claude-opus-4-7-thinking",
            "claude-opus-4-6",
            "claude-opus-4-6-thinking",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6-thinking",
            "claude-opus-4-5-20251101",
            "claude-opus-4-5-20251101-thinking",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-5-20250929-thinking",
            "claude-haiku-4-5-20251001",
            "claude-haiku-4-5-20251001-thinking",
        ] {
            assert!(ids.contains(expected), "缺失模型 id: {expected}");
        }
    }

    #[test]
    fn supported_models_preserves_order() {
        let models = supported_models();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids[0], "claude-opus-5");
        assert_eq!(ids[2], "claude-sonnet-5");
        assert_eq!(ids[4], "claude-opus-4-8");
        assert_eq!(ids[6], "claude-opus-4-7");
        assert_eq!(ids[8], "claude-opus-4-6");
        assert_eq!(ids[10], "claude-sonnet-4-6");
        assert_eq!(ids[12], "claude-opus-4-5-20251101");
        assert_eq!(ids[16], "claude-haiku-4-5-20251001");
        assert_eq!(ids.len(), 18);
    }

    #[test]
    fn supported_models_have_expected_metadata() {
        for m in supported_models() {
            assert_eq!(m.model_type, "chat", "model={}", m.id);
            let expected_max_tokens = if m.id.starts_with("claude-opus-5")
                || m.id.starts_with("claude-sonnet-5")
                || m.id.starts_with("claude-opus-4-8")
            {
                128_000
            } else {
                64_000
            };
            assert_eq!(m.max_tokens, expected_max_tokens, "model={}", m.id);
            assert_eq!(m.owned_by, "anthropic", "model={}", m.id);
            assert_eq!(m.object, "model", "model={}", m.id);
        }
    }
}
