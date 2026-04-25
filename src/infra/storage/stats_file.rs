//! 凭据使用统计持久化（kiro_stats.json）

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::error::ConfigError;

/// 单条凭据的统计数据
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatsEntry {
    pub success_count: u64,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

pub struct StatsFileStore {
    path: Option<PathBuf>,
}

impl StatsFileStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    /// 加载所有统计；文件不存在或解析失败返回空 map
    pub fn load(&self) -> HashMap<u64, StatsEntry> {
        let path = match &self.path {
            Some(p) => p,
            None => return HashMap::new(),
        };
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };
        let raw: HashMap<String, StatsEntry> = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("解析统计缓存失败，将忽略: {}", e);
                return HashMap::new();
            }
        };
        raw.into_iter()
            .filter_map(|(k, v)| k.parse::<u64>().ok().map(|id| (id, v)))
            .collect()
    }

    /// 保存所有统计（key 序列化为 String 兼容 JSON）
    pub fn save(&self, stats: &HashMap<u64, StatsEntry>) -> Result<bool, ConfigError> {
        let path = match &self.path {
            Some(p) => p,
            None => return Ok(false),
        };
        let raw: HashMap<String, &StatsEntry> =
            stats.iter().map(|(k, v)| (k.to_string(), v)).collect();
        let json = serde_json::to_string_pretty(&raw)?;
        std::fs::write(path, json)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-stats-test-{tag}-{id}.json"))
    }

    #[test]
    fn load_empty_when_no_path() {
        let store = StatsFileStore::new(None);
        assert!(store.load().is_empty());
    }

    #[test]
    fn save_then_load_roundtrip_preserves_fields() {
        let path = tmp_path("roundtrip");
        let store = StatsFileStore::new(Some(path.clone()));

        let mut stats = HashMap::new();
        stats.insert(
            42,
            StatsEntry {
                success_count: 10,
                last_used_at: Some("2026-04-25T10:00:00Z".into()),
            },
        );
        stats.insert(
            7,
            StatsEntry {
                success_count: 0,
                last_used_at: None,
            },
        );

        let written = store.save(&stats).unwrap();
        assert!(written);

        let reloaded = store.load();
        assert_eq!(reloaded, stats);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let path = tmp_path("missing");
        let store = StatsFileStore::new(Some(path));
        assert!(store.load().is_empty());
    }

    #[test]
    fn load_malformed_json_returns_empty() {
        let path = tmp_path("malformed");
        fs::write(&path, "not-json").unwrap();
        let store = StatsFileStore::new(Some(path.clone()));
        assert!(store.load().is_empty());
        let _ = fs::remove_file(&path);
    }
}
