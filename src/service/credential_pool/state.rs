//! 凭据状态：失败计数 + 禁用 + 自愈
//!
//! 锁顺序约束（参见 pool.rs）：`store -> state -> stats`，禁止反向获取。

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::domain::retry::DisabledReason;

/// 单凭据失败超过此值即自动禁用为 TooManyFailures
pub const MAX_FAILURES_PER_CREDENTIAL: u32 = 3;

/// 单条凭据的运行时状态
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryState {
    pub failure_count: u32,
    pub refresh_failure_count: u32,
    pub disabled: bool,
    pub disabled_reason: Option<DisabledReason>,
}

impl EntryState {
    pub fn enabled() -> Self {
        Self::default()
    }

    pub fn disabled_with(reason: DisabledReason) -> Self {
        Self {
            disabled: true,
            disabled_reason: Some(reason),
            ..Default::default()
        }
    }
}

/// id → 状态的 map（不持有凭据数据本身，数据由 store 持有）
#[derive(Default)]
pub struct CredentialState {
    entries: Mutex<HashMap<u64, EntryState>>,
}

impl CredentialState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&self, id: u64, state: EntryState) {
        self.entries.lock().insert(id, state);
    }

    pub fn remove(&self, id: u64) {
        self.entries.lock().remove(&id);
    }

    pub fn get(&self, id: u64) -> Option<EntryState> {
        self.entries.lock().get(&id).cloned()
    }

    pub fn snapshot(&self) -> HashMap<u64, EntryState> {
        self.entries.lock().clone()
    }

    pub fn ids(&self) -> Vec<u64> {
        self.entries.lock().keys().copied().collect()
    }

    /// 报告一次成功：failure_count = 0, refresh_failure_count = 0
    ///
    /// 不主动启用 disabled——禁用状态只能由 set_disabled 或自愈清除。
    pub fn report_success(&self, id: u64) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get_mut(&id) {
            entry.failure_count = 0;
            entry.refresh_failure_count = 0;
        }
    }

    /// 报告一次失败；failure_count 达到 MAX 时自动禁用为 TooManyFailures
    /// 返回 true 表示该凭据现在处于 disabled 状态
    pub fn report_failure(&self, id: u64) -> bool {
        let mut entries = self.entries.lock();
        let entry = entries.entry(id).or_default();
        entry.failure_count = entry.failure_count.saturating_add(1);
        if entry.failure_count >= MAX_FAILURES_PER_CREDENTIAL && !entry.disabled {
            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::TooManyFailures);
        }
        entry.disabled
    }

    /// 立即禁用为 QuotaExceeded
    pub fn report_quota_exhausted(&self, id: u64) -> bool {
        let mut entries = self.entries.lock();
        let entry = entries.entry(id).or_default();
        entry.disabled = true;
        entry.disabled_reason = Some(DisabledReason::QuotaExceeded);
        true
    }

    /// 报告一次 refresh 失败；累计达到 MAX 时禁用
    pub fn report_refresh_failure(&self, id: u64) -> bool {
        let mut entries = self.entries.lock();
        let entry = entries.entry(id).or_default();
        entry.refresh_failure_count = entry.refresh_failure_count.saturating_add(1);
        if entry.refresh_failure_count >= MAX_FAILURES_PER_CREDENTIAL && !entry.disabled {
            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::TooManyFailures);
        }
        entry.disabled
    }

    /// 立即禁用为 InvalidRefreshToken（refresh_token 永久失效）
    pub fn report_refresh_token_invalid(&self, id: u64) -> bool {
        let mut entries = self.entries.lock();
        let entry = entries.entry(id).or_default();
        entry.disabled = true;
        entry.disabled_reason = Some(DisabledReason::InvalidRefreshToken);
        true
    }

    /// 用户手动启用/禁用
    ///
    /// 启用时清空 disabled_reason 与 failure_count；禁用时不指定具体 reason。
    pub fn set_disabled(&self, id: u64, disabled: bool) {
        let mut entries = self.entries.lock();
        let entry = entries.entry(id).or_default();
        entry.disabled = disabled;
        if !disabled {
            entry.disabled_reason = None;
            entry.failure_count = 0;
            entry.refresh_failure_count = 0;
        }
    }

    /// 设置带原因的禁用（如 InvalidConfig）
    pub fn set_disabled_with_reason(&self, id: u64, reason: DisabledReason) {
        let mut entries = self.entries.lock();
        let entry = entries.entry(id).or_default();
        entry.disabled = true;
        entry.disabled_reason = Some(reason);
    }

    /// 自愈：把所有 disabled_reason == TooManyFailures 的条目重置
    /// 返回 true 表示至少一条被自愈
    ///
    /// QuotaExceeded / InvalidRefreshToken / InvalidConfig **不参与自愈**——
    /// 这些是确定性失败，需要外部干预（充值 / 重新登录 / 修改配置）。
    pub fn heal_too_many_failures(&self) -> bool {
        let mut entries = self.entries.lock();
        let mut healed = false;
        for entry in entries.values_mut() {
            if entry.disabled
                && entry.disabled_reason == Some(DisabledReason::TooManyFailures)
            {
                entry.disabled = false;
                entry.disabled_reason = None;
                entry.failure_count = 0;
                entry.refresh_failure_count = 0;
                healed = true;
            }
        }
        healed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_entry(state: &CredentialState, id: u64) {
        state.upsert(id, EntryState::default());
    }

    #[test]
    fn report_failure_disables_after_3_times() {
        let state = CredentialState::new();
        ensure_entry(&state, 1);
        assert!(!state.report_failure(1));
        assert!(!state.report_failure(1));
        let now_disabled = state.report_failure(1);
        assert!(now_disabled);
        let s = state.get(1).unwrap();
        assert_eq!(s.failure_count, 3);
        assert!(s.disabled);
        assert_eq!(s.disabled_reason, Some(DisabledReason::TooManyFailures));
    }

    #[test]
    fn report_quota_exhausted_disables_immediately() {
        let state = CredentialState::new();
        ensure_entry(&state, 2);
        assert!(state.report_quota_exhausted(2));
        let s = state.get(2).unwrap();
        assert!(s.disabled);
        assert_eq!(s.disabled_reason, Some(DisabledReason::QuotaExceeded));
    }

    #[test]
    fn report_refresh_token_invalid_disables_immediately() {
        let state = CredentialState::new();
        ensure_entry(&state, 3);
        assert!(state.report_refresh_token_invalid(3));
        let s = state.get(3).unwrap();
        assert!(s.disabled);
        assert_eq!(s.disabled_reason, Some(DisabledReason::InvalidRefreshToken));
    }

    #[test]
    fn report_success_resets_failure_count() {
        let state = CredentialState::new();
        ensure_entry(&state, 4);
        state.report_failure(4);
        state.report_failure(4);
        state.report_success(4);
        let s = state.get(4).unwrap();
        assert_eq!(s.failure_count, 0);
        assert!(!s.disabled);
    }

    #[test]
    fn heal_too_many_failures_resets_only_too_many_failures() {
        let state = CredentialState::new();
        ensure_entry(&state, 1);
        ensure_entry(&state, 2);
        ensure_entry(&state, 3);
        // 1: TooManyFailures
        for _ in 0..3 {
            state.report_failure(1);
        }
        // 2: QuotaExceeded
        state.report_quota_exhausted(2);
        // 3: InvalidRefreshToken
        state.report_refresh_token_invalid(3);

        let healed = state.heal_too_many_failures();
        assert!(healed);

        // 1 自愈
        let s1 = state.get(1).unwrap();
        assert!(!s1.disabled);
        assert!(s1.disabled_reason.is_none());
        assert_eq!(s1.failure_count, 0);

        // 2 / 3 不自愈
        assert!(state.get(2).unwrap().disabled);
        assert_eq!(
            state.get(2).unwrap().disabled_reason,
            Some(DisabledReason::QuotaExceeded)
        );
        assert!(state.get(3).unwrap().disabled);
        assert_eq!(
            state.get(3).unwrap().disabled_reason,
            Some(DisabledReason::InvalidRefreshToken)
        );
    }

    #[test]
    fn set_disabled_false_clears_reason_and_failure_count() {
        let state = CredentialState::new();
        ensure_entry(&state, 1);
        for _ in 0..3 {
            state.report_failure(1);
        }
        state.set_disabled(1, false);
        let s = state.get(1).unwrap();
        assert!(!s.disabled);
        assert!(s.disabled_reason.is_none());
        assert_eq!(s.failure_count, 0);
    }

    #[test]
    fn set_disabled_with_reason_invalid_config() {
        let state = CredentialState::new();
        state.set_disabled_with_reason(7, DisabledReason::InvalidConfig);
        let s = state.get(7).unwrap();
        assert!(s.disabled);
        assert_eq!(s.disabled_reason, Some(DisabledReason::InvalidConfig));
    }

    #[test]
    fn heal_returns_false_when_nothing_to_heal() {
        let state = CredentialState::new();
        ensure_entry(&state, 1);
        state.report_quota_exhausted(1);
        let healed = state.heal_too_many_failures();
        assert!(!healed);
        assert!(state.get(1).unwrap().disabled);
    }
}
