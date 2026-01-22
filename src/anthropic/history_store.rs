//! 历史记录持久化存储
//!
//! 按 session_id 将对话历史保存到文件，支持：
//! - 自动保存/加载历史
//! - 过期清理（默认 24 小时）
//! - 并发安全访问

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::kiro::model::requests::conversation::Message;

/// 历史存储配置
#[derive(Debug, Clone)]
pub struct HistoryStoreConfig {
    /// 存储目录
    pub storage_dir: PathBuf,
    /// 历史过期时间（秒）
    pub expire_secs: u64,
    /// 是否启用持久化
    pub enabled: bool,
}

impl Default for HistoryStoreConfig {
    fn default() -> Self {
        Self {
            storage_dir: PathBuf::from("history"),
            expire_secs: 24 * 60 * 60, // 24 小时
            enabled: true,
        }
    }
}

/// 持久化的历史记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedHistory {
    /// 会话 ID
    pub session_id: String,
    /// 历史消息
    pub messages: Vec<Message>,
    /// 创建时间戳（毫秒）
    pub created_at: u64,
    /// 最后更新时间戳（毫秒）
    pub updated_at: u64,
    /// 摘要（如果有）
    pub summary: Option<String>,
}

impl PersistedHistory {
    /// 创建新的历史记录
    pub fn new(session_id: String, messages: Vec<Message>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            session_id,
            messages,
            created_at: now,
            updated_at: now,
            summary: None,
        }
    }

    /// 更新历史消息
    pub fn update(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
    }

    /// 设置摘要
    pub fn with_summary(mut self, summary: String) -> Self {
        self.summary = Some(summary);
        self
    }

    /// 检查是否过期
    pub fn is_expired(&self, expire_secs: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let expire_ms = expire_secs * 1000;
        now.saturating_sub(self.updated_at) > expire_ms
    }
}

/// 历史存储管理器
#[derive(Debug, Clone)]
pub struct HistoryStore {
    config: HistoryStoreConfig,
    /// 内存缓存
    cache: Arc<RwLock<HashMap<String, PersistedHistory>>>,
}

impl HistoryStore {
    /// 创建新的历史存储
    pub fn new(config: HistoryStoreConfig) -> Self {
        // 确保存储目录存在
        if config.enabled {
            if let Err(e) = std::fs::create_dir_all(&config.storage_dir) {
                tracing::warn!("创建历史存储目录失败: {}", e);
            }
        }

        Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(HistoryStoreConfig::default())
    }

    /// 获取文件路径
    fn get_file_path(&self, session_id: &str) -> PathBuf {
        // 清理 session_id 中的非法字符
        let safe_id: String = session_id
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.config.storage_dir.join(format!("{}.json", safe_id))
    }

    /// 保存历史记录
    pub fn save(&self, session_id: &str, messages: Vec<Message>) -> Result<(), String> {
        if !self.config.enabled {
            return Ok(());
        }

        let history = {
            let cache = self.cache.read().map_err(|e| e.to_string())?;
            if let Some(existing) = cache.get(session_id) {
                let mut h = existing.clone();
                h.update(messages);
                h
            } else {
                PersistedHistory::new(session_id.to_string(), messages)
            }
        };

        // 更新缓存
        {
            let mut cache = self.cache.write().map_err(|e| e.to_string())?;
            cache.insert(session_id.to_string(), history.clone());
        }

        // 写入文件
        let path = self.get_file_path(session_id);
        let json = serde_json::to_string_pretty(&history)
            .map_err(|e| format!("序列化历史失败: {}", e))?;
        
        std::fs::write(&path, json)
            .map_err(|e| format!("写入历史文件失败: {}", e))?;

        tracing::debug!(
            session_id = %session_id,
            message_count = history.messages.len(),
            path = %path.display(),
            "历史记录已保存"
        );

        Ok(())
    }

    /// 保存历史记录（带摘要）
    pub fn save_with_summary(
        &self,
        session_id: &str,
        messages: Vec<Message>,
        summary: String,
    ) -> Result<(), String> {
        if !self.config.enabled {
            return Ok(());
        }

        let mut history = PersistedHistory::new(session_id.to_string(), messages);
        history.summary = Some(summary);

        // 更新缓存
        {
            let mut cache = self.cache.write().map_err(|e| e.to_string())?;
            cache.insert(session_id.to_string(), history.clone());
        }

        // 写入文件
        let path = self.get_file_path(session_id);
        let json = serde_json::to_string_pretty(&history)
            .map_err(|e| format!("序列化历史失败: {}", e))?;
        
        std::fs::write(&path, json)
            .map_err(|e| format!("写入历史文件失败: {}", e))?;

        tracing::debug!(
            session_id = %session_id,
            message_count = history.messages.len(),
            has_summary = true,
            path = %path.display(),
            "历史记录已保存（含摘要）"
        );

        Ok(())
    }

    /// 加载历史记录
    pub fn load(&self, session_id: &str) -> Option<PersistedHistory> {
        if !self.config.enabled {
            return None;
        }

        // 先检查缓存
        {
            let cache = self.cache.read().ok()?;
            if let Some(history) = cache.get(session_id) {
                if !history.is_expired(self.config.expire_secs) {
                    return Some(history.clone());
                }
            }
        }

        // 从文件加载
        let path = self.get_file_path(session_id);
        if !path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&path).ok()?;
        let history: PersistedHistory = serde_json::from_str(&content).ok()?;

        // 检查是否过期
        if history.is_expired(self.config.expire_secs) {
            // 删除过期文件
            let _ = std::fs::remove_file(&path);
            return None;
        }

        // 更新缓存
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(session_id.to_string(), history.clone());
        }

        tracing::debug!(
            session_id = %session_id,
            message_count = history.messages.len(),
            "历史记录已加载"
        );

        Some(history)
    }

    /// 获取历史消息
    pub fn get_messages(&self, session_id: &str) -> Option<Vec<Message>> {
        self.load(session_id).map(|h| h.messages)
    }

    /// 获取摘要
    pub fn get_summary(&self, session_id: &str) -> Option<String> {
        self.load(session_id).and_then(|h| h.summary)
    }

    /// 删除历史记录
    pub fn delete(&self, session_id: &str) -> Result<(), String> {
        // 从缓存删除
        if let Ok(mut cache) = self.cache.write() {
            cache.remove(session_id);
        }

        // 删除文件
        let path = self.get_file_path(session_id);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("删除历史文件失败: {}", e))?;
        }

        tracing::debug!(session_id = %session_id, "历史记录已删除");
        Ok(())
    }

    /// 清理过期历史
    pub fn cleanup_expired(&self) -> Result<usize, String> {
        if !self.config.enabled {
            return Ok(0);
        }

        let mut cleaned = 0;

        // 清理缓存
        {
            let mut cache = self.cache.write().map_err(|e| e.to_string())?;
            let expired_keys: Vec<String> = cache
                .iter()
                .filter(|(_, h)| h.is_expired(self.config.expire_secs))
                .map(|(k, _)| k.clone())
                .collect();
            
            for key in expired_keys {
                cache.remove(&key);
                cleaned += 1;
            }
        }

        // 清理文件
        let entries = std::fs::read_dir(&self.config.storage_dir)
            .map_err(|e| format!("读取存储目录失败: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(history) = serde_json::from_str::<PersistedHistory>(&content) {
                        if history.is_expired(self.config.expire_secs) {
                            if std::fs::remove_file(&path).is_ok() {
                                cleaned += 1;
                            }
                        }
                    }
                }
            }
        }

        if cleaned > 0 {
            tracing::info!(cleaned = cleaned, "已清理过期历史记录");
        }

        Ok(cleaned)
    }

    /// 列出所有会话
    pub fn list_sessions(&self) -> Result<Vec<String>, String> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();

        let entries = std::fs::read_dir(&self.config.storage_dir)
            .map_err(|e| format!("读取存储目录失败: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem() {
                    sessions.push(stem.to_string_lossy().to_string());
                }
            }
        }

        Ok(sessions)
    }

    /// 获取存储统计
    pub fn stats(&self) -> HistoryStoreStats {
        let cache_size = self.cache.read().map(|c| c.len()).unwrap_or(0);
        
        let (file_count, total_size) = if self.config.enabled {
            std::fs::read_dir(&self.config.storage_dir)
                .map(|entries| {
                    let mut count = 0;
                    let mut size = 0u64;
                    for entry in entries.flatten() {
                        if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                            count += 1;
                            size += entry.metadata().map(|m| m.len()).unwrap_or(0);
                        }
                    }
                    (count, size)
                })
                .unwrap_or((0, 0))
        } else {
            (0, 0)
        };

        HistoryStoreStats {
            cache_size,
            file_count,
            total_size_bytes: total_size,
            storage_dir: self.config.storage_dir.clone(),
            enabled: self.config.enabled,
        }
    }
}

/// 存储统计信息
#[derive(Debug, Clone)]
pub struct HistoryStoreStats {
    /// 缓存中的会话数
    pub cache_size: usize,
    /// 文件数量
    pub file_count: usize,
    /// 总文件大小（字节）
    pub total_size_bytes: u64,
    /// 存储目录
    pub storage_dir: PathBuf,
    /// 是否启用
    pub enabled: bool,
}

/// 全局历史存储实例
static HISTORY_STORE: std::sync::OnceLock<HistoryStore> = std::sync::OnceLock::new();

/// 初始化全局历史存储
pub fn init_global_store(config: HistoryStoreConfig) {
    let _ = HISTORY_STORE.set(HistoryStore::new(config));
}

/// 获取全局历史存储
pub fn global_store() -> &'static HistoryStore {
    HISTORY_STORE.get_or_init(HistoryStore::with_defaults)
}

/// 启动定期清理任务
pub fn start_cleanup_task(interval: Duration) {
    tokio::spawn(async move {
        let mut interval_timer = tokio::time::interval(interval);
        loop {
            interval_timer.tick().await;
            if let Err(e) = global_store().cleanup_expired() {
                tracing::warn!("清理过期历史失败: {}", e);
            }
        }
    });
}
