//! EventReducer：Kiro 事件 → Anthropic SSE 事件 的状态机
//!
//! 本模块持有 `SseEvent` 与 `SseStateManager`，负责保证 SSE 序列符合 Anthropic 规范：
//! 1. `message_start` 只发一次
//! 2. `content_block_*` 必须 start → delta → stop 的顺序
//! 3. `message_delta` 必须在所有 `content_block_stop` 之后
//! 4. `message_stop` 在最后
//!
//! 与之配合的"业务侧"代码（`StreamContext` / `BufferedStreamContext`）位于
//! [`super::delivery`]，本模块不感知 thinking / tool_use 等业务状态，只做最低层的事件守门。

use std::collections::HashMap;

use serde_json::json;

/// SSE 事件
#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event: String,
    pub data: serde_json::Value,
}

impl SseEvent {
    pub fn new(event: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            event: event.into(),
            data,
        }
    }

    /// 格式化为 SSE 字符串
    pub fn to_sse_string(&self) -> String {
        format!(
            "event: {}\ndata: {}\n\n",
            self.event,
            serde_json::to_string(&self.data).unwrap_or_default()
        )
    }
}

/// 内容块状态
#[derive(Debug, Clone)]
struct BlockState {
    block_type: String,
    started: bool,
    stopped: bool,
}

impl BlockState {
    fn new(block_type: impl Into<String>) -> Self {
        Self {
            block_type: block_type.into(),
            started: false,
            stopped: false,
        }
    }
}

/// SSE 状态管理器
///
/// 确保 SSE 事件序列符合 Claude API 规范：
/// 1. message_start 只能出现一次
/// 2. content_block 必须先 start 再 delta 再 stop
/// 3. message_delta 只能出现一次，且在所有 content_block_stop 之后
/// 4. message_stop 在最后
#[derive(Debug)]
pub struct SseStateManager {
    /// message_start 是否已发送
    message_started: bool,
    /// message_delta 是否已发送
    message_delta_sent: bool,
    /// 活跃的内容块状态
    active_blocks: HashMap<i32, BlockState>,
    /// 消息是否已结束
    message_ended: bool,
    /// 下一个块索引
    next_block_index: i32,
    /// 当前 stop_reason
    stop_reason: Option<String>,
    /// 是否有工具调用
    has_tool_use: bool,
}

impl Default for SseStateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SseStateManager {
    pub fn new() -> Self {
        Self {
            message_started: false,
            message_delta_sent: false,
            active_blocks: HashMap::new(),
            message_ended: false,
            next_block_index: 0,
            stop_reason: None,
            has_tool_use: false,
        }
    }

    /// 判断指定块是否处于可接收 delta 的打开状态
    pub(super) fn is_block_open_of_type(&self, index: i32, expected_type: &str) -> bool {
        self.active_blocks
            .get(&index)
            .is_some_and(|b| b.started && !b.stopped && b.block_type == expected_type)
    }

    /// 获取下一个块索引
    pub fn next_block_index(&mut self) -> i32 {
        let index = self.next_block_index;
        self.next_block_index += 1;
        index
    }

    /// 记录工具调用
    pub fn set_has_tool_use(&mut self, has: bool) {
        self.has_tool_use = has;
    }

    /// 设置 stop_reason
    pub fn set_stop_reason(&mut self, reason: impl Into<String>) {
        self.stop_reason = Some(reason.into());
    }

    /// 检查是否存在非 thinking 类型的内容块（如 text 或 tool_use）
    pub(super) fn has_non_thinking_blocks(&self) -> bool {
        self.active_blocks
            .values()
            .any(|b| b.block_type != "thinking")
    }

    /// 获取最终的 stop_reason
    pub fn get_stop_reason(&self) -> String {
        if let Some(ref reason) = self.stop_reason {
            reason.clone()
        } else if self.has_tool_use {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        }
    }

    /// 处理 message_start 事件
    pub fn handle_message_start(&mut self, event: serde_json::Value) -> Option<SseEvent> {
        if self.message_started {
            tracing::debug!("跳过重复的 message_start 事件");
            return None;
        }
        self.message_started = true;
        Some(SseEvent::new("message_start", event))
    }

    /// 处理 content_block_start 事件
    pub fn handle_content_block_start(
        &mut self,
        index: i32,
        block_type: &str,
        data: serde_json::Value,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 如果是 tool_use 块，先关闭之前的文本块
        if block_type == "tool_use" {
            self.has_tool_use = true;
            for (block_index, block) in self.active_blocks.iter_mut() {
                if block.block_type == "text" && block.started && !block.stopped {
                    events.push(SseEvent::new(
                        "content_block_stop",
                        json!({
                            "type": "content_block_stop",
                            "index": block_index
                        }),
                    ));
                    block.stopped = true;
                }
            }
        }

        if let Some(block) = self.active_blocks.get_mut(&index) {
            if block.started {
                tracing::debug!("块 {} 已启动，跳过重复的 content_block_start", index);
                return events;
            }
            block.started = true;
        } else {
            let mut block = BlockState::new(block_type);
            block.started = true;
            self.active_blocks.insert(index, block);
        }

        events.push(SseEvent::new("content_block_start", data));
        events
    }

    /// 处理 content_block_delta 事件
    pub fn handle_content_block_delta(
        &mut self,
        index: i32,
        data: serde_json::Value,
    ) -> Option<SseEvent> {
        if let Some(block) = self.active_blocks.get(&index) {
            if !block.started || block.stopped {
                tracing::warn!(
                    "块 {} 状态异常: started={}, stopped={}",
                    index,
                    block.started,
                    block.stopped
                );
                return None;
            }
        } else {
            tracing::warn!("收到未知块 {} 的 delta 事件", index);
            return None;
        }

        Some(SseEvent::new("content_block_delta", data))
    }

    /// 处理 content_block_stop 事件
    pub fn handle_content_block_stop(&mut self, index: i32) -> Option<SseEvent> {
        if let Some(block) = self.active_blocks.get_mut(&index) {
            if block.stopped {
                tracing::debug!("块 {} 已停止，跳过重复的 content_block_stop", index);
                return None;
            }
            block.stopped = true;
            return Some(SseEvent::new(
                "content_block_stop",
                json!({
                    "type": "content_block_stop",
                    "index": index
                }),
            ));
        }
        None
    }

    /// 生成最终事件序列
    pub fn generate_final_events(
        &mut self,
        input_tokens: i32,
        output_tokens: i32,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 关闭所有未关闭的块
        for (index, block) in self.active_blocks.iter_mut() {
            if block.started && !block.stopped {
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({
                        "type": "content_block_stop",
                        "index": index
                    }),
                ));
                block.stopped = true;
            }
        }

        if !self.message_delta_sent {
            self.message_delta_sent = true;
            events.push(SseEvent::new(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": self.get_stop_reason(),
                        "stop_sequence": null
                    },
                    "usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens
                    }
                }),
            ));
        }

        if !self.message_ended {
            self.message_ended = true;
            events.push(SseEvent::new(
                "message_stop",
                json!({ "type": "message_stop" }),
            ));
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_event_format_includes_event_data_and_terminator() {
        let event = SseEvent::new("message_start", json!({"type": "message_start"}));
        let sse_str = event.to_sse_string();

        assert!(sse_str.starts_with("event: message_start\n"));
        assert!(sse_str.contains("data: "));
        assert!(sse_str.ends_with("\n\n"));
    }

    #[test]
    fn message_start_only_emits_once() {
        let mut manager = SseStateManager::new();
        let first = manager.handle_message_start(json!({"type": "message_start"}));
        assert!(first.is_some(), "first message_start should be emitted");
        let second = manager.handle_message_start(json!({"type": "message_start"}));
        assert!(second.is_none(), "duplicate message_start should be skipped");
    }

    #[test]
    fn content_block_lifecycle_start_delta_stop_idempotent() {
        let mut manager = SseStateManager::new();

        let events = manager.handle_content_block_start(0, "text", json!({}));
        assert_eq!(events.len(), 1);

        let event = manager.handle_content_block_delta(0, json!({}));
        assert!(event.is_some());

        let event = manager.handle_content_block_stop(0);
        assert!(event.is_some());

        // 重复 stop 应该被跳过
        let event = manager.handle_content_block_stop(0);
        assert!(event.is_none(), "duplicate stop should be skipped");
    }

    #[test]
    fn content_block_stop_idempotent() {
        let mut manager = SseStateManager::new();
        manager.handle_content_block_start(7, "text", json!({}));
        let first = manager.handle_content_block_stop(7);
        assert!(first.is_some(), "first stop should emit event");
        let second = manager.handle_content_block_stop(7);
        assert!(second.is_none(), "second stop should be no-op");
    }

    #[test]
    fn generate_final_events_closes_open_blocks_in_order() {
        let mut manager = SseStateManager::new();
        manager.handle_message_start(json!({"type": "message_start"}));
        manager.handle_content_block_start(0, "text", json!({}));
        manager.handle_content_block_start(1, "tool_use", json!({}));

        let events = manager.generate_final_events(10, 5);

        let kinds: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
        let last_stop = kinds.iter().rposition(|k| *k == "content_block_stop");
        let delta_pos = kinds.iter().position(|k| *k == "message_delta");
        let stop_pos = kinds.iter().position(|k| *k == "message_stop");

        assert!(last_stop.is_some(), "应至少关闭一个 content_block");
        assert!(delta_pos.is_some(), "应包含 message_delta");
        assert!(stop_pos.is_some(), "应包含 message_stop");
        assert!(
            last_stop.unwrap() < delta_pos.unwrap(),
            "content_block_stop 必须在 message_delta 之前"
        );
        assert!(
            delta_pos.unwrap() < stop_pos.unwrap(),
            "message_delta 必须在 message_stop 之前"
        );
    }

    #[test]
    fn message_delta_only_emits_once_after_finalize() {
        let mut manager = SseStateManager::new();
        let first_finalize = manager.generate_final_events(0, 0);
        let second_finalize = manager.generate_final_events(0, 0);

        let first_delta_count = first_finalize
            .iter()
            .filter(|e| e.event == "message_delta")
            .count();
        let second_delta_count = second_finalize
            .iter()
            .filter(|e| e.event == "message_delta")
            .count();

        assert_eq!(first_delta_count, 1);
        assert_eq!(second_delta_count, 0, "message_delta 不应重复发送");
    }
}
