//! 可持久化的按账户（凭据 ID）统计
//!
//! 目标：
//! - 记录每个账户的调用次数（成功/失败）
//! - 记录用量（累计 input/output tokens）
//! - 记录最后一次调用错误（含时间）
//! - 进程重启后仍可读取（落盘到 JSON）

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use anyhow::Context;
use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::mpsc,
    task,
    time::{Duration, Sleep},
};

/// 单个 bucket（按日/按模型）的统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BucketStats {
    /// 调用次数（对上游发起请求的尝试次数）
    pub calls_total: u64,
    /// 成功次数（上游返回 2xx）
    pub calls_ok: u64,
    /// 失败次数（网络/非 2xx/流读取中断等）
    pub calls_err: u64,
    /// 累计输入 tokens
    pub input_tokens_total: u64,
    /// 累计输出 tokens
    pub output_tokens_total: u64,
    /// 最后一次调用时间（RFC3339）
    pub last_call_at: Option<String>,
    /// 最后一次成功时间（RFC3339）
    pub last_success_at: Option<String>,
    /// 最后一次错误时间（RFC3339）
    pub last_error_at: Option<String>,
    /// 最后一次错误（如果最后一次调用成功则为 None）
    pub last_error: Option<String>,
}

/// 单个账户（凭据 ID）的统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountStats {
    pub id: u64,

    /// 调用次数（对上游发起请求的尝试次数）
    pub calls_total: u64,

    /// 成功次数（上游返回 2xx，且 provider 认为成功）
    pub calls_ok: u64,

    /// 失败次数（网络错误、非 2xx、以及流式读取中断等）
    pub calls_err: u64,

    /// 累计输入 tokens
    pub input_tokens_total: u64,

    /// 累计输出 tokens
    pub output_tokens_total: u64,

    /// 最后一次调用时间（RFC3339）
    pub last_call_at: Option<String>,

    /// 最后一次成功时间（RFC3339）
    pub last_success_at: Option<String>,

    /// 最后一次错误时间（RFC3339）
    pub last_error_at: Option<String>,

    /// 最后一次错误（如果最后一次调用成功则为 None）
    pub last_error: Option<String>,

    /// 按日拆分（YYYY-MM-DD -> bucket）
    #[serde(default)]
    pub by_day: HashMap<String, BucketStats>,

    /// 按模型拆分（model -> bucket）
    #[serde(default)]
    pub by_model: HashMap<String, BucketStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StatsFile {
    /// 版本号（历史上存在 1；当前写入为 2）
    #[serde(default = "default_stats_version")]
    version: u32,
    #[serde(default)]
    accounts: Vec<AccountStats>,
}

fn default_stats_version() -> u32 {
    2
}

#[derive(Debug, Default)]
struct StatsInner {
    accounts: HashMap<u64, AccountStats>,
    dirty: bool,
}

/// 统计存储（线程安全 + 异步批量落盘）
#[derive(Debug)]
pub struct StatsStore {
    path: PathBuf,
    inner: RwLock<StatsInner>,
    flush_tx: mpsc::UnboundedSender<()>,
}

impl StatsStore {
    /// 从指定路径加载（不存在则创建空存储），并启动后台批量落盘任务。
    pub fn load_or_new(path: PathBuf) -> anyhow::Result<Arc<Self>> {
        let initial = load_from_path(&path)?;

        let (flush_tx, flush_rx) = mpsc::unbounded_channel();
        let store = Arc::new(Self {
            path,
            inner: RwLock::new(StatsInner {
                accounts: initial,
                dirty: false,
            }),
            flush_tx,
        });

        // 仅在 tokio runtime 内启动后台任务。
        if tokio::runtime::Handle::try_current().is_ok() {
            Self::spawn_flush_task(store.clone(), flush_rx);
        }

        Ok(store)
    }

    /// 获取某个账户的统计（不存在则返回 0 值）。
    pub fn get_or_default(&self, id: u64) -> AccountStats {
        let inner = self.inner.read();
        inner
            .accounts
            .get(&id)
            .cloned()
            .unwrap_or_else(|| AccountStats {
                id,
                ..AccountStats::default()
            })
    }

    /// 记录一次对上游的调用尝试。
    pub fn record_attempt(&self, id: u64, model: Option<&str>) {
        if id == 0 {
            return;
        }
        let now = Utc::now().to_rfc3339();
        let day = today_key();

        {
            let mut inner = self.inner.write();
            let stats = inner.accounts.entry(id).or_insert_with(|| AccountStats {
                id,
                ..AccountStats::default()
            });

            // attempt 只用于记录“最后调用时间”，不计入 calls_total
            stats.last_call_at = Some(now.clone());

            // by_day
            let day_bucket = stats.by_day.entry(day).or_default();
            day_bucket.last_call_at = Some(now.clone());

            // by_model
            if let Some(m) = model {
                let model_bucket = stats.by_model.entry(m.to_string()).or_default();
                model_bucket.last_call_at = Some(now);
            }

            inner.dirty = true;
        }
        self.schedule_flush();
    }

    /// 记录一次成功（并清空 last_error）。
    pub fn record_success(&self, id: u64, model: Option<&str>) {
        if id == 0 {
            return;
        }
        let now = Utc::now().to_rfc3339();
        let day = today_key();

        {
            let mut inner = self.inner.write();
            let stats = inner.accounts.entry(id).or_insert_with(|| AccountStats {
                id,
                ..AccountStats::default()
            });

            stats.calls_total = stats.calls_total.saturating_add(1);
            stats.calls_ok = stats.calls_ok.saturating_add(1);
            stats.last_success_at = Some(now.clone());
            stats.last_error = None;
            stats.last_error_at = None;

            // by_day
            let day_bucket = stats.by_day.entry(day).or_default();
            day_bucket.calls_total = day_bucket.calls_total.saturating_add(1);
            day_bucket.calls_ok = day_bucket.calls_ok.saturating_add(1);
            day_bucket.last_success_at = Some(now.clone());
            day_bucket.last_error = None;
            day_bucket.last_error_at = None;

            // by_model
            if let Some(m) = model {
                let model_bucket = stats.by_model.entry(m.to_string()).or_default();
                model_bucket.calls_total = model_bucket.calls_total.saturating_add(1);
                model_bucket.calls_ok = model_bucket.calls_ok.saturating_add(1);
                model_bucket.last_success_at = Some(now);
                model_bucket.last_error = None;
                model_bucket.last_error_at = None;
            }

            inner.dirty = true;
        }
        self.schedule_flush();
    }

    /// 记录一次失败（会覆盖 last_error）。
    pub fn record_error(&self, id: u64, model: Option<&str>, error: impl Into<String>) {
        if id == 0 {
            return;
        }
        let now = Utc::now().to_rfc3339();
        let day = today_key();
        let err = error.into();

        {
            let mut inner = self.inner.write();
            let stats = inner.accounts.entry(id).or_insert_with(|| AccountStats {
                id,
                ..AccountStats::default()
            });

            stats.calls_total = stats.calls_total.saturating_add(1);
            stats.calls_err = stats.calls_err.saturating_add(1);
            stats.last_error_at = Some(now.clone());
            stats.last_error = Some(err.clone());

            // by_day
            let day_bucket = stats.by_day.entry(day).or_default();
            day_bucket.calls_total = day_bucket.calls_total.saturating_add(1);
            day_bucket.calls_err = day_bucket.calls_err.saturating_add(1);
            day_bucket.last_error_at = Some(now.clone());
            day_bucket.last_error = Some(err.clone());

            // by_model
            if let Some(m) = model {
                let model_bucket = stats.by_model.entry(m.to_string()).or_default();
                model_bucket.calls_total = model_bucket.calls_total.saturating_add(1);
                model_bucket.calls_err = model_bucket.calls_err.saturating_add(1);
                model_bucket.last_error_at = Some(now);
                model_bucket.last_error = Some(err);
            }

            inner.dirty = true;
        }
        self.schedule_flush();
    }

    /// 增加用量统计。
    pub fn add_usage(&self, id: u64, model: Option<&str>, input_tokens: i64, output_tokens: i64) {
        if id == 0 {
            return;
        }

        let in_u = tokens_to_u64(input_tokens);
        let out_u = tokens_to_u64(output_tokens);

        if in_u == 0 && out_u == 0 {
            return;
        }

        let day = today_key();

        {
            let mut inner = self.inner.write();
            let stats = inner.accounts.entry(id).or_insert_with(|| AccountStats {
                id,
                ..AccountStats::default()
            });

            stats.input_tokens_total = stats.input_tokens_total.saturating_add(in_u);
            stats.output_tokens_total = stats.output_tokens_total.saturating_add(out_u);

            // by_day
            let day_bucket = stats.by_day.entry(day).or_default();
            day_bucket.input_tokens_total = day_bucket.input_tokens_total.saturating_add(in_u);
            day_bucket.output_tokens_total = day_bucket.output_tokens_total.saturating_add(out_u);

            // by_model
            if let Some(m) = model {
                let model_bucket = stats.by_model.entry(m.to_string()).or_default();
                model_bucket.input_tokens_total = model_bucket.input_tokens_total.saturating_add(in_u);
                model_bucket.output_tokens_total = model_bucket.output_tokens_total.saturating_add(out_u);
            }

            inner.dirty = true;
        }
        self.schedule_flush();
    }

    /// 清空指定账号统计（不存在则忽略）。
    pub fn reset_account(&self, id: u64) {
        if id == 0 {
            return;
        }
        {
            let mut inner = self.inner.write();
            inner.accounts.remove(&id);
            inner.dirty = true;
        }
        self.schedule_flush();
    }

    /// 清空所有账号统计。
    pub fn reset_all(&self) {
        {
            let mut inner = self.inner.write();
            inner.accounts.clear();
            inner.dirty = true;
        }
        self.schedule_flush();
    }

    /// 强制落盘一次（主要用于测试/手动维护）。
    pub async fn flush_now(&self) -> anyhow::Result<()> {
        let (path, json) = self.build_persist_payload()?;
        persist_to_path(&path, &json).await
    }

    fn schedule_flush(&self) {
        let _ = self.flush_tx.send(());
    }

    fn spawn_flush_task(store: Arc<Self>, mut flush_rx: mpsc::UnboundedReceiver<()>) {
        task::spawn(async move {
            // debounce：合并短时间内的多次写入
            let debounce = Duration::from_millis(800);
            let mut pending_timer: Option<Pin<Box<Sleep>>> = None;

            loop {
                tokio::select! {
                    msg = flush_rx.recv() => {
                        if msg.is_none() {
                            break;
                        }
                        pending_timer = Some(Box::pin(tokio::time::sleep(debounce)));
                    }
                    _ = async {
                        if let Some(t) = &mut pending_timer {
                            t.as_mut().await;
                        }
                    }, if pending_timer.is_some() => {
                        pending_timer = None;
                        if let Err(e) = store.flush_if_dirty().await {
                            tracing::warn!("统计落盘失败（下次继续重试）: {}", e);
                        }
                    }
                }
            }

            // channel 关闭时，尽量再落盘一次
            if let Err(e) = store.flush_if_dirty().await {
                tracing::warn!("统计落盘失败（退出前最后一次尝试）: {}", e);
            }
        });
    }

    async fn flush_if_dirty(&self) -> anyhow::Result<()> {
        let dirty = {
            let inner = self.inner.read();
            inner.dirty
        };
        if !dirty {
            return Ok(());
        }

        let (path, json) = self.build_persist_payload()?;
        persist_to_path(&path, &json).await?;

        // 写入成功后清理 dirty
        {
            let mut inner = self.inner.write();
            inner.dirty = false;
        }
        Ok(())
    }

    fn build_persist_payload(&self) -> anyhow::Result<(PathBuf, String)> {
        let mut accounts: Vec<AccountStats> = {
            let inner = self.inner.read();
            inner.accounts.values().cloned().collect()
        };
        accounts.sort_by_key(|s| s.id);

        let file = StatsFile {
            version: default_stats_version(),
            accounts,
        };

        let json = serde_json::to_string_pretty(&file).context("序列化统计失败")?;
        Ok((self.path.clone(), json))
    }
}

fn tokens_to_u64(v: i64) -> u64 {
    if v <= 0 {
        0
    } else {
        v as u64
    }
}

fn load_from_path(path: &Path) -> anyhow::Result<HashMap<u64, AccountStats>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("读取统计文件失败: {:?}", path))?;

    if content.trim().is_empty() {
        return Ok(HashMap::new());
    }

    // 支持旧版本字段：新增字段都带 #[serde(default)]，因此能向后兼容。
    let parsed: StatsFile = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            // 保护现场：把坏文件改名，避免服务启动失败。
            let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
            let backup = path
                .with_file_name(format!("{}.corrupt.{}", file_stem(path), ts));
            if let Err(re) = fs::rename(path, &backup) {
                tracing::warn!(
                    "统计文件解析失败且备份失败（将忽略并从空统计开始）: parse={}, rename={}",
                    e,
                    re
                );
            } else {
                tracing::warn!(
                    "统计文件解析失败，已备份到 {:?}（将从空统计开始）: {}",
                    backup,
                    e
                );
            }
            return Ok(HashMap::new());
        }
    };

    let mut map = HashMap::new();
    for mut acc in parsed.accounts {
        if acc.id == 0 {
            continue;
        }
        // 保证 id 字段一致
        let id = acc.id;
        acc.id = id;
        map.insert(id, acc);
    }

    Ok(map)
}

fn today_key() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

fn file_stem(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("credential-stats.json")
        .to_string()
}

async fn persist_to_path(path: &Path, json: &str) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    let json = json.to_string();

    task::spawn_blocking(move || persist_to_path_blocking(&path, &json))
        .await
        .context("统计落盘任务 join 失败")??;

    Ok(())
}

fn persist_to_path_blocking(path: &Path, json: &str) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        fs::create_dir_all(dir).with_context(|| format!("创建统计目录失败: {:?}", dir))?;
    }

    let tmp_path = path.with_extension("json.tmp");

    fs::write(&tmp_path, json)
        .with_context(|| format!("写入临时统计文件失败: {:?}", tmp_path))?;

    // Windows 上 rename 不能覆盖已存在文件：先尝试删除旧文件。
    if path.exists() {
        let _ = fs::remove_file(path);
    }

    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // 兜底：尝试直接写入目标（可能会导致非原子更新，但比完全失败好）
            tracing::warn!("统计文件 rename 失败，尝试直接写入目标: {}", e);
            fs::write(path, json).with_context(|| format!("直接写入统计文件失败: {:?}", path))?;
            let _ = fs::remove_file(&tmp_path);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokens_to_u64() {
        assert_eq!(tokens_to_u64(-1), 0);
        assert_eq!(tokens_to_u64(0), 0);
        assert_eq!(tokens_to_u64(1), 1);
    }

    #[test]
    fn test_record_attempt_success_error_usage() {
        // 不启动后台任务：测试只验证内存行为。
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kiro_stats_test_{}.json", fastrand::u64(..)));
        let store = match StatsStore::load_or_new(path) {
            Ok(v) => v,
            Err(e) => panic!("{:?}", e),
        };

        store.record_attempt(1, Some("m1"));
        // attempt 不计入 calls_total
        let s0 = store.get_or_default(1);
        assert_eq!(s0.calls_total, 0);
        assert!(s0.last_call_at.is_some());

        store.record_success(1, Some("m1"));
        store.add_usage(1, Some("m1"), 10, 20);

        let s = store.get_or_default(1);
        assert_eq!(s.calls_total, 1);
        assert_eq!(s.calls_ok, 1);
        assert_eq!(s.calls_err, 0);
        assert_eq!(s.input_tokens_total, 10);
        assert_eq!(s.output_tokens_total, 20);
        assert!(s.last_call_at.is_some());
        assert!(s.last_success_at.is_some());
        assert!(s.last_error.is_none());

        assert_eq!(s.by_model.get("m1").map(|b| b.calls_total), Some(1));
        assert_eq!(s.by_model.get("m1").map(|b| b.calls_ok), Some(1));
        assert_eq!(s.by_model.get("m1").map(|b| b.input_tokens_total), Some(10));
        assert_eq!(s.by_model.get("m1").map(|b| b.output_tokens_total), Some(20));
        assert!(!s.by_day.is_empty());

        store.record_error(1, Some("m1"), "boom");
        let s2 = store.get_or_default(1);
        assert_eq!(s2.calls_total, 2);
        assert_eq!(s2.calls_err, 1);
        assert!(s2.last_error.is_some());
        assert!(s2.last_error_at.is_some());
        assert_eq!(s2.by_model.get("m1").map(|b| b.calls_err), Some(1));

        // reset account
        store.reset_account(1);
        let s3 = store.get_or_default(1);
        assert_eq!(s3.calls_total, 0);
        assert!(s3.by_model.is_empty());
        assert!(s3.by_day.is_empty());

        // reset all
        store.record_attempt(2, None);
        store.reset_all();
        let s4 = store.get_or_default(2);
        assert_eq!(s4.calls_total, 0);
    }
}
