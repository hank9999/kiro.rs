//! 优先级选择器：返回 priority 最小的可用凭据；model 含 "opus" 时跳过不支持 opus 的凭据

use crate::domain::selector::{CredentialSelector, CredentialView};

#[derive(Default)]
pub struct PrioritySelector;

impl PrioritySelector {
    pub fn new() -> Self {
        Self
    }
}

impl CredentialSelector for PrioritySelector {
    fn select(&self, candidates: &[CredentialView<'_>], model: Option<&str>) -> Option<u64> {
        debug_assert!(
            candidates.iter().all(|v| !v.state.disabled),
            "selector 收到的 candidates 应全部为 enabled（pool 已过滤）"
        );
        let needs_opus = model.is_some_and(|m| m.to_lowercase().contains("opus"));
        candidates
            .iter()
            .filter(|v| !needs_opus || v.credential.supports_opus())
            .min_by_key(|v| v.credential.priority)
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

    fn stats(success_count: u64) -> CredentialStatsView {
        CredentialStatsView { success_count }
    }

    #[test]
    fn select_empty_returns_none() {
        let selector = PrioritySelector::new();
        assert_eq!(selector.select(&[], None), None);
    }

    #[test]
    fn select_returns_lowest_priority() {
        let selector = PrioritySelector::new();
        let c1 = Credential { priority: 5, ..Default::default() };
        let c2 = Credential { priority: 1, ..Default::default() };
        let c3 = Credential { priority: 3, ..Default::default() };
        let s = enabled();
        let st = stats(0);
        let candidates = vec![
            view(1, &c1, &s, &st),
            view(2, &c2, &s, &st),
            view(3, &c3, &s, &st),
        ];
        assert_eq!(selector.select(&candidates, None), Some(2));
    }

    #[test]
    fn select_skips_non_opus_when_model_is_opus() {
        let selector = PrioritySelector::new();
        // free 不支持 opus
        let c1 = Credential {
            priority: 0,
            subscription_title: Some("KIRO FREE".to_string()),
            ..Default::default()
        };
        // pro 支持 opus
        let c2 = Credential {
            priority: 5,
            subscription_title: Some("KIRO PRO".to_string()),
            ..Default::default()
        };
        let s = enabled();
        let st = stats(0);
        let candidates = vec![view(1, &c1, &s, &st), view(2, &c2, &s, &st)];
        assert_eq!(
            selector.select(&candidates, Some("claude-opus-4-7")),
            Some(2),
            "opus 模型必须跳过 free 凭据"
        );
        // 非 opus 模型不过滤
        assert_eq!(selector.select(&candidates, Some("claude-sonnet-4-6")), Some(1));
    }

    #[test]
    fn select_returns_first_in_tie() {
        let selector = PrioritySelector::new();
        let c1 = Credential { priority: 1, ..Default::default() };
        let c2 = Credential { priority: 1, ..Default::default() };
        let s = enabled();
        let st = stats(0);
        let candidates = vec![view(10, &c1, &s, &st), view(20, &c2, &s, &st)];
        // min_by_key 平局返回最先出现的
        assert_eq!(selector.select(&candidates, None), Some(10));
    }

    #[test]
    fn select_returns_none_if_all_filtered_by_opus() {
        let selector = PrioritySelector::new();
        let c1 = Credential {
            priority: 0,
            subscription_title: Some("FREE".to_string()),
            ..Default::default()
        };
        let s = enabled();
        let st = stats(0);
        let candidates = vec![view(1, &c1, &s, &st)];
        assert_eq!(selector.select(&candidates, Some("claude-opus-4")), None);
    }
}
