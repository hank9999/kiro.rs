//! 历史消息管理器 - 处理对话长度限制
//!
//! 提供多种策略处理 Kiro API 的输入长度限制：
//! 1. 自动截断 - 保留最近 N 条消息
//! 2. 智能摘要 - 压缩早期消息（调用 LLM 生成摘要）
//! 3. 错误重试 - 捕获错误后截断重试
//! 4. 预估检测 - 发送前预估并截断
//!
//! 移植自 KiroProxy 的 HistoryManager

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::kiro::model::requests::conversation::{
    ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, Message, UserInputMessage, UserInputMessageContext, UserMessage,
};
use crate::kiro::model::requests::kiro::KiroRequest;

/// 截断策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TruncateStrategy {
    /// 不截断
    None,
    /// 自动截断（保留最近 N 条）
    AutoTruncate,
    /// 智能摘要
    SmartSummary,
    /// 错误时截断重试
    ErrorRetry,
    /// 预估检测
    PreEstimate,
}

impl Default for TruncateStrategy {
    fn default() -> Self {
        Self::ErrorRetry
    }
}

/// 历史消息配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// 启用的策略列表
    pub strategies: Vec<TruncateStrategy>,

    /// 自动截断配置：最大消息数
    pub max_messages: usize,

    /// 自动截断配置：最大字符数（约 50k tokens）
    pub max_chars: usize,

    /// 智能摘要配置：摘要时保留最近 N 条完整消息
    pub summary_keep_recent: usize,

    /// 智能摘要配置：触发摘要的字符数阈值
    pub summary_threshold: usize,

    /// 智能摘要配置：摘要最大长度
    pub summary_max_length: usize,

    /// 错误重试配置：重试时保留的消息数
    pub retry_max_messages: usize,

    /// 错误重试配置：最大重试次数
    pub max_retries: usize,

    /// 预估配置：预估阈值（字符数）
    pub estimate_threshold: usize,

    /// 每 token 约等于多少字符
    pub chars_per_token: f64,

    /// 摘要缓存：是否启用
    pub summary_cache_enabled: bool,

    /// 摘要缓存：最大复用时间（秒）
    pub summary_cache_max_age_seconds: u64,

    /// 摘要缓存：旧历史新增 N 条后刷新摘要
    pub summary_cache_min_delta_messages: usize,

    /// 是否添加截断警告
    pub add_warning_header: bool,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            strategies: vec![TruncateStrategy::ErrorRetry, TruncateStrategy::SmartSummary],
            max_messages: 30,
            max_chars: 150_000,
            summary_keep_recent: 10,
            summary_threshold: 100_000,
            summary_max_length: 2000,
            retry_max_messages: 20,
            max_retries: 2,
            estimate_threshold: 180_000,
            chars_per_token: 3.0,
            summary_cache_enabled: true,
            summary_cache_max_age_seconds: 180,
            summary_cache_min_delta_messages: 3,
            add_warning_header: true,
        }
    }
}

impl HistoryConfig {
    /// 检查是否启用了指定策略
    pub fn has_strategy(&self, strategy: TruncateStrategy) -> bool {
        self.strategies.contains(&strategy)
    }
}

/// 摘要缓存条目
#[derive(Debug, Clone)]
struct SummaryCacheEntry {
    summary: String,
    old_history_count: usize,
    created_at: Instant,
}

/// 摘要缓存（按会话）
pub struct SummaryCache {
    entries: RwLock<HashMap<String, SummaryCacheEntry>>,
    max_entries: usize,
}

impl SummaryCache {
    /// 创建新的摘要缓存
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            max_entries,
        }
    }

    /// 获取缓存的摘要
    pub async fn get(
        &self,
        key: &str,
        old_history_count: usize,
        max_age: Duration,
        min_delta_messages: usize,
    ) -> Option<String> {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key) {
            // 检查是否过期
            if entry.created_at.elapsed() > max_age {
                return None;
            }
            // 检查历史是否变化太大
            if old_history_count.saturating_sub(entry.old_history_count) >= min_delta_messages {
                return None;
            }
            return Some(entry.summary.clone());
        }
        None
    }

    /// 设置缓存的摘要
    pub async fn set(&self, key: String, summary: String, old_history_count: usize) {
        let mut entries = self.entries.write().await;
        
        // 如果超过最大条目数，删除最旧的
        if entries.len() >= self.max_entries && !entries.contains_key(&key) {
            // 找到最旧的条目
            if let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, v)| v.created_at)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest_key);
            }
        }

        entries.insert(
            key,
            SummaryCacheEntry {
                summary,
                old_history_count,
                created_at: Instant::now(),
            },
        );
    }
}

/// 全局摘要缓存
static SUMMARY_CACHE: std::sync::OnceLock<Arc<SummaryCache>> = std::sync::OnceLock::new();

/// 获取全局摘要缓存
pub fn get_summary_cache() -> Arc<SummaryCache> {
    SUMMARY_CACHE
        .get_or_init(|| Arc::new(SummaryCache::new(128)))
        .clone()
}

/// 截断信息
#[derive(Debug, Clone, Default)]
pub struct TruncateInfo {
    /// 是否发生了截断
    pub truncated: bool,
    /// 截断描述
    pub message: String,
    /// 原始消息数
    pub original_count: usize,
    /// 截断后消息数
    pub final_count: usize,
    /// 是否使用了摘要
    pub used_summary: bool,
}

impl TruncateInfo {
    fn new_truncated(message: String, original_count: usize, final_count: usize) -> Self {
        Self {
            truncated: true,
            message,
            original_count,
            final_count,
            used_summary: false,
        }
    }

    fn new_with_summary(message: String, original_count: usize, final_count: usize) -> Self {
        Self {
            truncated: true,
            message,
            original_count,
            final_count,
            used_summary: true,
        }
    }
}

/// 摘要生成器 trait
#[async_trait::async_trait]
pub trait SummaryGenerator: Send + Sync {
    /// 生成摘要
    async fn generate(&self, prompt: &str) -> anyhow::Result<String>;
}

/// 历史消息管理器
pub struct HistoryManager {
    config: HistoryConfig,
    truncate_info: TruncateInfo,
    cache_key: Option<String>,
}


impl HistoryManager {
    /// 创建新的历史消息管理器
    pub fn new(config: HistoryConfig) -> Self {
        Self {
            config,
            truncate_info: TruncateInfo::default(),
            cache_key: None,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(HistoryConfig::default())
    }

    /// 设置缓存 key（通常是 session_id）
    pub fn with_cache_key(mut self, key: String) -> Self {
        self.cache_key = Some(key);
        self
    }

    /// 获取截断信息
    pub fn truncate_info(&self) -> &TruncateInfo {
        &self.truncate_info
    }

    /// 是否发生了截断
    pub fn was_truncated(&self) -> bool {
        self.truncate_info.truncated
    }

    /// 重置状态
    pub fn reset(&mut self) {
        self.truncate_info = TruncateInfo::default();
    }

    /// 估算历史消息的字符数
    pub fn estimate_history_chars(history: &[Message]) -> usize {
        history
            .iter()
            .map(|msg| Self::estimate_message_chars(msg))
            .sum()
    }

    /// 估算单条消息的字符数
    fn estimate_message_chars(msg: &Message) -> usize {
        match msg {
            Message::User(user_msg) => {
                let content_len = user_msg.user_input_message.content.len();
                let context_len =
                    Self::estimate_user_message_context_chars(&user_msg.user_input_message);
                content_len + context_len
            }
            Message::Assistant(assistant_msg) => {
                let content_len = assistant_msg.assistant_response_message.content.len();
                let tool_uses_len = assistant_msg
                    .assistant_response_message
                    .tool_uses
                    .as_ref()
                    .map(|tools| {
                        tools
                            .iter()
                            .map(|t| t.name.len() + t.tool_use_id.len() + t.input.to_string().len())
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                content_len + tool_uses_len
            }
        }
    }

    /// 估算用户消息上下文的字符数
    fn estimate_user_message_context_chars(user_msg: &UserMessage) -> usize {
        let tool_results_len: usize = user_msg
            .user_input_message_context
            .tool_results
            .iter()
            .map(|r| r.tool_use_id.len() + r.content.len())
            .sum();

        let tools_len: usize = user_msg
            .user_input_message_context
            .tools
            .iter()
            .map(|t| {
                t.tool_specification.name.len()
                    + t.tool_specification.description.len()
                    + 200 // 估算 input_schema 的大小
            })
            .sum();

        tool_results_len + tools_len
    }

    /// 估算请求总字符数
    pub fn estimate_request_chars(
        &self,
        history: &[Message],
        user_content: &str,
    ) -> (usize, usize, usize) {
        let history_chars = Self::estimate_history_chars(history);
        let user_chars = user_content.len();
        (history_chars, user_chars, history_chars + user_chars)
    }

    /// 检查是否需要预截断
    pub fn should_pre_truncate(&self, history: &[Message], user_content: &str) -> bool {
        if !self.config.has_strategy(TruncateStrategy::PreEstimate) {
            return false;
        }

        let (_, _, total_chars) = self.estimate_request_chars(history, user_content);
        total_chars > self.config.estimate_threshold
    }

    /// 检查是否需要智能摘要
    pub fn should_summarize(&self, history: &[Message]) -> bool {
        if !self.config.has_strategy(TruncateStrategy::SmartSummary) {
            return false;
        }

        let total_chars = Self::estimate_history_chars(history);
        total_chars > self.config.summary_threshold
            && history.len() > self.config.summary_keep_recent
    }

    /// 按消息数量截断
    pub fn truncate_by_count(&mut self, history: Vec<Message>, max_count: usize) -> Vec<Message> {
        if history.len() <= max_count {
            return history;
        }

        let original_count = history.len();

        // 从后往前保留
        let truncated: Vec<Message> = history
            .into_iter()
            .skip(original_count - max_count)
            .collect();

        self.truncate_info = TruncateInfo::new_truncated(
            format!(
                "按数量截断: {} -> {} 条消息",
                original_count,
                truncated.len()
            ),
            original_count,
            truncated.len(),
        );

        truncated
    }

    /// 按字符数截断
    pub fn truncate_by_chars(&mut self, history: Vec<Message>, max_chars: usize) -> Vec<Message> {
        let original_chars = Self::estimate_history_chars(&history);
        if original_chars <= max_chars {
            return history;
        }

        let original_count = history.len();

        // 从后往前保留
        let mut result = Vec::new();
        let mut current_chars = 0;

        for msg in history.into_iter().rev() {
            let msg_chars = Self::estimate_message_chars(&msg);
            if current_chars + msg_chars > max_chars && !result.is_empty() {
                break;
            }
            current_chars += msg_chars;
            result.push(msg);
        }

        // 反转回正确顺序
        result.reverse();

        self.truncate_info = TruncateInfo::new_truncated(
            format!(
                "按字符数截断: {} -> {} 条消息 ({} -> {} 字符)",
                original_count,
                result.len(),
                original_chars,
                current_chars
            ),
            original_count,
            result.len(),
        );

        result
    }

    /// 格式化历史消息用于生成摘要
    fn format_history_for_summary(history: &[Message]) -> String {
        let mut lines = Vec::new();

        for msg in history {
            let (role, content) = match msg {
                Message::User(user_msg) => {
                    let mut content = user_msg.user_input_message.content.clone();
                    // 截断过长的单条消息
                    if content.len() > 500 {
                        content = format!("{}...", &content[..500]);
                    }
                    ("user", content)
                }
                Message::Assistant(assistant_msg) => {
                    let mut content = assistant_msg.assistant_response_message.content.clone();
                    if content.len() > 500 {
                        content = format!("{}...", &content[..500]);
                    }
                    ("assistant", content)
                }
            };
            lines.push(format!("[{}]: {}", role, content));
        }

        lines.join("\n")
    }

    /// 生成摘要提示词
    fn build_summary_prompt(history: &[Message], max_length: usize) -> String {
        let formatted = Self::format_history_for_summary(history);
        // 限制输入长度
        let formatted = if formatted.len() > 10000 {
            format!("{}...(truncated)", &formatted[..10000])
        } else {
            formatted
        };

        format!(
            r#"请简洁地总结以下对话历史的关键信息，包括：
1. 用户的主要目标和需求
2. 已完成的重要操作
3. 当前的工作状态和上下文

对话历史：
{}

请用中文输出摘要，控制在 {} 字符以内："#,
            formatted, max_length
        )
    }

    /// 构建带摘要的历史消息
    ///
    /// 格式：
    /// 1. 摘要消息（user）
    /// 2. 占位响应（assistant）
    /// 3. 最近的完整消息
    pub fn build_summary_history(
        summary: &str,
        recent_history: Vec<Message>,
        model_id: &str,
    ) -> Vec<Message> {
        let mut result = Vec::new();

        // 确保 recent_history 以 user 消息开头
        let mut recent = recent_history;
        while !recent.is_empty() && matches!(recent.first(), Some(Message::Assistant(_))) {
            recent.remove(0);
        }

        // 如果第一条 user 消息有 toolResults，需要清除它
        // 因为摘要后的 assistant 占位消息没有 toolUses
        if let Some(Message::User(user_msg)) = recent.first_mut() {
            user_msg
                .user_input_message
                .user_input_message_context
                .tool_results
                .clear();
        }

        // 摘要消息
        let summary_content = format!(
            "[Earlier conversation summary]\n{}\n\n[Continuing from recent messages...]",
            summary
        );
        let summary_msg = HistoryUserMessage::new(summary_content, model_id);
        result.push(Message::User(summary_msg));

        // 占位 assistant 消息（没有 toolUses）
        let placeholder = HistoryAssistantMessage::new("I understand the context. Let's continue.");
        result.push(Message::Assistant(placeholder));

        // 最近的完整消息
        result.extend(recent);

        // 修复 tool_use/tool_result 配对
        Self::fix_tool_pairing(&mut result);

        result
    }


    /// 使用智能摘要压缩历史消息
    ///
    /// 返回 (压缩后的历史, 是否成功)
    pub async fn compress_with_summary<G: SummaryGenerator>(
        &mut self,
        history: Vec<Message>,
        model_id: &str,
        generator: &G,
    ) -> (Vec<Message>, bool) {
        let total_chars = Self::estimate_history_chars(&history);
        if total_chars <= self.config.summary_threshold {
            return (history, false);
        }

        if history.len() <= self.config.summary_keep_recent {
            return (history, false);
        }

        let original_count = history.len();

        // 分离早期消息和最近消息
        let keep_recent = self.config.summary_keep_recent;
        let split_point = history.len() - keep_recent;
        let (old_history, recent_history): (Vec<_>, Vec<_>) = history
            .into_iter()
            .enumerate()
            .partition(|(i, _)| *i < split_point);

        let old_history: Vec<Message> = old_history.into_iter().map(|(_, m)| m).collect();
        let recent_history: Vec<Message> = recent_history.into_iter().map(|(_, m)| m).collect();

        // 尝试从缓存获取摘要
        let cache_key = self.cache_key.as_ref().map(|k| format!("{}:{}", k, keep_recent));
        let old_count = old_history.len();

        if let Some(ref key) = cache_key {
            if self.config.summary_cache_enabled {
                let cache = get_summary_cache();
                if let Some(cached_summary) = cache
                    .get(
                        key,
                        old_count,
                        Duration::from_secs(self.config.summary_cache_max_age_seconds),
                        self.config.summary_cache_min_delta_messages,
                    )
                    .await
                {
                    tracing::info!("使用缓存的摘要: key={}", key);
                    let result =
                        Self::build_summary_history(&cached_summary, recent_history, model_id);
                    self.truncate_info = TruncateInfo::new_with_summary(
                        format!(
                            "智能摘要(缓存): {} -> {} 条消息",
                            original_count,
                            result.len()
                        ),
                        original_count,
                        result.len(),
                    );
                    return (result, true);
                }
            }
        }

        // 生成摘要
        let prompt = Self::build_summary_prompt(&old_history, self.config.summary_max_length);
        match generator.generate(&prompt).await {
            Ok(mut summary) => {
                // 限制摘要长度
                if summary.len() > self.config.summary_max_length {
                    summary = format!("{}...", &summary[..self.config.summary_max_length]);
                }

                // 缓存摘要
                if let Some(ref key) = cache_key {
                    if self.config.summary_cache_enabled {
                        let cache = get_summary_cache();
                        cache.set(key.clone(), summary.clone(), old_count).await;
                        tracing::info!("缓存摘要: key={}, len={}", key, summary.len());
                    }
                }

                let result = Self::build_summary_history(&summary, recent_history, model_id);
                self.truncate_info = TruncateInfo::new_with_summary(
                    format!(
                        "智能摘要: {} -> {} 条消息 (摘要 {} 字符)",
                        original_count,
                        result.len(),
                        summary.len()
                    ),
                    original_count,
                    result.len(),
                );
                (result, true)
            }
            Err(e) => {
                tracing::warn!("生成摘要失败: {}", e);
                // 摘要失败，回退到简单截断
                let mut result = recent_history;
                Self::fix_tool_pairing(&mut result);
                self.truncate_info = TruncateInfo::new_truncated(
                    format!(
                        "摘要生成失败，回退截断: {} -> {} 条消息",
                        original_count,
                        result.len()
                    ),
                    original_count,
                    result.len(),
                );
                (result, false)
            }
        }
    }

    /// 处理长度超限错误（带摘要支持）
    ///
    /// 返回 (截断后的历史, 是否应该重试)
    pub async fn handle_length_error_with_summary<G: SummaryGenerator>(
        &mut self,
        history: Vec<Message>,
        model_id: &str,
        retry_count: usize,
        generator: Option<&G>,
    ) -> (Vec<Message>, bool) {
        // 检查是否启用了错误重试策略
        if !self.config.has_strategy(TruncateStrategy::ErrorRetry) {
            return (history, false);
        }

        // 检查是否超过最大重试次数
        if retry_count >= self.config.max_retries {
            return (history, false);
        }

        if history.is_empty() {
            return (history, false);
        }

        self.reset();

        // 根据重试次数逐步减少消息
        let factor = 1.0 - (retry_count as f64 * 0.3);
        let target_count = (self.config.retry_max_messages as f64 * factor).max(5.0) as usize;

        if history.len() <= target_count {
            return (history, false);
        }

        let original_count = history.len();

        // 分离早期消息和最近消息
        let split_point = history.len() - target_count;
        let (old_history, recent_history): (Vec<_>, Vec<_>) = history
            .into_iter()
            .enumerate()
            .partition(|(i, _)| *i < split_point);

        let old_history: Vec<Message> = old_history.into_iter().map(|(_, m)| m).collect();
        let recent_history: Vec<Message> = recent_history.into_iter().map(|(_, m)| m).collect();

        // 尝试生成摘要
        if let Some(summary_gen) = generator {
            // 尝试从缓存获取
            let cache_key = self
                .cache_key
                .as_ref()
                .map(|k| format!("{}:retry:{}", k, target_count));
            let old_count = old_history.len();

            if let Some(ref key) = cache_key {
                if self.config.summary_cache_enabled {
                    let cache = get_summary_cache();
                    if let Some(cached_summary) = cache
                        .get(
                            key,
                            old_count,
                            Duration::from_secs(self.config.summary_cache_max_age_seconds),
                            self.config.summary_cache_min_delta_messages,
                        )
                        .await
                    {
                        tracing::info!("错误重试使用缓存摘要: key={}", key);
                        let result =
                            Self::build_summary_history(&cached_summary, recent_history, model_id);
                        self.truncate_info = TruncateInfo::new_with_summary(
                            format!(
                                "错误重试摘要(缓存) (第 {} 次): {} -> {} 条消息",
                                retry_count + 1,
                                original_count,
                                result.len()
                            ),
                            original_count,
                            result.len(),
                        );
                        return (result, true);
                    }
                }
            }

            // 生成新摘要
            let prompt = Self::build_summary_prompt(&old_history, self.config.summary_max_length);
            match summary_gen.generate(&prompt).await {
                Ok(mut summary) => {
                    if summary.len() > self.config.summary_max_length {
                        summary = format!("{}...", &summary[..self.config.summary_max_length]);
                    }

                    // 缓存摘要
                    if let Some(ref key) = cache_key {
                        if self.config.summary_cache_enabled {
                            let cache = get_summary_cache();
                            cache.set(key.clone(), summary.clone(), old_count).await;
                        }
                    }

                    let result =
                        Self::build_summary_history(&summary, recent_history, model_id);
                    self.truncate_info = TruncateInfo::new_with_summary(
                        format!(
                            "错误重试摘要 (第 {} 次): {} -> {} 条消息 (摘要 {} 字符)",
                            retry_count + 1,
                            original_count,
                            result.len(),
                            summary.len()
                        ),
                        original_count,
                        result.len(),
                    );
                    return (result, true);
                }
                Err(e) => {
                    tracing::warn!("错误重试生成摘要失败: {}", e);
                }
            }
        }

        // 摘要失败或无生成器，回退到按数量截断
        self.reset();
        let truncated = self.truncate_by_count(
            old_history.into_iter().chain(recent_history).collect(),
            target_count,
        );

        if self.truncate_info.truncated {
            self.truncate_info.message = format!(
                "错误重试截断 (第 {} 次): {} -> {} 条消息",
                retry_count + 1,
                original_count,
                self.truncate_info.final_count
            );
            (truncated, true)
        } else {
            (truncated, false)
        }
    }

    /// 处理长度超限错误（简单版本，无摘要）
    pub fn handle_length_error(
        &mut self,
        history: Vec<Message>,
        retry_count: usize,
    ) -> (Vec<Message>, bool) {
        if !self.config.has_strategy(TruncateStrategy::ErrorRetry) {
            return (history, false);
        }

        if retry_count >= self.config.max_retries {
            return (history, false);
        }

        if history.is_empty() {
            return (history, false);
        }

        self.reset();

        let factor = 1.0 - (retry_count as f64 * 0.3);
        let target_count = (self.config.retry_max_messages as f64 * factor).max(5.0) as usize;

        if history.len() <= target_count {
            return (history, false);
        }

        let truncated = self.truncate_by_count(history, target_count);

        if self.truncate_info.truncated {
            self.truncate_info.message = format!(
                "错误重试截断 (第 {} 次): {} -> {} 条消息",
                retry_count + 1,
                self.truncate_info.original_count,
                self.truncate_info.final_count
            );
            (truncated, true)
        } else {
            (truncated, false)
        }
    }


    /// 修复历史消息的 tool_use/tool_result 配对
    pub fn fix_tool_pairing(history: &mut [Message]) {
        // 收集所有 tool_use_id
        let mut tool_use_ids: HashSet<String> = HashSet::new();

        for msg in history.iter() {
            if let Message::Assistant(assistant_msg) = msg {
                if let Some(ref tool_uses) = assistant_msg.assistant_response_message.tool_uses {
                    for tool_use in tool_uses {
                        tool_use_ids.insert(tool_use.tool_use_id.clone());
                    }
                }
            }
        }

        // 过滤孤立的 tool_result
        for msg in history.iter_mut() {
            if let Message::User(user_msg) = msg {
                let tool_results = &mut user_msg
                    .user_input_message
                    .user_input_message_context
                    .tool_results;
                tool_results.retain(|r| tool_use_ids.contains(&r.tool_use_id));
            }
        }
    }

    /// 在截断后修复历史消息
    pub fn fix_history_after_truncate(mut history: Vec<Message>) -> Vec<Message> {
        if history.is_empty() {
            return history;
        }

        // 确保历史以 user 消息开头
        while !history.is_empty() && matches!(history.first(), Some(Message::Assistant(_))) {
            history.remove(0);
        }

        // 修复 tool_use/tool_result 配对
        Self::fix_tool_pairing(&mut history);

        history
    }
}

/// Kiro API 摘要生成器
///
/// 使用 Kiro API 调用 claude-sonnet-4.5 生成摘要
pub struct KiroSummaryGenerator {
    provider: Arc<crate::kiro::provider::KiroProvider>,
    profile_arn: String,
}

impl KiroSummaryGenerator {
    /// 创建新的 Kiro 摘要生成器
    pub fn new(
        provider: Arc<crate::kiro::provider::KiroProvider>,
        profile_arn: Option<String>,
    ) -> Self {
        Self {
            provider,
            profile_arn: profile_arn.unwrap_or_default(),
        }
    }
}

#[async_trait::async_trait]
impl SummaryGenerator for KiroSummaryGenerator {
    async fn generate(&self, prompt: &str) -> anyhow::Result<String> {
        // 构建简单的摘要请求
        let user_input = UserInputMessage::new(prompt.to_string(), "claude-sonnet-4.5")
            .with_context(UserInputMessageContext::new())
            .with_origin("AI_EDITOR");

        let current_message = CurrentMessage::new(user_input);

        let conversation_state = ConversationState::new(uuid::Uuid::new_v4().to_string())
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(current_message)
            .with_history(vec![]);

        let kiro_request = KiroRequest {
            conversation_state,
            profile_arn: Some(self.profile_arn.clone()),
        };

        let request_body = serde_json::to_string(&kiro_request)?;

        // 调用 Kiro API（非流式）
        let response = self
            .provider
            .call_api_with_credential_id(&request_body, Some("claude-sonnet-4.5"))
            .await
            .map_err(|e| anyhow::anyhow!("Kiro API 调用失败: {}", e))?;

        let (_, resp) = response;
        let body_bytes = resp.bytes().await?;

        // 解析响应
        let mut decoder = crate::kiro::parser::decoder::EventStreamDecoder::new();
        if let Err(e) = decoder.feed(&body_bytes) {
            tracing::warn!("解码摘要响应失败: {}", e);
        }

        let mut text_content = String::new();

        for result in decoder.decode_iter() {
            if let Ok(frame) = result {
                if let Ok(event) = crate::kiro::model::events::Event::from_frame(frame) {
                    if let crate::kiro::model::events::Event::AssistantResponse(resp) = event {
                        text_content.push_str(&resp.content);
                    }
                }
            }
        }

        if text_content.is_empty() {
            return Err(anyhow::anyhow!("摘要响应为空"));
        }

        Ok(text_content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟摘要生成器（用于测试）
    struct MockSummaryGenerator {
        response: String,
        should_fail: bool,
    }

    impl MockSummaryGenerator {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                should_fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                response: String::new(),
                should_fail: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl SummaryGenerator for MockSummaryGenerator {
        async fn generate(&self, _prompt: &str) -> anyhow::Result<String> {
            if self.should_fail {
                Err(anyhow::anyhow!("模拟生成失败"))
            } else {
                Ok(self.response.clone())
            }
        }
    }

    /// 创建测试用的用户消息
    fn create_test_user_message(content: &str, model_id: &str) -> Message {
        Message::User(HistoryUserMessage::new(content.to_string(), model_id))
    }

    /// 创建测试用的助手消息
    fn create_test_assistant_message(content: &str) -> Message {
        Message::Assistant(HistoryAssistantMessage::new(content))
    }

    /// 创建测试用的历史消息列表
    fn create_test_history(count: usize) -> Vec<Message> {
        let mut history = Vec::new();
        for i in 0..count {
            if i % 2 == 0 {
                history.push(create_test_user_message(
                    &format!("用户消息 {}", i / 2 + 1),
                    "claude-sonnet-4.5",
                ));
            } else {
                history.push(create_test_assistant_message(&format!(
                    "助手回复 {}",
                    i / 2 + 1
                )));
            }
        }
        history
    }

    #[test]
    fn test_truncate_by_count() {
        let mut manager = HistoryManager::with_defaults();
        let history = create_test_history(10);

        let truncated = manager.truncate_by_count(history, 4);

        assert_eq!(truncated.len(), 4);
        assert!(manager.truncate_info.truncated);
        assert_eq!(manager.truncate_info.original_count, 10);
        assert_eq!(manager.truncate_info.final_count, 4);
    }

    #[test]
    fn test_truncate_by_count_no_truncation_needed() {
        let mut manager = HistoryManager::with_defaults();
        let history = create_test_history(4);

        let result = manager.truncate_by_count(history.clone(), 10);

        assert_eq!(result.len(), 4);
        assert!(!manager.truncate_info.truncated);
    }

    #[test]
    fn test_build_summary_history() {
        let summary = "这是一个测试摘要，描述了之前的对话内容。";
        let recent = create_test_history(4);

        let result = HistoryManager::build_summary_history(summary, recent, "claude-sonnet-4.5");

        // 应该有：摘要消息 + 占位响应 + 最近消息
        assert!(result.len() >= 2);

        // 第一条应该是包含摘要的用户消息
        if let Message::User(user_msg) = &result[0] {
            assert!(user_msg
                .user_input_message
                .content
                .contains("[Earlier conversation summary]"));
            assert!(user_msg.user_input_message.content.contains(summary));
        } else {
            panic!("第一条消息应该是用户消息");
        }

        // 第二条应该是占位助手消息
        if let Message::Assistant(assistant_msg) = &result[1] {
            assert!(assistant_msg
                .assistant_response_message
                .content
                .contains("I understand the context"));
        } else {
            panic!("第二条消息应该是助手消息");
        }
    }

    #[test]
    fn test_fix_tool_pairing() {
        // 这个测试验证孤立的 tool_result 会被清理
        let mut history = create_test_history(4);

        // 在用户消息中添加一个孤立的 tool_result
        if let Message::User(user_msg) = &mut history[2] {
            user_msg
                .user_input_message
                .user_input_message_context
                .tool_results
                .push(crate::kiro::model::requests::tool::ToolResult {
                    tool_use_id: "orphan_tool_123".to_string(),
                    content: vec![],
                    status: Some("success".to_string()),
                    is_error: false,
                });
        }

        HistoryManager::fix_tool_pairing(&mut history);

        // 孤立的 tool_result 应该被清理
        if let Message::User(user_msg) = &history[2] {
            assert!(user_msg
                .user_input_message
                .user_input_message_context
                .tool_results
                .is_empty());
        }
    }

    #[test]
    fn test_fix_history_after_truncate_removes_leading_assistant() {
        let mut history = Vec::new();
        // 以助手消息开头（不正常）
        history.push(create_test_assistant_message("助手消息"));
        history.push(create_test_user_message("用户消息", "claude-sonnet-4.5"));
        history.push(create_test_assistant_message("助手回复"));

        let fixed = HistoryManager::fix_history_after_truncate(history);

        // 第一条应该是用户消息
        assert!(matches!(fixed.first(), Some(Message::User(_))));
        assert_eq!(fixed.len(), 2);
    }

    #[test]
    fn test_handle_length_error_simple() {
        let mut manager = HistoryManager::with_defaults();
        let history = create_test_history(30);

        let (truncated, should_retry) = manager.handle_length_error(history, 0);

        assert!(should_retry);
        assert!(truncated.len() <= 20); // retry_max_messages 默认是 20
        assert!(manager.truncate_info.truncated);
    }

    #[test]
    fn test_handle_length_error_max_retries() {
        let mut manager = HistoryManager::with_defaults();
        let history = create_test_history(30);

        // 超过最大重试次数
        let (_, should_retry) = manager.handle_length_error(history, 10);

        assert!(!should_retry);
    }

    #[tokio::test]
    async fn test_handle_length_error_with_summary_success() {
        let mut manager = HistoryManager::with_defaults();
        let history = create_test_history(30);
        let mock_gen = MockSummaryGenerator::new("这是生成的摘要内容");

        let (result, should_retry) = manager
            .handle_length_error_with_summary(
                history,
                "claude-sonnet-4.5",
                0,
                Some(&mock_gen),
            )
            .await;

        assert!(should_retry);
        assert!(manager.truncate_info.used_summary);
        // 结果应该包含摘要消息
        if let Some(Message::User(user_msg)) = result.first() {
            assert!(user_msg
                .user_input_message
                .content
                .contains("[Earlier conversation summary]"));
        }
    }

    #[tokio::test]
    async fn test_handle_length_error_with_summary_fallback() {
        let mut manager = HistoryManager::with_defaults();
        let history = create_test_history(30);
        let mock_gen = MockSummaryGenerator::failing();

        let (result, should_retry) = manager
            .handle_length_error_with_summary(
                history,
                "claude-sonnet-4.5",
                0,
                Some(&mock_gen),
            )
            .await;

        assert!(should_retry);
        // 摘要失败应该回退到简单截断
        assert!(!manager.truncate_info.used_summary);
        assert!(result.len() <= 20);
    }

    #[tokio::test]
    async fn test_summary_cache() {
        let cache = SummaryCache::new(10);
        let key = "test_session:10";
        let summary = "缓存的摘要内容";

        // 设置缓存
        cache.set(key.to_string(), summary.to_string(), 5).await;

        // 获取缓存（应该命中）
        let result = cache
            .get(key, 5, Duration::from_secs(60), 3)
            .await;
        assert_eq!(result, Some(summary.to_string()));

        // 历史变化太大时不应命中
        let result = cache
            .get(key, 10, Duration::from_secs(60), 3)
            .await;
        assert!(result.is_none());
    }

    #[test]
    fn test_history_config_has_strategy() {
        let config = HistoryConfig::default();

        assert!(config.has_strategy(TruncateStrategy::ErrorRetry));
        assert!(config.has_strategy(TruncateStrategy::SmartSummary));
        assert!(!config.has_strategy(TruncateStrategy::PreEstimate));
    }

    #[test]
    fn test_format_history_for_summary() {
        let history = create_test_history(4);
        let formatted = HistoryManager::format_history_for_summary(&history);

        assert!(formatted.contains("[user]:"));
        assert!(formatted.contains("[assistant]:"));
        assert!(formatted.contains("用户消息"));
        assert!(formatted.contains("助手回复"));
    }

    #[test]
    fn test_build_summary_prompt() {
        let history = create_test_history(4);
        let prompt = HistoryManager::build_summary_prompt(&history, 2000);

        assert!(prompt.contains("请简洁地总结"));
        assert!(prompt.contains("用户的主要目标"));
        assert!(prompt.contains("2000 字符"));
    }
}

    /// 集成测试：使用真实 Kiro API 生成摘要
    /// 需要有效的 credentials.json 才能运行
    /// 运行命令: cargo test test_kiro_summary_generator_integration -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_kiro_summary_generator_integration() {
        use crate::kiro::model::credentials::CredentialsConfig;
        use crate::kiro::token_manager::MultiTokenManager;
        use crate::kiro::provider::KiroProvider;
        use crate::model::config::Config;
        use std::sync::Arc;

        // 加载配置和凭证
        let config = Config::load("../config.json")
            .expect("无法加载 config.json");
        let credentials_config = CredentialsConfig::load("../credentials.json")
            .expect("无法加载 credentials.json");
        let credentials_list = credentials_config.into_sorted_credentials();

        if credentials_list.is_empty() {
            println!("⚠️ 没有可用凭证，跳过测试");
            return;
        }

        // 获取第一个凭证的 profile_arn
        let profile_arn = credentials_list.first().and_then(|c| c.profile_arn.clone());
        println!("profile_arn: {:?}", profile_arn);

        // 创建 token manager
        let token_manager = MultiTokenManager::new(
            config.clone(),
            credentials_list,
            None,
            None,
            false,
        ).expect("无法创建 MultiTokenManager");

        // 创建 provider
        let provider = KiroProvider::new(Arc::new(token_manager))
            .expect("无法创建 KiroProvider");

        // 创建摘要生成器
        let generator = KiroSummaryGenerator::new(
            Arc::new(provider),
            profile_arn,
        );

        // 测试摘要生成
        let test_prompt = r#"请简洁地总结以下对话历史的关键信息：

[user]: 我想创建一个 Rust 项目来处理 JSON 数据
[assistant]: 好的，我可以帮你创建。你需要使用 serde 和 serde_json 库。
[user]: 请帮我写一个解析 JSON 的函数
[assistant]: 这是一个简单的 JSON 解析函数...

请用中文输出摘要，控制在 500 字符以内："#;

        println!("发送摘要请求...");
        let result = generator.generate(test_prompt).await;

        match result {
            Ok(summary) => {
                println!("✅ 摘要生成成功！");
                println!("摘要内容 ({} 字符):\n{}", summary.len(), summary);
                assert!(!summary.is_empty(), "摘要不应为空");
                assert!(summary.len() < 1000, "摘要应该简洁");
            }
            Err(e) => {
                println!("❌ 摘要生成失败: {}", e);
                panic!("摘要生成失败: {}", e);
            }
        }
    }

    /// 集成测试：完整的摘要重试流程
    /// 运行命令: cargo test test_full_summary_retry_flow -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_full_summary_retry_flow() {
        use crate::kiro::model::credentials::CredentialsConfig;
        use crate::kiro::token_manager::MultiTokenManager;
        use crate::kiro::provider::KiroProvider;
        use crate::model::config::Config;
        use std::sync::Arc;

        // 加载配置和凭证
        let config = Config::load("../config.json")
            .expect("无法加载 config.json");
        let credentials_config = CredentialsConfig::load("../credentials.json")
            .expect("无法加载 credentials.json");
        let credentials_list = credentials_config.into_sorted_credentials();

        if credentials_list.is_empty() {
            println!("⚠️ 没有可用凭证，跳过测试");
            return;
        }

        // 获取第一个凭证的 profile_arn
        let profile_arn = credentials_list.first().and_then(|c| c.profile_arn.clone());
        println!("profile_arn: {:?}", profile_arn);

        // 创建 token manager
        let token_manager = MultiTokenManager::new(
            config.clone(),
            credentials_list,
            None,
            None,
            false,
        ).expect("无法创建 MultiTokenManager");

        // 创建 provider
        let provider = Arc::new(
            KiroProvider::new(Arc::new(token_manager))
                .expect("无法创建 KiroProvider")
        );

        // 创建摘要生成器
        let generator = KiroSummaryGenerator::new(
            provider.clone(),
            profile_arn,
        );

        // 创建一个较长的历史消息列表（模拟超限场景）
        let mut history = Vec::new();
        for i in 0..30 {
            if i % 2 == 0 {
                history.push(Message::User(HistoryUserMessage::new(
                    format!("这是第 {} 条用户消息，包含一些测试内容来模拟真实对话。", i / 2 + 1),
                    "claude-sonnet-4.5",
                )));
            } else {
                history.push(Message::Assistant(HistoryAssistantMessage::new(
                    &format!("这是第 {} 条助手回复，我会尽力帮助你完成任务。", i / 2 + 1),
                )));
            }
        }

        println!("创建了 {} 条历史消息", history.len());

        // 创建 HistoryManager 并测试摘要重试
        let mut manager = HistoryManager::with_defaults()
            .with_cache_key("test_session".to_string());

        println!("测试 handle_length_error_with_summary...");
        let (result, should_retry) = manager
            .handle_length_error_with_summary(
                history,
                "claude-sonnet-4.5",
                0,
                Some(&generator),
            )
            .await;

        println!("should_retry: {}", should_retry);
        println!("truncate_info: {:?}", manager.truncate_info());
        println!("结果消息数: {}", result.len());

        if manager.truncate_info().used_summary {
            println!("✅ 使用了智能摘要！");
            // 检查第一条消息是否包含摘要
            if let Some(Message::User(user_msg)) = result.first() {
                let content = &user_msg.user_input_message.content;
                println!("摘要消息内容:\n{}", content);
                assert!(content.contains("[Earlier conversation summary]"));
            }
        } else {
            println!("⚠️ 回退到简单截断");
        }

        assert!(should_retry, "应该建议重试");
        assert!(result.len() < 30, "消息数应该减少");
    }
