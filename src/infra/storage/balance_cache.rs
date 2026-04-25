//! 余额缓存持久化（kiro_balance_cache.json）
//!
//! 默认 TTL 5 分钟。caller 提供 `serde_json::Value` 作为缓存数据，
//! 避免与 admin DTO 形成循环依赖。

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// 默认余额缓存 TTL（5 分钟）
pub const BALANCE_CACHE_TTL_SECS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedBalanceEntry {
    pub cached_at: f64,
    pub data: serde_json::Value,
}

pub struct BalanceCacheStore {
    path: Option<PathBuf>,
    cache: Mutex<HashMap<u64, CachedBalanceEntry>>,
    ttl_secs: i64,
}

impl BalanceCacheStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self::with_ttl(path, BALANCE_CACHE_TTL_SECS)
    }

    pub fn with_ttl(path: Option<PathBuf>, ttl_secs: i64) -> Self {
        let cache = Self::load_initial(&path, ttl_secs);
        Self {
            path,
            cache: Mutex::new(cache),
            ttl_secs,
        }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn ttl_secs(&self) -> i64 {
        self.ttl_secs
    }

    /// 取未过期的缓存项；过期或未命中返回 None
    pub fn get(&self, id: u64) -> Option<serde_json::Value> {
        let cache = self.cache.lock();
        let entry = cache.get(&id)?;
        let now = Utc::now().timestamp() as f64;
        if (now - entry.cached_at) < self.ttl_secs as f64 {
            Some(entry.data.clone())
        } else {
            None
        }
    }

    /// 写入缓存（cached_at 取当前时间）+ 持久化到磁盘
    pub fn put(&self, id: u64, data: serde_json::Value) {
        let entry = CachedBalanceEntry {
            cached_at: Utc::now().timestamp() as f64,
            data,
        };
        {
            let mut cache = self.cache.lock();
            cache.insert(id, entry);
        }
        self.persist();
    }

    /// 失效单条缓存 + 持久化
    pub fn invalidate(&self, id: u64) {
        let removed = {
            let mut cache = self.cache.lock();
            cache.remove(&id).is_some()
        };
        if removed {
            self.persist();
        }
    }

    fn load_initial(
        path: &Option<PathBuf>,
        ttl_secs: i64,
    ) -> HashMap<u64, CachedBalanceEntry> {
        let path = match path {
            Some(p) => p,
            None => return HashMap::new(),
        };
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };
        let map: HashMap<String, CachedBalanceEntry> = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("解析余额缓存失败，将忽略: {}", e);
                return HashMap::new();
            }
        };
        let now = Utc::now().timestamp() as f64;
        map.into_iter()
            .filter_map(|(k, v)| {
                let id = k.parse::<u64>().ok()?;
                if (now - v.cached_at) < ttl_secs as f64 {
                    Some((id, v))
                } else {
                    None
                }
            })
            .collect()
    }

    fn persist(&self) {
        let path = match &self.path {
            Some(p) => p,
            None => return,
        };
        let cache = self.cache.lock();
        let map: HashMap<String, &CachedBalanceEntry> =
            cache.iter().map(|(k, v)| (k.to_string(), v)).collect();
        match serde_json::to_string_pretty(&map) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::warn!("保存余额缓存失败: {}", e);
                }
            }
            Err(e) => tracing::warn!("序列化余额缓存失败: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use uuid::Uuid;

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-balance-test-{tag}-{id}.json"))
    }

    #[test]
    fn put_then_get_returns_data() {
        let path = tmp_path("put-get");
        let store = BalanceCacheStore::new(Some(path.clone()));
        store.put(7, json!({"balance": 100, "currency": "USD"}));
        let v = store.get(7).expect("should hit");
        assert_eq!(v["balance"], 100);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn get_returns_none_when_missing() {
        let path = tmp_path("missing");
        let store = BalanceCacheStore::new(Some(path));
        assert!(store.get(99).is_none());
    }

    #[test]
    fn put_persists_to_disk_and_reload_works() {
        let path = tmp_path("persist");
        {
            let store = BalanceCacheStore::new(Some(path.clone()));
            store.put(1, json!({"balance": 42}));
        }
        let store2 = BalanceCacheStore::new(Some(path.clone()));
        let v = store2.get(1).expect("should reload");
        assert_eq!(v["balance"], 42);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn expired_entry_is_filtered_on_reload() {
        let path = tmp_path("expired");
        // 手写一个 cached_at 是 2020 年的 entry
        let raw = serde_json::json!({
            "1": { "cached_at": 1577836800.0, "data": {"balance": 1} }
        });
        fs::write(&path, raw.to_string()).unwrap();
        let store = BalanceCacheStore::new(Some(path.clone()));
        // 现在时间 - 2020 = 远超 300s → 应被丢弃
        assert!(store.get(1).is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn invalidate_removes_entry() {
        let path = tmp_path("invalidate");
        let store = BalanceCacheStore::new(Some(path.clone()));
        store.put(5, json!({"x": 1}));
        assert!(store.get(5).is_some());
        store.invalidate(5);
        assert!(store.get(5).is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn no_path_does_not_persist_but_still_caches() {
        let store = BalanceCacheStore::new(None);
        store.put(1, json!({"x": 2}));
        assert_eq!(store.get(1).unwrap()["x"], 2);
    }
}
