//! 均衡选择器（Least-Used）：返回 success_count 最小的凭据；平局看 priority

use crate::domain::selector::{CredentialSelector, CredentialView};

#[derive(Default)]
pub struct BalancedSelector;

impl BalancedSelector {
    pub fn new() -> Self {
        Self
    }
}

impl CredentialSelector for BalancedSelector {
    fn select(&self, candidates: &[CredentialView<'_>], model: Option<&str>) -> Option<u64> {
        debug_assert!(
            candidates.iter().all(|v| !v.state.disabled),
            "selector 收到的 candidates 应全部为 enabled（pool 已过滤）"
        );
        let needs_opus = model.is_some_and(|m| m.to_lowercase().contains("opus"));
        candidates
            .iter()
            .filter(|v| !needs_opus || v.credential.supports_opus())
            .min_by(|a, b| {
                a.stats
                    .success_count
                    .cmp(&b.stats.success_count)
                    .then_with(|| a.credential.priority.cmp(&b.credential.priority))
            })
            .map(|v| v.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::credential::Credential;
    use crate::domain::selector::{CredentialStateView, CredentialStatsView};

    fn view<'a>(
        id: u64,
        cred: &'a Credential,
        state: &'a CredentialStateView,
        stats: &'a CredentialStatsView,
    ) -> CredentialView<'a> {
        CredentialView { id, credential: cred, state, stats }
    }

    fn enabled() -> CredentialStateView {
        CredentialStateView { disabled: false }
    }

    #[test]
    fn select_returns_least_used() {
        let selector = BalancedSelector::new();
        let c1 = Credential { priority: 0, ..Default::default() };
        let c2 = Credential { priority: 0, ..Default::default() };
        let c3 = Credential { priority: 0, ..Default::default() };
        let s = enabled();
        let st1 = CredentialStatsView { success_count: 100 };
        let st2 = CredentialStatsView { success_count: 5 };
        let st3 = CredentialStatsView { success_count: 50 };
        let candidates = vec![
            view(1, &c1, &s, &st1),
            view(2, &c2, &s, &st2),
            view(3, &c3, &s, &st3),
        ];
        assert_eq!(selector.select(&candidates, None), Some(2));
    }

    #[test]
    fn select_breaks_tie_by_priority() {
        let selector = BalancedSelector::new();
        let c1 = Credential { priority: 5, ..Default::default() };
        let c2 = Credential { priority: 1, ..Default::default() };
        let s = enabled();
        let st = CredentialStatsView { success_count: 10 };
        let candidates = vec![view(1, &c1, &s, &st), view(2, &c2, &s, &st)];
        // 同样 success_count → 比 priority → priority=1 更优
        assert_eq!(selector.select(&candidates, None), Some(2));
    }

    #[test]
    fn select_empty_returns_none() {
        let selector = BalancedSelector::new();
        assert_eq!(selector.select(&[], None), None);
    }

    #[test]
    fn select_skips_non_opus_when_model_is_opus() {
        let selector = BalancedSelector::new();
        let c1 = Credential {
            subscription_title: Some("FREE".to_string()),
            ..Default::default()
        };
        let c2 = Credential {
            subscription_title: Some("PRO".to_string()),
            ..Default::default()
        };
        let s = enabled();
        let st_low = CredentialStatsView { success_count: 1 };
        let st_high = CredentialStatsView { success_count: 100 };
        let candidates = vec![view(1, &c1, &s, &st_low), view(2, &c2, &s, &st_high)];
        // opus 模型应跳过 free，即使 free 的 success_count 更低
        assert_eq!(selector.select(&candidates, Some("opus-4")), Some(2));
    }
}
