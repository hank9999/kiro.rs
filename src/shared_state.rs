//! 跨模块共享的运行时状态
//!
//! 用于解决 `anthropic` 与 `admin` 之间的反向依赖：
//! - `admin::service::AdminService` 需要"写入"模型缓存（后台刷新任务）
//! - `anthropic::handlers::get_models` 需要"读取"模型缓存
//!
//! 两者都通过克隆 [`ModelsCacheHandle`]（内部 `Arc<Mutex<...>>`）共享同一份数据。

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::sync::Arc;

use crate::anthropic::types::Model;

/// 模型列表缓存内部状态
#[derive(Debug, Clone)]
pub struct ModelsCacheState {
    /// 当前对外暴露的模型列表
    pub models: Vec<Model>,
    /// 上游标记的默认模型 ID（如 "auto"）；fallback 模式下为 None
    pub default_id: Option<String>,
    /// 最近一次刷新成功的时间
    pub last_refreshed_at: Option<DateTime<Utc>>,
    /// 最近一次刷新错误（最近一次失败的描述；成功后清空）
    pub last_error: Option<String>,
    /// 当前是否在使用 fallback 列表（首次加载尚未成功 / 完全禁用动态时为 true）
    pub is_fallback: bool,
}

/// 可被多模块共享读写的模型缓存句柄
#[derive(Debug, Clone)]
pub struct ModelsCacheHandle {
    inner: Arc<Mutex<ModelsCacheState>>,
}

impl ModelsCacheHandle {
    /// 用一份初始模型列表创建缓存（首次启动应传 fallback_models）
    pub fn new_with_fallback(models: Vec<Model>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ModelsCacheState {
                models,
                default_id: None,
                last_refreshed_at: None,
                last_error: None,
                is_fallback: true,
            })),
        }
    }

    /// 读取当前模型列表（克隆一份返回，避免持有锁）
    pub fn snapshot_models(&self) -> Vec<Model> {
        self.inner.lock().models.clone()
    }

    /// 读取完整状态（用于 admin 监控页面诊断）
    #[allow(dead_code)]
    pub fn snapshot(&self) -> ModelsCacheState {
        self.inner.lock().clone()
    }

    /// 替换模型列表（刷新成功后调用）
    pub fn replace(&self, models: Vec<Model>, default_id: Option<String>) {
        let mut state = self.inner.lock();
        state.models = models;
        state.default_id = default_id;
        state.last_refreshed_at = Some(Utc::now());
        state.last_error = None;
        state.is_fallback = false;
    }

    /// 标记一次刷新失败（不清除现有缓存）
    pub fn mark_error(&self, msg: impl Into<String>) {
        let mut state = self.inner.lock();
        state.last_error = Some(msg.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Model;

    fn dummy_model(id: &str) -> Model {
        Model {
            id: id.to_string(),
            object: "model".to_string(),
            created: 0,
            owned_by: "test".to_string(),
            display_name: id.to_string(),
            model_type: "chat".to_string(),
            max_tokens: 64000,
        }
    }

    #[test]
    fn test_initial_state_is_fallback() {
        let handle = ModelsCacheHandle::new_with_fallback(vec![dummy_model("a")]);
        let snap = handle.snapshot();
        assert!(snap.is_fallback);
        assert_eq!(snap.models.len(), 1);
        assert!(snap.last_refreshed_at.is_none());
        assert!(snap.last_error.is_none());
    }

    #[test]
    fn test_replace_clears_fallback_flag() {
        let handle = ModelsCacheHandle::new_with_fallback(vec![dummy_model("a")]);
        handle.replace(
            vec![dummy_model("b"), dummy_model("c")],
            Some("auto".to_string()),
        );
        let snap = handle.snapshot();
        assert!(!snap.is_fallback);
        assert_eq!(snap.models.len(), 2);
        assert_eq!(snap.default_id.as_deref(), Some("auto"));
        assert!(snap.last_refreshed_at.is_some());
    }

    #[test]
    fn test_mark_error_keeps_existing_models() {
        let handle = ModelsCacheHandle::new_with_fallback(vec![dummy_model("a")]);
        handle.mark_error("network timeout");
        let snap = handle.snapshot();
        assert_eq!(snap.models.len(), 1);
        assert_eq!(snap.last_error.as_deref(), Some("network timeout"));
    }

    #[test]
    fn test_handle_is_cloneable_and_shared() {
        let h1 = ModelsCacheHandle::new_with_fallback(vec![dummy_model("a")]);
        let h2 = h1.clone();
        h1.replace(vec![dummy_model("z")], None);
        assert_eq!(h2.snapshot_models()[0].id, "z");
    }
}
