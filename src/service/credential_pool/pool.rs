//! CredentialPool：组合 store + state + stats + selector + refresher 的门面
//!
//! ## 锁顺序约束
//!
//! 内部锁顺序固定为 `store -> state -> stats`，禁止反向获取，避免死锁。
//!
//! ## acquire 视图组装
//!
//! 1. 在三把锁持有期内 `snapshot()`（克隆数据，立即释放锁）
//! 2. 按 id join 拼装 [`CredentialView`]（不假设 Vec 索引对齐）
//! 3. 过滤 `disabled == false`
//! 4. 调 `selector.select(&views, model)` 返回 `Option<u64>`
//! 5. 释放所有锁后再做 token 刷新 / I/O（**禁止跨 .await 持锁**）

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;

use crate::config::Config;
use crate::domain::credential::Credential;
use crate::domain::error::{ProviderError, RefreshError};
use crate::domain::retry::DisabledReason;
use crate::domain::selector::{
    CredentialSelector, CredentialStateView, CredentialStatsView, CredentialView,
};
use crate::domain::token::TokenSource;
use crate::infra::machine_id::MachineIdResolver;
use crate::infra::refresher::{ApiKeyRefresher, IdcRefresher, SocialRefresher};
use crate::infra::selector::{BalancedSelector, PrioritySelector};
use crate::infra::storage::{StatsEntry, StatsFileStore};

use super::state::{CredentialState, EntryState};
use super::stats::{CredentialStats, EntryStats};
use super::store::CredentialStore;

pub const MODE_PRIORITY: &str = "priority";
pub const MODE_BALANCED: &str = "balanced";

/// 调用上下文（acquire 的返回值）
#[derive(Debug, Clone)]
pub struct CallContext {
    pub id: u64,
    pub credentials: Credential,
    pub token: String,
    pub machine_id: String,
}

pub struct CredentialPool {
    store: Arc<CredentialStore>,
    state: Arc<CredentialState>,
    stats: Arc<CredentialStats>,
    stats_store: Option<Arc<StatsFileStore>>,
    config: Arc<Config>,
    resolver: Arc<MachineIdResolver>,
    refresher_social: Arc<SocialRefresher>,
    refresher_idc: Arc<IdcRefresher>,
    refresher_api_key: Arc<ApiKeyRefresher>,
    load_balancing_mode: Mutex<String>,
    current_id: Mutex<Option<u64>>,
}

impl CredentialPool {
    /// 构造
    ///
    /// `stats_store` 为 None 时仅内存维护统计（不持久化）。
    pub fn new(
        store: Arc<CredentialStore>,
        state: Arc<CredentialState>,
        stats: Arc<CredentialStats>,
        stats_store: Option<Arc<StatsFileStore>>,
        config: Arc<Config>,
        resolver: Arc<MachineIdResolver>,
    ) -> Self {
        let mode = config.features.load_balancing_mode.clone();
        let refresher_social = Arc::new(SocialRefresher::new(config.clone(), resolver.clone()));
        let refresher_idc = Arc::new(IdcRefresher::new(config.clone(), resolver.clone()));
        let refresher_api_key = Arc::new(ApiKeyRefresher::new());
        Self {
            store,
            state,
            stats,
            stats_store,
            config,
            resolver,
            refresher_social,
            refresher_idc,
            refresher_api_key,
            load_balancing_mode: Mutex::new(mode),
            current_id: Mutex::new(None),
        }
    }

    pub fn store(&self) -> &CredentialStore {
        &self.store
    }

    pub fn state(&self) -> &CredentialState {
        &self.state
    }

    pub fn stats(&self) -> &CredentialStats {
        &self.stats
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn resolver(&self) -> &MachineIdResolver {
        &self.resolver
    }

    pub fn total_count(&self) -> usize {
        self.store.count()
    }

    pub fn available_count(&self) -> usize {
        let snap = self.state.snapshot();
        snap.values().filter(|s| !s.disabled).count()
    }

    pub fn get_load_balancing_mode(&self) -> String {
        self.load_balancing_mode.lock().clone()
    }

    /// 切换负载均衡模式（仅接受 "priority" / "balanced"，其他保留旧值）
    pub fn set_load_balancing_mode(&self, mode: &str) -> Result<(), ProviderError> {
        let normalized = match mode {
            MODE_PRIORITY | MODE_BALANCED => mode.to_string(),
            other => {
                return Err(ProviderError::BadRequest(format!(
                    "unknown load balancing mode: {other}"
                )));
            }
        };
        *self.load_balancing_mode.lock() = normalized;
        Ok(())
    }

    /// 选凭据 + 准备 token（含必要时刷新）
    ///
    /// 内部循环：单次 selector → 单次 prepare_token；prepare 失败则把该凭据标记 disabled
    /// 后回 loop 重选；selector 返回 None 时尝试自愈一次，仍 None 即 exhausted。
    pub async fn acquire(&self, model: Option<&str>) -> Result<CallContext, ProviderError> {
        let total = self.total_count();
        let mut healed_once = false;

        loop {
            let selected = match self.select_one(model) {
                Some(id) => id,
                None => {
                    if !healed_once && self.state.heal_too_many_failures() {
                        healed_once = true;
                        continue;
                    }
                    return Err(ProviderError::AllCredentialsExhausted {
                        available: self.available_count(),
                        total,
                    });
                }
            };

            let cred = match self.store.get(selected) {
                Some(c) => c,
                None => {
                    self.state.remove(selected);
                    continue;
                }
            };

            match self.prepare_token(selected, &cred).await {
                Ok((token, fresh_cred)) => {
                    let machine_id = self.resolver.resolve(&fresh_cred, &self.config);
                    if self.get_load_balancing_mode() != MODE_BALANCED {
                        *self.current_id.lock() = Some(selected);
                    }
                    return Ok(CallContext {
                        id: selected,
                        credentials: fresh_cred,
                        token,
                        machine_id,
                    });
                }
                Err(refresh_err) => {
                    tracing::warn!(id = selected, ?refresh_err, "凭据 token 准备失败，回退到下一条");
                    match refresh_err {
                        RefreshError::TokenInvalid => {
                            self.state.report_refresh_token_invalid(selected);
                        }
                        _ => {
                            self.state.report_refresh_failure(selected);
                        }
                    }
                    continue;
                }
            }
        }
    }

    /// 选凭据：拼 view → 调 selector
    ///
    /// priority 模式下，若 current_id 仍 enabled 且 model 兼容，则直接复用 current_id。
    fn select_one(&self, model: Option<&str>) -> Option<u64> {
        let mode = self.get_load_balancing_mode();

        let store_map = self.store.snapshot();
        let state_map = self.state.snapshot();
        let stats_map = self.stats.snapshot();

        let needs_opus = model
            .map(|m| m.to_lowercase().contains("opus"))
            .unwrap_or(false);

        // priority 模式 current_id fast path
        if mode != MODE_BALANCED {
            if let Some(current) = *self.current_id.lock() {
                if let Some(cred) = store_map.get(&current) {
                    let enabled = state_map.get(&current).map(|s| !s.disabled).unwrap_or(true);
                    let opus_ok = !needs_opus || cred.supports_opus();
                    if enabled && opus_ok {
                        return Some(current);
                    }
                }
            }
        }

        // 拼 view
        let state_views: HashMap<u64, CredentialStateView> = store_map
            .keys()
            .map(|id| {
                let disabled = state_map.get(id).map(|s| s.disabled).unwrap_or(false);
                (*id, CredentialStateView { disabled })
            })
            .collect();
        let stats_views: HashMap<u64, CredentialStatsView> = store_map
            .keys()
            .map(|id| {
                let success_count = stats_map.get(id).map(|s| s.success_count).unwrap_or(0);
                (*id, CredentialStatsView { success_count })
            })
            .collect();

        let views: Vec<CredentialView<'_>> = store_map
            .iter()
            .filter_map(|(id, cred)| {
                let state = state_views.get(id)?;
                if state.disabled {
                    return None;
                }
                let stats = stats_views.get(id)?;
                Some(CredentialView {
                    id: *id,
                    credential: cred,
                    state,
                    stats,
                })
            })
            .collect();

        let selected = if mode == MODE_BALANCED {
            BalancedSelector::new().select(&views, model)
        } else {
            PrioritySelector::new().select(&views, model)
        };

        if mode != MODE_BALANCED {
            if let Some(id) = selected {
                *self.current_id.lock() = Some(id);
            }
        }

        selected
    }

    /// 准备 token：未过期直接用 access_token；过期则触发 refresh
    ///
    /// API Key 凭据走 ApiKeyRefresher passthrough。
    async fn prepare_token(
        &self,
        id: u64,
        cred: &Credential,
    ) -> Result<(String, Credential), RefreshError> {
        if cred.is_api_key_credential() {
            let outcome = self.refresher_api_key.refresh(cred).await?;
            return Ok((outcome.access_token, cred.clone()));
        }

        if let Some(token) = cred.access_token.clone() {
            if !is_token_expired(cred) {
                return Ok((token, cred.clone()));
            }
        }

        // 触发 refresh
        let refresher_choice = pick_refresher_kind(cred);
        let outcome = match refresher_choice {
            RefresherKind::Idc => self.refresher_idc.refresh(cred).await,
            RefresherKind::Social => self.refresher_social.refresh(cred).await,
        }?;

        // 写回 store
        let mut updated = cred.clone();
        updated.access_token = Some(outcome.access_token.clone());
        if let Some(rt) = outcome.refresh_token {
            updated.refresh_token = Some(rt);
        }
        if let Some(arn) = outcome.profile_arn {
            updated.profile_arn = Some(arn);
        }
        if let Some(ea) = outcome.expires_at {
            updated.expires_at = Some(ea);
        }
        let _ = self.store.replace(id, updated.clone());
        Ok((outcome.access_token, updated))
    }

    pub fn report_success(&self, id: u64) {
        self.state.report_success(id);
        self.stats.record_use(id);
        self.maybe_persist_stats(id);
    }

    /// 报告失败；返回 true 表示该凭据已被禁用
    pub fn report_failure(&self, id: u64) -> bool {
        self.state.report_failure(id)
    }

    pub fn report_quota_exhausted(&self, id: u64) -> bool {
        self.state.report_quota_exhausted(id)
    }

    pub fn report_refresh_failure(&self, id: u64) -> bool {
        self.state.report_refresh_failure(id)
    }

    pub fn report_refresh_token_invalid(&self, id: u64) -> bool {
        self.state.report_refresh_token_invalid(id)
    }

    fn maybe_persist_stats(&self, _id: u64) {
        // Phase 2: 简化为每次落盘（无 debounce）；Phase 7 接入完整 debounce
        if let Some(store) = &self.stats_store {
            let map: HashMap<u64, StatsEntry> = self.stats.to_storage_map();
            let _ = store.save(&map);
        }
    }

    /// 装载阶段使用：把 store 的所有 id 在 state 里建一条空 EntryState；issues 中的 id 同时设 InvalidConfig
    pub fn install_initial_states(
        &self,
        invalid_config_ids: &HashSet<u64>,
        initial_disabled_ids: &HashSet<u64>,
    ) {
        for id in self.store.ids() {
            let entry = if invalid_config_ids.contains(&id) {
                EntryState::disabled_with(DisabledReason::InvalidConfig)
            } else if initial_disabled_ids.contains(&id) {
                EntryState {
                    disabled: true,
                    disabled_reason: None,
                    ..Default::default()
                }
            } else {
                EntryState::default()
            };
            self.state.upsert(id, entry);
        }
    }

    /// 装载阶段使用：把 stats_store 加载的统计回填到 stats
    pub fn install_initial_stats(&self, loaded: HashMap<u64, EntryStats>) {
        for (id, stats) in loaded {
            self.stats.upsert(id, stats);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RefresherKind {
    Social,
    Idc,
}

fn pick_refresher_kind(cred: &Credential) -> RefresherKind {
    let auth_method = cred.auth_method.as_deref().unwrap_or_else(|| {
        if cred.client_id.is_some() && cred.client_secret.is_some() {
            "idc"
        } else {
            "social"
        }
    });
    if auth_method.eq_ignore_ascii_case("idc")
        || auth_method.eq_ignore_ascii_case("builder-id")
        || auth_method.eq_ignore_ascii_case("iam")
    {
        RefresherKind::Idc
    } else {
        RefresherKind::Social
    }
}

/// 判断 token 是否在 5 分钟内过期（含已过期）
fn is_token_expired(cred: &Credential) -> bool {
    let Some(expires_at) = &cred.expires_at else {
        return true;
    };
    let Ok(expires) = DateTime::parse_from_rfc3339(expires_at) else {
        return true;
    };
    expires <= Utc::now() + Duration::minutes(5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::storage::CredentialsFileStore;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-pool-test-{tag}-{id}.json"))
    }

    fn far_future_expires_at() -> String {
        (Utc::now() + Duration::days(7)).to_rfc3339()
    }

    /// 构造一个含 N 条 social 凭据的 pool；每条都已带 access_token + 远期 expires_at。
    fn pool_with_n_credentials(n: usize, mode: &str) -> (CredentialPool, PathBuf) {
        let path = tmp_path("pool");
        let mut creds_json = Vec::new();
        for i in 0..n {
            creds_json.push(serde_json::json!({
                "refreshToken": format!("rt-{i}"),
                "accessToken": format!("at-{i}"),
                "expiresAt": far_future_expires_at(),
                "authMethod": "social",
                "priority": i,
            }));
        }
        let arr = serde_json::Value::Array(creds_json);
        fs::write(&path, serde_json::to_string_pretty(&arr).unwrap()).unwrap();

        let file = Arc::new(CredentialsFileStore::new(Some(path.clone())));
        let mut config = Config::default();
        config.features.load_balancing_mode = mode.to_string();
        let config = Arc::new(config);
        let resolver = Arc::new(MachineIdResolver::new());
        let (store, _issues) =
            CredentialStore::load(file, config.clone(), resolver.clone()).unwrap();
        let store = Arc::new(store);
        let state = Arc::new(CredentialState::new());
        let stats = Arc::new(CredentialStats::new());
        let pool = CredentialPool::new(
            store,
            state,
            stats,
            None,
            config,
            resolver,
        );
        let invalid: HashSet<u64> = HashSet::new();
        let initial_disabled: HashSet<u64> = HashSet::new();
        pool.install_initial_states(&invalid, &initial_disabled);
        (pool, path)
    }

    #[tokio::test]
    async fn acquire_returns_single_credential_when_one_available() {
        let (pool, path) = pool_with_n_credentials(1, MODE_PRIORITY);
        let ctx = pool.acquire(None).await.unwrap();
        assert!(ctx.token.starts_with("at-"));
        assert!(!ctx.machine_id.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn acquire_returns_exhausted_when_all_disabled() {
        let (pool, path) = pool_with_n_credentials(2, MODE_PRIORITY);
        for id in pool.store.ids() {
            // 用 QuotaExceeded 禁用（不会自愈）
            pool.report_quota_exhausted(id);
        }
        let err = pool.acquire(None).await.unwrap_err();
        match err {
            ProviderError::AllCredentialsExhausted { available, total } => {
                assert_eq!(available, 0);
                assert_eq!(total, 2);
            }
            other => panic!("期望 AllCredentialsExhausted，得到 {other:?}"),
        }
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn balanced_mode_distributes_across_two_credentials() {
        let (pool, path) = pool_with_n_credentials(2, MODE_BALANCED);
        let mut counts: HashMap<u64, u64> = HashMap::new();
        for _ in 0..6 {
            let ctx = pool.acquire(None).await.unwrap();
            *counts.entry(ctx.id).or_insert(0) += 1;
            // 触发 success_count++
            pool.report_success(ctx.id);
        }
        assert_eq!(counts.len(), 2, "balanced 应在 2 条凭据间均匀分布");
        let v: Vec<u64> = counts.values().copied().collect();
        let max = *v.iter().max().unwrap();
        let min = *v.iter().min().unwrap();
        assert!(max - min <= 1, "差距不应超过 1：{counts:?}");
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn priority_mode_sticks_to_current_id_when_still_enabled() {
        let (pool, path) = pool_with_n_credentials(2, MODE_PRIORITY);
        let ctx1 = pool.acquire(None).await.unwrap();
        let ctx2 = pool.acquire(None).await.unwrap();
        let ctx3 = pool.acquire(None).await.unwrap();
        assert_eq!(ctx1.id, ctx2.id, "priority 应固定在 current_id");
        assert_eq!(ctx2.id, ctx3.id);
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn priority_falls_back_when_current_disabled() {
        let (pool, path) = pool_with_n_credentials(2, MODE_PRIORITY);
        let ctx1 = pool.acquire(None).await.unwrap();
        pool.report_quota_exhausted(ctx1.id);
        let ctx2 = pool.acquire(None).await.unwrap();
        assert_ne!(ctx1.id, ctx2.id, "禁用 current 后应切到下一条");
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn report_quota_then_acquire_switches_to_next() {
        let (pool, path) = pool_with_n_credentials(3, MODE_PRIORITY);
        let ctx1 = pool.acquire(None).await.unwrap();
        pool.report_quota_exhausted(ctx1.id);
        let ctx2 = pool.acquire(None).await.unwrap();
        pool.report_quota_exhausted(ctx2.id);
        let ctx3 = pool.acquire(None).await.unwrap();
        assert_ne!(ctx1.id, ctx2.id);
        assert_ne!(ctx2.id, ctx3.id);
        assert_ne!(ctx1.id, ctx3.id);
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn heal_too_many_failures_unblocks_acquire() {
        let (pool, path) = pool_with_n_credentials(2, MODE_PRIORITY);
        // 每条 report_failure 3 次 → 全部 TooManyFailures
        for id in pool.store.ids() {
            for _ in 0..3 {
                pool.report_failure(id);
            }
        }
        // 此时没有 enabled 凭据 → acquire 触发自愈一次后应成功
        let ctx = pool.acquire(None).await.unwrap();
        assert!(!ctx.token.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn set_load_balancing_mode_validates_input() {
        let (pool, path) = pool_with_n_credentials(1, MODE_PRIORITY);
        assert!(pool.set_load_balancing_mode("balanced").is_ok());
        assert_eq!(pool.get_load_balancing_mode(), "balanced");
        assert!(pool.set_load_balancing_mode("priority").is_ok());
        assert!(pool.set_load_balancing_mode("invalid_mode").is_err());
        // 失败时保留旧值
        assert_eq!(pool.get_load_balancing_mode(), "priority");
        let _ = fs::remove_file(&path);
    }
}
