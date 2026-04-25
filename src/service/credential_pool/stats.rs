//! 凭据使用统计：内存数据 + caller 触发持久化

use std::collections::HashMap;

use chrono::Utc;
use parking_lot::Mutex;

use crate::infra::storage::StatsEntry;

/// 单条凭据的内存统计
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryStats {
    pub success_count: u64,
    pub last_used_at: Option<String>,
}

impl EntryStats {
    pub fn from_storage(entry: StatsEntry) -> Self {
        Self {
            success_count: entry.success_count,
            last_used_at: entry.last_used_at,
        }
    }

    pub fn to_storage(&self) -> StatsEntry {
        StatsEntry {
            success_count: self.success_count,
            last_used_at: self.last_used_at.clone(),
        }
    }
}

/// 内存中的统计 map
#[derive(Default)]
pub struct CredentialStats {
    entries: Mutex<HashMap<u64, EntryStats>>,
}

impl CredentialStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_storage(loaded: HashMap<u64, StatsEntry>) -> Self {
        let entries = loaded
            .into_iter()
            .map(|(id, s)| (id, EntryStats::from_storage(s)))
            .collect();
        Self {
            entries: Mutex::new(entries),
        }
    }

    pub fn upsert(&self, id: u64, stats: EntryStats) {
        self.entries.lock().insert(id, stats);
    }

    pub fn remove(&self, id: u64) {
        self.entries.lock().remove(&id);
    }

    pub fn get(&self, id: u64) -> Option<EntryStats> {
        self.entries.lock().get(&id).cloned()
    }

    /// 记录一次成功使用：success_count++ + last_used_at = now
    pub fn record_use(&self, id: u64) {
        let mut entries = self.entries.lock();
        let entry = entries.entry(id).or_default();
        entry.success_count += 1;
        entry.last_used_at = Some(Utc::now().to_rfc3339());
    }

    pub fn snapshot(&self) -> HashMap<u64, EntryStats> {
        self.entries.lock().clone()
    }

    /// 转换为持久化格式
    pub fn to_storage_map(&self) -> HashMap<u64, StatsEntry> {
        self.entries
            .lock()
            .iter()
            .map(|(id, s)| (*id, s.to_storage()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_use_increments_and_sets_timestamp() {
        let stats = CredentialStats::new();
        stats.record_use(7);
        let s = stats.get(7).unwrap();
        assert_eq!(s.success_count, 1);
        assert!(s.last_used_at.is_some());
    }

    #[test]
    fn record_use_multiple_times_increments() {
        let stats = CredentialStats::new();
        stats.record_use(1);
        stats.record_use(1);
        stats.record_use(1);
        assert_eq!(stats.get(1).unwrap().success_count, 3);
    }

    #[test]
    fn upsert_replaces() {
        let stats = CredentialStats::new();
        stats.upsert(
            10,
            EntryStats {
                success_count: 100,
                last_used_at: Some("ts".into()),
            },
        );
        assert_eq!(stats.get(10).unwrap().success_count, 100);
    }

    #[test]
    fn from_storage_roundtrip_via_to_storage_map() {
        let mut loaded = HashMap::new();
        loaded.insert(
            5,
            StatsEntry {
                success_count: 42,
                last_used_at: Some("2026-04-25T00:00:00Z".into()),
            },
        );
        let stats = CredentialStats::from_storage(loaded);
        let map = stats.to_storage_map();
        assert_eq!(map.get(&5).unwrap().success_count, 42);
    }

    #[test]
    fn remove_deletes() {
        let stats = CredentialStats::new();
        stats.record_use(3);
        stats.remove(3);
        assert!(stats.get(3).is_none());
    }
}
