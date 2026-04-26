//! 凭据统计的去抖落盘
//!
//! - `record()`：首次调用启动一次延迟 task；同一窗口内的重复调用合并为单次 save。
//! - `flush()`：同步立即落盘并重置 pending（delete_credential / shutdown 路径用）。
//!
//! 落盘失败仅 warn，不向 caller 抛错——统计不是关键数据，磁盘抖动不应阻塞请求。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::infra::storage::StatsFileStore;

use super::stats::CredentialStats;

pub const DEFAULT_STATS_DEBOUNCE: Duration = Duration::from_secs(30);

pub struct StatsPersister {
    stats: Arc<CredentialStats>,
    store: Arc<StatsFileStore>,
    debounce: Mutex<Duration>,
    pending: AtomicBool,
    /// 串行化 fs::write，避免 timer + flush 并发写同一文件导致 JSON 部分写损坏
    save_lock: Mutex<()>,
}

impl StatsPersister {
    pub fn new(stats: Arc<CredentialStats>, store: Arc<StatsFileStore>) -> Self {
        Self {
            stats,
            store,
            debounce: Mutex::new(DEFAULT_STATS_DEBOUNCE),
            pending: AtomicBool::new(false),
            save_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub fn set_debounce(&self, d: Duration) {
        *self.debounce.lock() = d;
    }

    fn debounce(&self) -> Duration {
        *self.debounce.lock()
    }

    /// 记录一次写动作；首次调用启动定时器，重复调用合并到同一窗口。
    ///
    /// 接收者签名为 `&Arc<Self>` 而非 `&self`：spawn 的 task 需要 `'static` 生命周期，
    /// 必须 `Arc::clone(self)` 进 future。改为 `&self` 会让 caller 无法构造 `'static` 对象。
    pub fn record(self: &Arc<Self>) {
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let debounce = self.debounce();
        let me = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(debounce).await;
            me.flush_inner();
        });
    }

    /// 强制立即落盘并重置 pending；幂等。
    pub fn flush(&self) {
        self.flush_inner();
    }

    fn flush_inner(&self) {
        // save_lock 串行化 fs::write：timer 醒来与显式 flush 可能并发触发，
        // 多个 fs::write 同时写同一 path 会导致 JSON 部分写损坏。
        let _save_guard = self.save_lock.lock();
        // 先 reset pending 再 save：reset → snapshot 期间进入的 record 能启动下一窗口；
        // save_lock 保证后续的 timer 醒来时不会与本次 save 交叉，写入仍是原子语义。
        self.pending.store(false, Ordering::Release);
        let map = self.stats.to_storage_map();
        if let Err(e) = self.store.save(&map) {
            tracing::warn!(?e, "stats 落盘失败");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::credential_pool::stats::EntryStats;
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-stats-persister-test-{tag}-{id}.json"))
    }

    fn fixture(stats_path: &Path) -> (Arc<StatsPersister>, Arc<CredentialStats>) {
        let stats = Arc::new(CredentialStats::new());
        stats.upsert(
            1,
            EntryStats {
                success_count: 5,
                last_used_at: Some("2026-04-26T00:00:00Z".into()),
            },
        );
        let store = Arc::new(StatsFileStore::new(Some(stats_path.to_path_buf())));
        let persister = Arc::new(StatsPersister::new(stats.clone(), store));
        (persister, stats)
    }

    #[tokio::test]
    async fn record_does_not_persist_within_debounce_window() {
        let path = tmp_path("debounce-window");
        let (persister, _) = fixture(&path);
        persister.set_debounce(Duration::from_millis(150));

        for _ in 0..5 {
            persister.record();
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!path.exists(), "debounce 内不应落盘: {:?}", path);

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(path.exists(), "debounce 后定时器应已触发落盘");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn flush_persists_immediately_even_within_debounce_window() {
        let path = tmp_path("flush-now");
        let (persister, _) = fixture(&path);
        persister.set_debounce(Duration::from_secs(60));

        persister.record();
        assert!(!path.exists(), "spawn 之后还未到 debounce，不应落盘");

        persister.flush();
        assert!(path.exists(), "flush 应同步落盘");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn flush_then_record_can_schedule_next_window() {
        let path = tmp_path("flush-reset");
        let (persister, _) = fixture(&path);
        persister.set_debounce(Duration::from_millis(120));

        persister.record();
        persister.flush();
        let _ = std::fs::remove_file(&path);

        // flush 后 pending 已 reset，下一次 record 应能再次 spawn 定时器
        persister.record();
        tokio::time::sleep(Duration::from_millis(180)).await;
        assert!(path.exists(), "flush 后 pending 应已 reset，新窗口的定时器应能落盘");

        let _ = std::fs::remove_file(&path);
    }
}
