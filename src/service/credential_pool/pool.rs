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
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::domain::credential::Credential;
use crate::domain::error::{ProviderError, RefreshError};
use crate::domain::retry::DisabledReason;
use crate::domain::selector::{
    CredentialSelector, CredentialStateView, CredentialStatsView, CredentialView,
};
use crate::domain::token::DynTokenSource;
use crate::domain::usage::UsageLimitsResponse;
use crate::infra::http::client::{ProxyConfig, build_client};
use crate::infra::machine_id::MachineIdResolver;
use crate::infra::refresher::{ApiKeyRefresher, IdcRefresher, SocialRefresher};
use crate::infra::selector::{BalancedSelector, PrioritySelector};
use crate::infra::storage::{StatsEntry, StatsFileStore};

use super::admin::{AdminEntrySnapshot, AdminPoolError, AdminSnapshot};
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
    refresher_social: Arc<dyn DynTokenSource>,
    refresher_idc: Arc<dyn DynTokenSource>,
    refresher_api_key: Arc<dyn DynTokenSource>,
    load_balancing_mode: Mutex<String>,
    current_id: Mutex<Option<u64>>,
    /// 按凭据 id 维护的 refresh 单点串行锁；avoid 同一凭据并发 refresh 浪费 refresh_token
    refresh_locks: Mutex<HashMap<u64, Arc<tokio::sync::Mutex<()>>>>,
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
        let refresher_social: Arc<dyn DynTokenSource> =
            Arc::new(SocialRefresher::new(config.clone(), resolver.clone()));
        let refresher_idc: Arc<dyn DynTokenSource> =
            Arc::new(IdcRefresher::new(config.clone(), resolver.clone()));
        let refresher_api_key: Arc<dyn DynTokenSource> = Arc::new(ApiKeyRefresher::new());
        Self::new_with_refreshers(
            store,
            state,
            stats,
            stats_store,
            config,
            resolver,
            refresher_social,
            refresher_idc,
            refresher_api_key,
        )
    }

    /// 构造（注入 refresher）；测试可替换为 mock。
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_refreshers(
        store: Arc<CredentialStore>,
        state: Arc<CredentialState>,
        stats: Arc<CredentialStats>,
        stats_store: Option<Arc<StatsFileStore>>,
        config: Arc<Config>,
        resolver: Arc<MachineIdResolver>,
        refresher_social: Arc<dyn DynTokenSource>,
        refresher_idc: Arc<dyn DynTokenSource>,
        refresher_api_key: Arc<dyn DynTokenSource>,
    ) -> Self {
        let mode = config.features.load_balancing_mode.clone();
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
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    /// 取得（或惰性创建）指定 id 的 refresh 串行锁
    fn refresh_guard_for(&self, id: u64) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.refresh_locks.lock();
        locks
            .entry(id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
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
    ///
    /// 内存生效后回写 config.json；持久化失败会回滚内存值。单次加锁完成"先读后写"，
    /// 避免双 lock() 之间的竞争窗口。
    pub fn set_load_balancing_mode(&self, mode: &str) -> Result<(), ProviderError> {
        let normalized = match mode {
            MODE_PRIORITY | MODE_BALANCED => mode.to_string(),
            other => {
                return Err(ProviderError::BadRequest(format!(
                    "unknown load balancing mode: {other}"
                )));
            }
        };

        let previous = {
            let mut guard = self.load_balancing_mode.lock();
            if *guard == normalized {
                return Ok(());
            }
            std::mem::replace(&mut *guard, normalized.clone())
        };

        if let Err(e) = self.persist_load_balancing_mode(&normalized) {
            *self.load_balancing_mode.lock() = previous;
            return Err(ProviderError::BadRequest(format!(
                "持久化负载均衡模式失败: {e}"
            )));
        }
        Ok(())
    }

    fn persist_load_balancing_mode(
        &self,
        mode: &str,
    ) -> Result<(), crate::domain::error::ConfigError> {
        let path = match self.config.config_path() {
            Some(p) => p.to_path_buf(),
            None => {
                tracing::warn!("配置文件路径未知，负载均衡模式仅运行时生效: {}", mode);
                return Ok(());
            }
        };
        let mut cfg = Config::load(&path)?;
        cfg.features.load_balancing_mode = mode.to_string();
        cfg.save()
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
        if mode != MODE_BALANCED
            && let Some(current) = *self.current_id.lock()
                && let Some(cred) = store_map.get(&current) {
                    let enabled = state_map.get(&current).map(|s| !s.disabled).unwrap_or(true);
                    let opus_ok = !needs_opus || cred.supports_opus();
                    if enabled && opus_ok {
                        return Some(current);
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

        if mode != MODE_BALANCED
            && let Some(id) = selected {
                *self.current_id.lock() = Some(id);
            }

        selected
    }

    /// 准备 token：未过期直接用 access_token；过期则触发 refresh
    ///
    /// API Key 凭据走 ApiKeyRefresher passthrough。
    /// 同一凭据的并发 refresh 通过 [`refresh_guard_for`] 串行化，
    /// 持锁后二次检查 store 中的最新 token，避免重复刷新。
    async fn prepare_token(
        &self,
        id: u64,
        cred: &Credential,
    ) -> Result<(String, Credential), RefreshError> {
        if cred.is_api_key_credential() {
            let outcome = self.refresher_api_key.refresh(cred).await?;
            return Ok((outcome.access_token, cred.clone()));
        }

        if let Some(token) = cred.access_token.clone()
            && !is_token_expired(cred)
        {
            return Ok((token, cred.clone()));
        }

        // single-flight：同一 id 的并发 refresh 串行化
        let guard = self.refresh_guard_for(id);
        let _lock = guard.lock().await;

        // 二次检查：拿到锁后重读 store，可能并发的前一个任务已刷新
        let fresh = self.store.get(id).unwrap_or_else(|| cred.clone());
        if let Some(token) = fresh.access_token.clone()
            && !is_token_expired(&fresh)
        {
            return Ok((token, fresh));
        }

        // 触发 refresh
        let refresher_choice = pick_refresher_kind(&fresh);
        let outcome = match refresher_choice {
            RefresherKind::Idc => self.refresher_idc.refresh(&fresh).await,
            RefresherKind::Social => self.refresher_social.refresh(&fresh).await,
        }?;

        // 写回 store；持久化失败仅 log，请求路径不因磁盘抖动失败
        let mut updated = fresh;
        updated.apply_refresh(&outcome);
        match self.store.replace(id, updated.clone()) {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(id, "刷新成功但凭据已被删除，token 仅本次请求有效");
            }
            Err(e) => {
                tracing::error!(?e, id, "刷新成功但持久化失败，凭据已更新到内存");
            }
        }
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

    /// 强制刷新 token：调对应的 refresher，更新 store 中的凭据
    ///
    /// API Key 凭据视为 no-op（无需刷新）；其他凭据按 auth_method 分发到 social / idc。
    /// 与 [`prepare_token`] 共享 single-flight 锁，避免与并发 acquire 重复刷新。
    ///
    /// 调用语义：上游返回 401/403 触发，server 已吊销当前 token。即便 access_token 在
    /// 客户端时钟上未过期，也必须刷新——除非二次检查发现 store 中的 token 值已被
    /// 并发 refresh 替换（说明上一个并发任务已为我们刷新好），方可跳过。
    pub async fn force_refresh(&self, id: u64) -> Result<(), RefreshError> {
        let cred = self.store.get(id).ok_or(RefreshError::Unauthorized)?;
        if cred.is_api_key_credential() {
            return Ok(());
        }
        let token_before = cred.access_token.clone();
        let guard = self.refresh_guard_for(id);
        let _lock = guard.lock().await;
        // 二次检查：仅当 token 值相对入锁前发生变化（即并发 refresh 已成功写回）时才跳过；
        // 否则即便未到期也必须刷新（caller 因 401/403 触发，token 已被服务端拒绝）
        let fresh = self.store.get(id).unwrap_or(cred);
        if fresh.access_token != token_before
            && fresh.access_token.is_some()
            && !is_token_expired(&fresh)
        {
            return Ok(());
        }
        let outcome = match pick_refresher_kind(&fresh) {
            RefresherKind::Idc => self.refresher_idc.refresh(&fresh).await,
            RefresherKind::Social => self.refresher_social.refresh(&fresh).await,
        }?;
        let mut updated = fresh;
        updated.apply_refresh(&outcome);
        match self.store.replace(id, updated) {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(id, "刷新成功但凭据已被删除，token 仅本次请求有效");
            }
            Err(e) => {
                tracing::error!(?e, id, "刷新成功但持久化失败，凭据已更新到内存");
            }
        }
        Ok(())
    }

    /// 装载阶段使用：把 store 的所有 id 在 state 里建一条空 EntryState；issues 中的 id 同时设 InvalidConfig
    ///
    /// priority 模式下完成后会立即把 current_id 选到最低 priority 的可用凭据，
    /// 避免首请求前 current_id 为 None 导致 admin_snapshot 显示 0。
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
        self.select_highest_priority();
    }

    /// 装载阶段使用：把 stats_store 加载的统计回填到 stats
    pub fn install_initial_stats(&self, loaded: HashMap<u64, EntryStats>) {
        for (id, stats) in loaded {
            self.stats.upsert(id, stats);
        }
    }
}

// ============================================================================
// Admin API 扩展（取代旧 MultiTokenManager 的 admin 方法）
// ============================================================================

impl CredentialPool {
    /// Admin UI 数据视图：组合 store/state/stats 三层快照
    ///
    /// 锁顺序：store → state → stats，与 acquire 路径保持一致。
    pub fn admin_snapshot(&self) -> AdminSnapshot {
        let store_map = self.store.snapshot();
        let state_map = self.state.snapshot();
        let stats_map = self.stats.snapshot();
        let current_id = self.current_id.lock().unwrap_or(0);

        let total = store_map.len();
        let available = store_map
            .keys()
            .filter(|id| !state_map.get(id).map(|s| s.disabled).unwrap_or(false))
            .count();

        let entries: Vec<AdminEntrySnapshot> = store_map
            .iter()
            .map(|(id, cred)| {
                let state = state_map.get(id).cloned().unwrap_or_default();
                let stats = stats_map.get(id).cloned().unwrap_or_default();
                let is_api_key = cred.is_api_key_credential();
                AdminEntrySnapshot {
                    id: *id,
                    priority: cred.priority,
                    disabled: state.disabled,
                    failure_count: state.failure_count,
                    auth_method: if is_api_key {
                        Some("api_key".to_string())
                    } else {
                        cred.auth_method
                            .as_deref()
                            .map(canonicalize_admin_auth_method)
                    },
                    has_profile_arn: cred.profile_arn.is_some(),
                    expires_at: if is_api_key { None } else { cred.expires_at.clone() },
                    refresh_token_hash: if is_api_key {
                        None
                    } else {
                        cred.refresh_token.as_deref().map(sha256_hex)
                    },
                    api_key_hash: if is_api_key {
                        cred.kiro_api_key.as_deref().map(sha256_hex)
                    } else {
                        None
                    },
                    masked_api_key: if is_api_key {
                        cred.kiro_api_key.as_deref().map(mask_api_key)
                    } else {
                        None
                    },
                    email: cred.email.clone(),
                    success_count: stats.success_count,
                    last_used_at: stats.last_used_at.clone(),
                    has_proxy: cred.proxy_url.is_some(),
                    proxy_url: cred.proxy_url.as_deref().map(mask_proxy_url),
                    refresh_failure_count: state.refresh_failure_count,
                    disabled_reason: state
                        .disabled_reason
                        .map(|r| disabled_reason_to_str(r).to_string()),
                    endpoint: cred.endpoint.clone(),
                }
            })
            .collect();

        AdminSnapshot {
            entries,
            current_id,
            total,
            available,
        }
    }

    /// 设置凭据禁用状态（同步到 store + state，state 层会清失败计数）
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<(), AdminPoolError> {
        let exists = self.store.set_disabled(id, disabled)?;
        if !exists {
            return Err(AdminPoolError::NotFound(id));
        }
        self.state.set_disabled(id, disabled);
        Ok(())
    }

    /// 修改凭据优先级（仅 priority 模式生效；balanced 模式仅持久化）
    pub fn set_priority(&self, id: u64, priority: u32) -> Result<(), AdminPoolError> {
        let exists = self.store.set_priority(id, priority)?;
        if !exists {
            return Err(AdminPoolError::NotFound(id));
        }
        self.select_highest_priority();
        Ok(())
    }

    /// 重置失败计数并启用（InvalidConfig 凭据需先修复配置后重启）
    pub fn reset_and_enable(&self, id: u64) -> Result<(), AdminPoolError> {
        if let Some(s) = self.state.get(id)
            && s.disabled_reason == Some(DisabledReason::InvalidConfig) {
                return Err(AdminPoolError::DisabledByInvalidConfig(id));
            }
        if self.store.get(id).is_none() {
            return Err(AdminPoolError::NotFound(id));
        }
        let _ = self.store.set_disabled(id, false)?;
        self.state.set_disabled(id, false);
        Ok(())
    }

    /// 切换到下一可用凭据（priority 模式专用，禁用当前后调用）
    ///
    /// 返回 true：成功切换或当前仍可用；false：balanced 模式或全部禁用
    pub fn switch_to_next(&self) -> bool {
        if self.get_load_balancing_mode() == MODE_BALANCED {
            return false;
        }
        let store_map = self.store.snapshot();
        let state_map = self.state.snapshot();
        let current = *self.current_id.lock();

        let next = store_map
            .iter()
            .filter(|(id, _)| {
                Some(**id) != current
                    && !state_map.get(id).map(|s| s.disabled).unwrap_or(false)
            })
            .min_by_key(|(_, c)| c.priority);

        if let Some((next_id, cred)) = next {
            tracing::info!(
                "已切换到凭据 #{}（优先级 {}）",
                next_id,
                cred.priority
            );
            *self.current_id.lock() = Some(*next_id);
            true
        } else if let Some(cur) = current {
            !state_map.get(&cur).map(|s| s.disabled).unwrap_or(false)
        } else {
            false
        }
    }

    /// 重选 current_id 为最低 priority 的可用凭据（priority 模式生效）
    fn select_highest_priority(&self) {
        if self.get_load_balancing_mode() == MODE_BALANCED {
            return;
        }
        let store_map = self.store.snapshot();
        let state_map = self.state.snapshot();
        let best = store_map
            .iter()
            .filter(|(id, _)| !state_map.get(id).map(|s| s.disabled).unwrap_or(false))
            .min_by_key(|(_, c)| c.priority);

        if let Some((new_id, cred)) = best {
            let mut current = self.current_id.lock();
            if Some(*new_id) != *current {
                tracing::info!(
                    "优先级变更后切换凭据: {:?} -> #{}（优先级 {}）",
                    *current,
                    new_id,
                    cred.priority
                );
                *current = Some(*new_id);
            }
        }
    }

    /// 添加新凭据（验证 + 哈希去重 + 实际刷新 + 持久化）
    pub async fn add_credential(
        &self,
        mut new_cred: Credential,
    ) -> Result<u64, AdminPoolError> {
        // 1. 基本字段校验
        new_cred.canonicalize_auth_method();
        if new_cred.is_api_key_credential() {
            let api_key = new_cred
                .kiro_api_key
                .as_deref()
                .ok_or(AdminPoolError::MissingApiKey)?;
            if api_key.is_empty() {
                return Err(AdminPoolError::EmptyApiKey);
            }
        } else {
            let rt = new_cred
                .refresh_token
                .as_deref()
                .ok_or(AdminPoolError::MissingRefreshToken)?;
            if rt.is_empty() {
                return Err(AdminPoolError::EmptyRefreshToken);
            }
            if rt.len() < 100 || rt.contains("...") {
                return Err(AdminPoolError::TruncatedRefreshToken(rt.len()));
            }
        }

        // 2. 基于 sha256 哈希检测重复
        let store_map = self.store.snapshot();
        if new_cred.is_api_key_credential() {
            let new_hash = sha256_hex(new_cred.kiro_api_key.as_deref().unwrap());
            let exists = store_map
                .values()
                .any(|c| c.kiro_api_key.as_deref().map(sha256_hex).as_deref() == Some(&new_hash));
            if exists {
                return Err(AdminPoolError::DuplicateApiKey);
            }
        } else {
            let new_hash = sha256_hex(new_cred.refresh_token.as_deref().unwrap());
            let exists = store_map
                .values()
                .any(|c| c.refresh_token.as_deref().map(sha256_hex).as_deref() == Some(&new_hash));
            if exists {
                return Err(AdminPoolError::DuplicateRefreshToken);
            }
        }
        drop(store_map);

        // 3. 验证有效性：API Key 跳过；OAuth 调一次 refresh 拿 access_token
        if !new_cred.is_api_key_credential() {
            let outcome = match pick_refresher_kind(&new_cred) {
                RefresherKind::Idc => self.refresher_idc.refresh(&new_cred).await,
                RefresherKind::Social => self.refresher_social.refresh(&new_cred).await,
            }?;
            new_cred.apply_refresh(&outcome);
        }

        // 4. 写入 store（自动分配 id + 持久化）
        let new_id = self.store.add(new_cred)?;
        // 5. state 初始化为 enabled
        self.state.upsert(new_id, EntryState::default());

        tracing::info!("成功添加凭据 #{}", new_id);
        Ok(new_id)
    }

    /// 删除凭据（必须已禁用；删除当前凭据时重选）
    pub fn delete_credential(&self, id: u64) -> Result<(), AdminPoolError> {
        if self.store.get(id).is_none() {
            return Err(AdminPoolError::NotFound(id));
        }
        let is_disabled = self
            .state
            .get(id)
            .map(|s| s.disabled)
            .unwrap_or(false);
        if !is_disabled {
            return Err(AdminPoolError::NotDisabled(id));
        }

        let was_current = *self.current_id.lock() == Some(id);

        let _ = self.store.remove(id)?;
        self.state.remove(id);
        self.stats.remove(id);
        // 移除该 id 的 refresh 锁，避免重复禁用同 id 后内存泄漏
        self.refresh_locks.lock().remove(&id);

        if was_current {
            *self.current_id.lock() = None;
            self.select_highest_priority();
        }

        // 立即落盘 stats，清除已删凭据残留
        if let Some(store) = &self.stats_store {
            let _ = store.save(&self.stats.to_storage_map());
        }

        tracing::info!("已删除凭据 #{}", id);
        Ok(())
    }

    /// 强制刷新指定凭据 token（不论是否过期）
    ///
    /// admin 显式触发，因此即便并发的 prepare_token 已刷新，也仍执行一次 refresh
    /// 以满足"强制"语义；single-flight 锁仅用于串行化。
    pub async fn force_refresh_token_for(&self, id: u64) -> Result<(), AdminPoolError> {
        let cred = self.store.get(id).ok_or(AdminPoolError::NotFound(id))?;
        if cred.is_api_key_credential() {
            return Err(AdminPoolError::ApiKeyNotRefreshable);
        }
        let guard = self.refresh_guard_for(id);
        let _lock = guard.lock().await;
        // 拿锁后重读，使用最新 refresh_token 发起 refresh
        let fresh = self.store.get(id).unwrap_or(cred);
        let outcome = match pick_refresher_kind(&fresh) {
            RefresherKind::Idc => self.refresher_idc.refresh(&fresh).await,
            RefresherKind::Social => self.refresher_social.refresh(&fresh).await,
        }?;
        let mut updated = fresh;
        updated.apply_refresh(&outcome);
        // admin 路径：持久化失败应反馈给调用方
        let _ = self.store.replace(id, updated)?;
        self.state.report_success(id);
        tracing::info!("凭据 #{} Token 已强制刷新", id);
        Ok(())
    }

    /// 查询使用额度（必要时刷新 token，调上游 q.{region}.amazonaws.com/getUsageLimits）
    pub async fn get_usage_limits_for(
        &self,
        id: u64,
    ) -> Result<UsageLimitsResponse, AdminPoolError> {
        let cred = self.store.get(id).ok_or(AdminPoolError::NotFound(id))?;
        let (token, fresh_cred) = self.prepare_token_for_admin(id, &cred).await?;
        let usage = self.fetch_usage_limits(&fresh_cred, &token).await?;

        // 同步订阅等级到凭据（仅在变化时）
        if let Some(title) = usage.subscription_title()
            && fresh_cred.subscription_title.as_deref() != Some(title) {
                let mut updated = fresh_cred.clone();
                updated.subscription_title = Some(title.to_string());
                let _ = self.store.replace(id, updated);
            }
        Ok(usage)
    }

    /// admin 路径专用 prepare_token：API Key 直用；OAuth 必要时刷新并写回
    ///
    /// 与请求路径共享 single-flight 锁，避免 admin 操作与并发 acquire 重复刷新。
    async fn prepare_token_for_admin(
        &self,
        id: u64,
        cred: &Credential,
    ) -> Result<(String, Credential), AdminPoolError> {
        if cred.is_api_key_credential() {
            let token = cred
                .kiro_api_key
                .clone()
                .ok_or(AdminPoolError::MissingApiKey)?;
            return Ok((token, cred.clone()));
        }
        if let Some(token) = cred.access_token.clone()
            && !is_token_expired(cred)
        {
            return Ok((token, cred.clone()));
        }
        let guard = self.refresh_guard_for(id);
        let _lock = guard.lock().await;
        let fresh = self.store.get(id).unwrap_or_else(|| cred.clone());
        if let Some(token) = fresh.access_token.clone()
            && !is_token_expired(&fresh)
        {
            return Ok((token, fresh));
        }
        let outcome = match pick_refresher_kind(&fresh) {
            RefresherKind::Idc => self.refresher_idc.refresh(&fresh).await,
            RefresherKind::Social => self.refresher_social.refresh(&fresh).await,
        }?;
        let mut updated = fresh;
        updated.apply_refresh(&outcome);
        match self.store.replace(id, updated.clone()) {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(id, "刷新成功但凭据已被删除，token 仅本次请求有效");
            }
            Err(e) => {
                tracing::error!(?e, id, "刷新成功但持久化失败，凭据已更新到内存");
            }
        }
        Ok((outcome.access_token, updated))
    }

    async fn fetch_usage_limits(
        &self,
        cred: &Credential,
        token: &str,
    ) -> Result<UsageLimitsResponse, AdminPoolError> {
        let region = cred.effective_api_region(&self.config);
        let host = format!("q.{region}.amazonaws.com");
        let machine_id = self.resolver.resolve(cred, &self.config);
        let kiro_version = &self.config.kiro.kiro_version;
        let os_name = &self.config.kiro.system_version;
        let node_version = &self.config.kiro.node_version;

        let mut url =
            format!("https://{host}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST");
        if let Some(profile_arn) = &cred.profile_arn {
            url.push_str(&format!("&profileArn={}", urlencoding::encode(profile_arn)));
        }

        let user_agent = format!(
            "aws-sdk-js/1.0.0 ua/2.1 os/{os_name} lang/js md/nodejs#{node_version} api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{kiro_version}-{machine_id}"
        );
        let amz_user_agent = format!("aws-sdk-js/1.0.0 KiroIDE-{kiro_version}-{machine_id}");

        let global_proxy = self.config.proxy.proxy_url.as_deref().map(|url| {
            let mut p = ProxyConfig::new(url);
            if let (Some(u), Some(pw)) = (
                &self.config.proxy.proxy_username,
                &self.config.proxy.proxy_password,
            ) {
                p = p.with_auth(u, pw);
            }
            p
        });
        let effective_proxy = cred.effective_proxy(global_proxy.as_ref());
        let client = build_client(effective_proxy.as_ref(), 60, self.config.net.tls_backend)
            .map_err(|e| AdminPoolError::Network(e.to_string()))?;

        let mut req = client
            .get(&url)
            .header("x-amz-user-agent", &amz_user_agent)
            .header("user-agent", &user_agent)
            .header("host", &host)
            .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=1")
            .header("Authorization", format!("Bearer {token}"))
            .header("Connection", "close");
        if cred.is_api_key_credential() {
            req = req.header("tokentype", "API_KEY");
        }

        let response = req
            .send()
            .await
            .map_err(|e| AdminPoolError::Network(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // 截断 body，避免大响应回显放大 + 限制错误链中的敏感细节泄漏
            let body = truncate_upstream_body(&body, 512);
            return Err(AdminPoolError::UpstreamHttp {
                status: status.as_u16(),
                body,
            });
        }
        let usage: UsageLimitsResponse = response
            .json()
            .await
            .map_err(|e| AdminPoolError::Network(e.to_string()))?;
        Ok(usage)
    }
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn mask_api_key(key: &str) -> String {
    if key.is_ascii() && key.len() > 16 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "***".to_string()
    }
}

fn disabled_reason_to_str(reason: DisabledReason) -> &'static str {
    match reason {
        DisabledReason::Manual => "Manual",
        DisabledReason::TooManyFailures => "TooManyFailures",
        DisabledReason::TooManyRefreshFailures => "TooManyRefreshFailures",
        DisabledReason::QuotaExceeded => "QuotaExceeded",
        DisabledReason::InvalidRefreshToken => "InvalidRefreshToken",
        DisabledReason::InvalidConfig => "InvalidConfig",
    }
}

fn canonicalize_admin_auth_method(m: &str) -> String {
    if m.eq_ignore_ascii_case("builder-id") || m.eq_ignore_ascii_case("iam") {
        "idc".to_string()
    } else {
        m.to_string()
    }
}

/// 把 proxy URL 中的 password 替换为 ****（保留 scheme/user/host:port）
///
/// 仅识别 `<scheme>://[user:password@]host:port` 形式；无 password 时原样返回。
fn mask_proxy_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let (scheme, rest) = url.split_at(scheme_end);
    let after = &rest[3..]; // 去掉 "://"
    let Some(at_pos) = after.find('@') else {
        return url.to_string();
    };
    let userinfo = &after[..at_pos];
    let host_port = &after[at_pos + 1..];
    let Some(colon_pos) = userinfo.find(':') else {
        return url.to_string();
    };
    let user = &userinfo[..colon_pos];
    format!("{scheme}://{user}:****@{host_port}")
}

/// 截断 upstream body 至 max_bytes 字节（按 UTF-8 字符边界）
fn truncate_upstream_body(body: &str, max_bytes: usize) -> String {
    if body.len() <= max_bytes {
        return body.to_string();
    }
    // 找最近的 UTF-8 字符边界
    let mut idx = max_bytes;
    while idx > 0 && !body.is_char_boundary(idx) {
        idx -= 1;
    }
    format!("{}…(truncated)", &body[..idx])
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
    use crate::domain::token::{RefreshOutcome, TokenSource};
    use crate::infra::storage::CredentialsFileStore;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    fn tmp_path(tag: &str) -> PathBuf {
        let id = Uuid::new_v4();
        std::env::temp_dir().join(format!("kiro-rs-pool-test-{tag}-{id}.json"))
    }

    fn far_future_expires_at() -> String {
        (Utc::now() + Duration::days(7)).to_rfc3339()
    }

    /// 测试用 refresher：计数自增；可设置 sleep 模拟延迟
    #[derive(Debug)]
    struct MockRefresher {
        count: AtomicUsize,
        delay_ms: u64,
    }

    impl MockRefresher {
        fn new(delay_ms: u64) -> Self {
            Self {
                count: AtomicUsize::new(0),
                delay_ms,
            }
        }
        fn calls(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    impl TokenSource for MockRefresher {
        async fn refresh(
            &self,
            _cred: &Credential,
        ) -> Result<RefreshOutcome, crate::domain::error::RefreshError> {
            // 计数在 sleep 之前递增：未受 single-flight 保护时双方都会推到 2，
            // 测试断言 == 1 即可捕获锁失效（不依赖 sleep 串行化掩盖 bug）
            let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            Ok(RefreshOutcome {
                access_token: format!("mock-at-{n}"),
                refresh_token: None,
                profile_arn: None,
                expires_at: Some(far_future_expires_at()),
            })
        }
    }

    /// 构造一个使用 mock refresher 的 pool（凭据无 access_token，强制走 refresh 路径）
    fn pool_with_mock_refresher(
        n: usize,
        mode: &str,
        mock: Arc<dyn DynTokenSource>,
    ) -> (CredentialPool, PathBuf) {
        let path = tmp_path("mock-pool");
        let mut creds_json = Vec::new();
        for i in 0..n {
            creds_json.push(serde_json::json!({
                "refreshToken": format!("rt-{i}"),
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
        let (store, _) = CredentialStore::load(file, config.clone(), resolver.clone()).unwrap();
        let store = Arc::new(store);
        let state = Arc::new(CredentialState::new());
        let stats = Arc::new(CredentialStats::new());
        let pool = CredentialPool::new_with_refreshers(
            store,
            state,
            stats,
            None,
            config,
            resolver,
            mock.clone(),
            mock.clone(),
            mock,
        );
        let invalid: HashSet<u64> = HashSet::new();
        let initial_disabled: HashSet<u64> = HashSet::new();
        pool.install_initial_states(&invalid, &initial_disabled);
        (pool, path)
    }

    #[test]
    fn refresh_guard_for_returns_same_arc_for_same_id() {
        let (pool, path) = pool_with_n_credentials(1, MODE_PRIORITY);
        let g1 = pool.refresh_guard_for(1);
        let g2 = pool.refresh_guard_for(1);
        assert!(Arc::ptr_eq(&g1, &g2));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn refresh_guard_for_returns_distinct_arc_for_distinct_ids() {
        let (pool, path) = pool_with_n_credentials(2, MODE_PRIORITY);
        let g1 = pool.refresh_guard_for(1);
        let g2 = pool.refresh_guard_for(2);
        assert!(!Arc::ptr_eq(&g1, &g2));
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn prepare_token_serializes_concurrent_refresh_for_same_credential() {
        let mock = Arc::new(MockRefresher::new(50));
        let mock_dyn: Arc<dyn DynTokenSource> = mock.clone();
        let (pool, path) = pool_with_mock_refresher(1, MODE_PRIORITY, mock_dyn);
        let pool = Arc::new(pool);

        let p1 = pool.clone();
        let p2 = pool.clone();
        let (r1, r2) = tokio::join!(p1.acquire(None), p2.acquire(None));
        let ctx1 = r1.expect("acquire 1");
        let ctx2 = r2.expect("acquire 2");

        assert_eq!(
            mock.calls(),
            1,
            "single-flight：同一凭据并发 acquire 仅刷新 1 次"
        );
        assert_eq!(ctx1.token, ctx2.token, "两个并发 acquire 应拿到同一 access_token");
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn force_refresh_concurrent_calls_serialize_to_one_refresh() {
        let mock = Arc::new(MockRefresher::new(50));
        let mock_dyn: Arc<dyn DynTokenSource> = mock.clone();
        let (pool, path) = pool_with_mock_refresher(1, MODE_PRIORITY, mock_dyn);
        let pool = Arc::new(pool);

        let p1 = pool.clone();
        let p2 = pool.clone();
        let (r1, r2) = tokio::join!(p1.force_refresh(1), p2.force_refresh(1));
        r1.expect("force_refresh #1");
        r2.expect("force_refresh #2");

        assert_eq!(
            mock.calls(),
            1,
            "并发 force_refresh 应通过 single-flight 串行化为单次刷新"
        );
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn force_refresh_revoked_but_clock_valid_token_still_refreshes() {
        // server 已吊销 token（401/403 触发 force_refresh），但客户端时钟显示未过期：
        // 必须刷新，否则会循环复用同一个被拒绝的 token
        let mock = Arc::new(MockRefresher::new(0));
        let mock_dyn: Arc<dyn DynTokenSource> = mock.clone();

        let path = tmp_path("revoked-token");
        let arr = serde_json::json!([{
            "refreshToken": "rt-0",
            "accessToken": "old-at",
            "expiresAt": far_future_expires_at(),
            "authMethod": "social",
            "priority": 0,
        }]);
        fs::write(&path, serde_json::to_string_pretty(&arr).unwrap()).unwrap();

        let file = Arc::new(CredentialsFileStore::new(Some(path.clone())));
        let mut config = Config::default();
        config.features.load_balancing_mode = MODE_PRIORITY.to_string();
        let config = Arc::new(config);
        let resolver = Arc::new(MachineIdResolver::new());
        let (store, _) = CredentialStore::load(file, config.clone(), resolver.clone()).unwrap();
        let store = Arc::new(store);
        let state = Arc::new(CredentialState::new());
        let stats = Arc::new(CredentialStats::new());
        let pool = CredentialPool::new_with_refreshers(
            store,
            state,
            stats,
            None,
            config,
            resolver,
            mock_dyn.clone(),
            mock_dyn.clone(),
            mock_dyn,
        );
        let invalid: HashSet<u64> = HashSet::new();
        let initial_disabled: HashSet<u64> = HashSet::new();
        pool.install_initial_states(&invalid, &initial_disabled);

        pool.force_refresh(1).await.expect("force_refresh");
        assert_eq!(
            mock.calls(),
            1,
            "force_refresh 必须无视客户端时钟刷新一次（除非并发 refresh 已替换 token）"
        );
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn force_refresh_token_for_different_ids_runs_in_parallel() {
        let mock = Arc::new(MockRefresher::new(80));
        let mock_dyn: Arc<dyn DynTokenSource> = mock.clone();
        let (pool, path) = pool_with_mock_refresher(2, MODE_BALANCED, mock_dyn);
        let pool = Arc::new(pool);

        let start = std::time::Instant::now();
        let p1 = pool.clone();
        let p2 = pool.clone();
        let (r1, r2) = tokio::join!(
            p1.force_refresh_token_for(1),
            p2.force_refresh_token_for(2),
        );
        let elapsed = start.elapsed();
        r1.expect("refresh id=1");
        r2.expect("refresh id=2");

        assert_eq!(mock.calls(), 2, "不同 id 各自独立 refresh");
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "不同 id 应并行执行 (~80ms)，实际耗时 {elapsed:?}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn admin_snapshot_serializes_manual_reason() {
        let (pool, path) = pool_with_n_credentials(1, MODE_PRIORITY);
        let id = pool.store.ids()[0];
        pool.set_disabled(id, true).unwrap();
        let snap = pool.admin_snapshot();
        let entry = snap.entries.iter().find(|e| e.id == id).expect("entry exists");
        assert!(entry.disabled);
        assert_eq!(entry.disabled_reason.as_deref(), Some("Manual"));
        let _ = fs::remove_file(&path);
    }

    #[tokio::test]
    async fn delete_credential_removes_refresh_lock_entry() {
        let (pool, path) = pool_with_n_credentials(1, MODE_PRIORITY);
        let id = pool.store.ids()[0];
        // 触发懒创建
        let _g = pool.refresh_guard_for(id);
        assert!(pool.refresh_locks.lock().contains_key(&id));
        // 必须先禁用才能删除
        pool.set_disabled(id, true).unwrap();
        pool.delete_credential(id).unwrap();
        assert!(!pool.refresh_locks.lock().contains_key(&id));
        let _ = fs::remove_file(&path);
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

    #[test]
    fn current_id_initialized_to_lowest_priority_after_install_initial_states() {
        let (pool, path) = pool_with_n_credentials(3, MODE_PRIORITY);
        // priority=0 凭据是 store.load 排序后的第一个，id=1
        let snap = pool.admin_snapshot();
        assert_eq!(snap.current_id, 1, "current_id 应在装载后初始化为最低 priority 的 id");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn current_id_zero_when_all_disabled_at_install() {
        let path = tmp_path("all-disabled");
        let arr = serde_json::json!([
            {"refreshToken":"rt-0","accessToken":"at-0","expiresAt":far_future_expires_at(),"authMethod":"social","priority":0},
            {"refreshToken":"rt-1","accessToken":"at-1","expiresAt":far_future_expires_at(),"authMethod":"social","priority":1},
        ]);
        fs::write(&path, serde_json::to_string_pretty(&arr).unwrap()).unwrap();
        let file = Arc::new(crate::infra::storage::CredentialsFileStore::new(Some(path.clone())));
        let mut config = Config::default();
        config.features.load_balancing_mode = MODE_PRIORITY.to_string();
        let config = Arc::new(config);
        let resolver = Arc::new(MachineIdResolver::new());
        let (store, _) = CredentialStore::load(file, config.clone(), resolver.clone()).unwrap();
        let store = Arc::new(store);
        let state = Arc::new(CredentialState::new());
        let stats = Arc::new(CredentialStats::new());
        let pool = CredentialPool::new(store, state, stats, None, config, resolver);
        let invalid: HashSet<u64> = HashSet::new();
        // 所有 id 初始即禁用
        let initial_disabled: HashSet<u64> = pool.store.ids().into_iter().collect();
        pool.install_initial_states(&invalid, &initial_disabled);
        let snap = pool.admin_snapshot();
        assert_eq!(snap.current_id, 0, "无可用凭据时 current_id 保持 0");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn current_id_zero_in_balanced_mode_after_install() {
        let (pool, path) = pool_with_n_credentials(3, MODE_BALANCED);
        let snap = pool.admin_snapshot();
        assert_eq!(snap.current_id, 0, "balanced 模式不维护 current_id");
        let _ = fs::remove_file(&path);
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

    #[test]
    fn mask_proxy_url_replaces_password_only() {
        assert_eq!(
            mask_proxy_url("http://user:pass@host:8080"),
            "http://user:****@host:8080"
        );
        assert_eq!(
            mask_proxy_url("socks5://u:p@example.com:1080"),
            "socks5://u:****@example.com:1080"
        );
    }

    #[test]
    fn mask_proxy_url_preserves_no_userinfo() {
        assert_eq!(mask_proxy_url("http://host:8080"), "http://host:8080");
        assert_eq!(mask_proxy_url("https://example.com"), "https://example.com");
    }

    #[test]
    fn mask_proxy_url_preserves_user_only() {
        assert_eq!(
            mask_proxy_url("http://user@host:8080"),
            "http://user@host:8080"
        );
    }

    #[test]
    fn mask_proxy_url_preserves_invalid_format() {
        // 无 :// 直接返回
        assert_eq!(mask_proxy_url("not-a-url"), "not-a-url");
        assert_eq!(mask_proxy_url(""), "");
    }

    #[test]
    fn truncate_upstream_body_below_limit_returns_as_is() {
        let s = "short body";
        assert_eq!(truncate_upstream_body(s, 100), s);
    }

    #[test]
    fn truncate_upstream_body_above_limit_truncates_with_marker() {
        let body = "a".repeat(1000);
        let out = truncate_upstream_body(&body, 50);
        assert!(out.starts_with("aaaa"));
        assert!(out.ends_with("…(truncated)"));
        // 实际长度 ≤ 50（截断字节）+ marker；不超过 64
        assert!(out.len() <= 64);
    }

    #[test]
    fn truncate_upstream_body_respects_utf8_boundary() {
        // 中文每字符 3 字节，max=4 时应截到 3 字节边界（保留 1 字符）
        let body = "中文测试";
        let out = truncate_upstream_body(body, 4);
        // 至少包含一个中文字符 + marker
        assert!(out.contains('中'));
        assert!(out.ends_with("…(truncated)"));
    }
}
