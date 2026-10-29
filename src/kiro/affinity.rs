//! 会话粘连。
//!
//! 同一个对话应落到同一个 Kiro 账号上，上游缓存才有机会命中。
//! 键的来源与 new-api 的渠道亲和一致：Claude 的 `metadata.user_id`，
//! Codex 的 `prompt_cache_key`。两者都没有时，用 system 加第一条 user
//! 消息做指纹。这个键只在本地选账号，不会发给上游。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

const MIN_EXPLICIT_KEY_LEN: usize = 8;
const FINGERPRINT_TEXT_LIMIT: usize = 4_000;
const DEFAULT_TTL: Duration = Duration::from_secs(3_600);
const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct Pin {
    credential_id: u64,
    last_used: Instant,
}

#[derive(Debug, Default)]
pub struct AffinityTable {
    entries: HashMap<String, Pin>,
}

impl AffinityTable {
    /// 命中未过期的账号时继续用它。账号已不可用，或键是第一次出现时，
    /// 在 `available` 里选号并记住。顺序是：当前粘连数、成功次数、priority、id。
    /// `available` 的三元组是 `(id, success_count, priority)`。
    pub fn resolve(
        &mut self,
        key: &str,
        available: &[(u64, u64, u32)],
        now: Instant,
    ) -> Option<u64> {
        self.evict(now);
        if let Some(pin) = self.entries.get_mut(key) {
            let fresh = now.saturating_duration_since(pin.last_used) < DEFAULT_TTL;
            let usable = available.iter().any(|(id, _, _)| *id == pin.credential_id);
            if fresh && usable {
                pin.last_used = now;
                return Some(pin.credential_id);
            }
        }

        let (credential_id, _, _) =
            available
                .iter()
                .min_by_key(|(id, success_count, priority)| {
                    (self.pin_count(*id), *success_count, *priority, *id)
                })?;
        self.entries.insert(
            key.to_string(),
            Pin {
                credential_id: *credential_id,
                last_used: now,
            },
        );
        self.evict(now);
        Some(*credential_id)
    }

    /// 把键固定到已经选定的账号。账号不在可用列表里时不记录。
    pub fn pin(
        &mut self,
        key: &str,
        credential_id: u64,
        available: &[(u64, u64, u32)],
        now: Instant,
    ) {
        if !available.iter().any(|(id, _, _)| *id == credential_id) {
            return;
        }
        self.entries.insert(
            key.to_string(),
            Pin {
                credential_id,
                last_used: now,
            },
        );
        self.evict(now);
    }

    pub fn pin_count(&self, credential_id: u64) -> usize {
        self.entries
            .values()
            .filter(|pin| pin.credential_id == credential_id)
            .count()
    }

    fn evict(&mut self, now: Instant) {
        self.entries
            .retain(|_, pin| now.saturating_duration_since(pin.last_used) < DEFAULT_TTL);
        if self.entries.len() <= MAX_ENTRIES {
            return;
        }
        let mut oldest: Vec<(Instant, String)> = self
            .entries
            .iter()
            .map(|(key, pin)| (pin.last_used, key.clone()))
            .collect();
        oldest.sort_by_key(|(last_used, _)| *last_used);
        let overflow = self.entries.len() - MAX_ENTRIES;
        for (_, key) in oldest.into_iter().take(overflow) {
            self.entries.remove(&key);
        }
    }
}

/// 用 Claude 原始请求的字段生成会话键。
///
/// 必须在转换成 Kiro 请求之前调用。转换后的 `conversationId`
/// 只有 `metadata.user_id` 里带 session UUID 时才稳定，否则每次都是新的。
pub fn affinity_key_for_anthropic(
    user_id: Option<&str>,
    system: Option<&str>,
    first_user: Option<&serde_json::Value>,
) -> Option<String> {
    let mut body = serde_json::Map::new();
    if let Some(user_id) = user_id {
        body.insert(
            "metadata".to_string(),
            serde_json::json!({ "user_id": user_id }),
        );
    }
    if let Some(system) = system {
        body.insert(
            "system".to_string(),
            serde_json::Value::String(system.to_string()),
        );
    }
    if let Some(content) = first_user {
        body.insert(
            "messages".to_string(),
            serde_json::json!([{ "role": "user", "content": content }]),
        );
    }
    extract_affinity_key(&serde_json::Value::Object(body).to_string())
}

/// 从 Anthropic / Responses 请求体提取稳定的会话键。
pub fn extract_affinity_key(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(explicit) = explicit_key(&value) {
        return Some(explicit);
    }
    fingerprint(&value)
}

fn explicit_key(value: &serde_json::Value) -> Option<String> {
    let candidates = [
        value.pointer("/metadata/user_id"),
        value.pointer("/metadata/user"),
        value.pointer("/prompt_cache_key"),
        value.pointer("/promptCacheKey"),
    ];
    for candidate in candidates {
        let Some(text) = candidate.and_then(|item| item.as_str()) else {
            continue;
        };
        let text = text.trim();
        if text.len() >= MIN_EXPLICIT_KEY_LEN {
            return Some(format!("id:{text}"));
        }
    }
    None
}

fn fingerprint(value: &serde_json::Value) -> Option<String> {
    let system = cap_text(&flatten_text(value.get("system")));
    let first_user = value
        .get("messages")
        .and_then(|messages| messages.as_array())
        .and_then(|messages| {
            messages
                .iter()
                .find(|message| message.get("role").and_then(|role| role.as_str()) == Some("user"))
        })
        .map(|message| cap_text(&flatten_text(message.get("content"))))
        .unwrap_or_default();
    if system.is_empty() && first_user.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(system.as_bytes());
    hasher.update(b"\n");
    hasher.update(first_user.as_bytes());
    Some(format!("fp:{}", hex::encode(hasher.finalize())))
}

fn flatten_text(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Object(block) => {
                    let kind = block
                        .get("type")
                        .and_then(|kind| kind.as_str())
                        .unwrap_or("text");
                    if kind == "text" || kind == "input_text" {
                        block
                            .get("text")
                            .and_then(|text| text.as_str())
                            .map(str::to_string)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn cap_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= FINGERPRINT_TEXT_LIMIT {
        return trimmed.to_string();
    }
    let mut end = FINGERPRINT_TEXT_LIMIT;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_session_stays_on_the_first_account() {
        let mut table = AffinityTable::default();
        let now = Instant::now();
        let available = vec![(1, 5, 0), (2, 0, 0)];
        let first = table.resolve("session-a", &available, now).unwrap();
        let available = vec![(1, 100, 0), (2, 0, 0)];
        let second = table.resolve("session-a", &available, now).unwrap();
        assert_eq!(first, 2);
        assert_eq!(second, 2);
    }

    #[test]
    fn new_session_picks_the_least_used_account() {
        let mut table = AffinityTable::default();
        let now = Instant::now();
        let first = table
            .resolve("session-a", &[(1, 0, 0), (2, 0, 0)], now)
            .unwrap();
        let second = table
            .resolve("session-b", &[(1, 1, 0), (2, 0, 0)], now)
            .unwrap();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn new_sessions_spread_before_success_counts_change() {
        let mut table = AffinityTable::default();
        let now = Instant::now();
        let available = [(1, 0, 0), (2, 0, 0), (3, 0, 0)];
        let picked = [
            table.resolve("session-a", &available, now).unwrap(),
            table.resolve("session-b", &available, now).unwrap(),
            table.resolve("session-c", &available, now).unwrap(),
        ];
        assert_eq!(picked, [1, 2, 3]);
    }

    #[test]
    fn unavailable_pin_moves_to_another_account() {
        let mut table = AffinityTable::default();
        let now = Instant::now();
        table.resolve("session-a", &[(1, 0, 0), (2, 0, 0)], now);
        let moved = table.resolve("session-a", &[(2, 4, 0)], now).unwrap();
        assert_eq!(moved, 2);
        let stayed = table
            .resolve("session-a", &[(1, 0, 0), (2, 9, 0)], now)
            .unwrap();
        assert_eq!(stayed, 2);
    }

    #[test]
    fn expired_pin_can_move() {
        let mut table = AffinityTable::default();
        let now = Instant::now();
        table.resolve("session-a", &[(1, 0, 0), (2, 1, 0)], now);
        let later = now + DEFAULT_TTL + Duration::from_secs(1);
        let moved = table
            .resolve("session-a", &[(1, 50, 0), (2, 0, 0)], later)
            .unwrap();
        assert_eq!(moved, 2);
    }

    #[test]
    fn claude_user_id_is_stable_across_turns() {
        let first = extract_affinity_key(
            r#"{"metadata":{"user_id":"user_abc_session_123456"},"messages":[{"role":"user","content":"one"}]}"#,
        );
        let second = extract_affinity_key(
            r#"{"metadata":{"user_id":"user_abc_session_123456"},"messages":[{"role":"user","content":"one"},{"role":"assistant","content":"ok"},{"role":"user","content":"two"}]}"#,
        );
        assert_eq!(first, Some("id:user_abc_session_123456".to_string()));
        assert_eq!(first, second);
    }

    #[test]
    fn prompt_cache_key_and_fingerprint_cover_the_other_clients() {
        let codex = extract_affinity_key(r#"{"prompt_cache_key":"cache-key-001","input":"hi"}"#);
        assert_eq!(codex, Some("id:cache-key-001".to_string()));

        let short_id = extract_affinity_key(
            r#"{"metadata":{"user_id":"abc"},"messages":[{"role":"user","content":"hello"}]}"#,
        );
        assert!(short_id.unwrap().starts_with("fp:"));

        let first = extract_affinity_key(
            r#"{"system":"you are helpful","messages":[{"role":"user","content":"fix the bug"}]}"#,
        )
        .unwrap();
        let later = extract_affinity_key(
            r#"{"system":"you are helpful","messages":[{"role":"user","content":[{"type":"text","text":"fix the bug"}]},{"role":"assistant","content":"done"},{"role":"user","content":"thanks"}]}"#,
        )
        .unwrap();
        assert_eq!(first, later);
        assert!(first.starts_with("fp:"));
    }

    #[test]
    fn anthropic_parts_keep_a_plain_user_id() {
        let content = serde_json::json!("fix the login bug");
        let first =
            affinity_key_for_anthropic(Some("plain-user-session-001"), None, Some(&content));
        let second = affinity_key_for_anthropic(
            Some("plain-user-session-001"),
            Some("system changed"),
            Some(&serde_json::json!("another turn")),
        );
        assert_eq!(first, second);
        assert_eq!(first.as_deref(), Some("id:plain-user-session-001"));
    }
}
