//! 用 Kiro 的 credit 消耗反推 Anthropic 的缓存读取用量。
//!
//! Kiro 只上报上下文大小和一笔 credit，不给缓存 token。
//! 缓存读取倍率默认按 Claude 官方价格。可以不重新构建，单独覆盖某个模型：
//! `KIRO_CREDIT_CACHE_RATIO_OPUS55`、`KIRO_CREDIT_CACHE_RATIO_SONNET55`、
//! `KIRO_CREDIT_CACHE_RATIO_SONNET5`、`KIRO_CREDIT_CACHE_RATIO_HAIKU45`。
//! 倍率必须大于 0 且小于 1，否则回退到默认值。缓存创建不计入。下游售价不参与计算。

use super::converter::map_model;

/// Sonnet 5.5 每个未缓存输入 token 的 credit。
///
/// 2026-10-21 用一对约 1200 token 的请求测得边际费率
/// `0.0261034826 / 1200 = 0.0000217529`。这是单点估计，不是完整价格曲线。
/// 可用 `KIRO_CREDIT_ALPHA_SONNET55` 覆盖。
/// Opus 5.5 同期测得约 `0.0000331675` credit/token，但默认不启用，
/// 需要显式设置 `KIRO_CREDIT_ALPHA_OPUS55`。
const DEFAULT_SONNET55_ALPHA: f64 = 0.0000217529;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageSplit {
    pub input_tokens: i32,
    pub cache_read_input_tokens: i32,
    pub cache_creation_input_tokens: i32,
    pub output_tokens: i32,
}

pub fn tokens_from_context_percentage(percentage: f64, window: i32) -> i32 {
    if !percentage.is_finite() || percentage <= 0.0 || window <= 0 {
        return 0;
    }
    round_non_negative(percentage / 100.0 * f64::from(window))
}

/// 把测得的上下文大小拆成未缓存输入和缓存读取。
///
/// 没有 credit 样本或没有配置 alpha 时，全部算作未缓存输入。
/// 缓存读取被限制在 `0..=total_input`。如果结果会让未缓存输入变成 0，
/// 则保留 1 个未缓存 token：部分下游会忽略流式响应里的 `input_tokens: 0`，
/// 否则会退回整段未缓存的估算值。
pub fn split_usage(
    model: &str,
    total_input: i32,
    output_tokens: i32,
    credits: Option<f64>,
) -> UsageSplit {
    let total_input = total_input.max(0);
    let output_tokens = output_tokens.max(0);
    let plain = UsageSplit {
        input_tokens: total_input,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
        output_tokens,
    };

    let Some(credits) = credits.filter(|value| value.is_finite() && *value >= 0.0) else {
        return plain;
    };
    let Some(alpha) = alpha_for_model(model).filter(|value| *value > 0.0) else {
        return plain;
    };
    let Some(cache_ratio) = official_cache_read_ratio(model) else {
        return plain;
    };

    let equivalent_input = credits / alpha;
    let raw_cache = (f64::from(total_input) + 5.0 * f64::from(output_tokens) - equivalent_input)
        / (1.0 - cache_ratio);
    let mut cache_read = round_non_negative(raw_cache).min(total_input);
    if cache_read == total_input && total_input > 0 {
        cache_read -= 1;
    }

    UsageSplit {
        input_tokens: total_input - cache_read,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: 0,
        output_tokens,
    }
}

pub fn observe_credit(unit: &str, usage: f64) -> Option<f64> {
    if !usage.is_finite() || usage < 0.0 {
        return None;
    }
    if unit.is_empty() || unit.eq_ignore_ascii_case("credit") {
        Some(usage)
    } else {
        None
    }
}

fn cache_ratio_from_raw(raw: Option<&str>, default: f64) -> f64 {
    let Some(raw) = raw else {
        return default;
    };
    match raw.trim().parse::<f64>() {
        Ok(value) if value.is_finite() && value > 0.0 && value < 1.0 => value,
        _ => default,
    }
}

fn cache_ratio_from_env(env_name: &str, default: f64) -> f64 {
    match std::env::var(env_name) {
        Ok(raw) => cache_ratio_from_raw(Some(raw.as_str()), default),
        Err(_) => default,
    }
}

fn official_cache_read_ratio(model: &str) -> Option<f64> {
    // 默认倍率是官方缓存读取价。重复请求若更接近全价的 10%，用环境变量覆盖。
    let (env_name, default) = match mapped_model(model).as_str() {
        "claude-sonnet-5.5" => ("KIRO_CREDIT_CACHE_RATIO_SONNET55", 0.05),
        "claude-opus-5.5" => ("KIRO_CREDIT_CACHE_RATIO_OPUS55", 0.05),
        "claude-sonnet-5" => ("KIRO_CREDIT_CACHE_RATIO_SONNET5", 0.10),
        "claude-haiku-4.5" => ("KIRO_CREDIT_CACHE_RATIO_HAIKU45", 0.10),
        _ => return None,
    };
    Some(cache_ratio_from_env(env_name, default))
}

fn alpha_from_raw(raw: Option<&str>, default: Option<f64>) -> Option<f64> {
    let Some(raw) = raw else {
        return default;
    };
    match raw.trim().parse::<f64>() {
        Ok(value) if value.is_finite() && value > 0.0 => Some(value),
        Ok(_) => None,
        Err(_) => default,
    }
}

fn alpha_for_model(model: &str) -> Option<f64> {
    let (env_name, default) = match mapped_model(model).as_str() {
        "claude-sonnet-5.5" => ("KIRO_CREDIT_ALPHA_SONNET55", Some(DEFAULT_SONNET55_ALPHA)),
        "claude-opus-5.5" => ("KIRO_CREDIT_ALPHA_OPUS55", None),
        "claude-sonnet-5" => ("KIRO_CREDIT_ALPHA_SONNET5", None),
        "claude-haiku-4.5" => ("KIRO_CREDIT_ALPHA_HAIKU45", None),
        _ => return None,
    };
    match std::env::var(env_name) {
        Ok(raw) => alpha_from_raw(Some(raw.as_str()), default),
        Err(_) => default,
    }
}

fn mapped_model(model: &str) -> String {
    map_model(model).unwrap_or_else(|| model.to_string())
}

fn round_non_negative(value: f64) -> i32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let rounded = value.round();
    if rounded >= f64::from(i32::MAX) {
        i32::MAX
    } else {
        rounded as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_percentage_rounds_to_the_probe_token_count() {
        assert_eq!(tokens_from_context_percentage(1.5994, 1_000_000), 15_994);
        assert_eq!(tokens_from_context_percentage(1.59945, 1_000_000), 15_995);
    }

    #[test]
    fn sonnet55_alpha_uses_the_measured_marginal_rate() {
        assert!((DEFAULT_SONNET55_ALPHA - 0.0000217529).abs() < 1e-12);
    }

    #[test]
    fn uncached_sonnet_at_the_measured_rate_does_not_invent_cache() {
        let credits = 15_994.0 * DEFAULT_SONNET55_ALPHA;
        let split = split_usage("claude-sonnet-5.5", 15_994, 0, Some(credits));
        assert_eq!(split.cache_read_input_tokens, 0);
        assert_eq!(split.input_tokens, 15_994);
        assert_eq!(split.cache_creation_input_tokens, 0);
    }

    #[test]
    fn cheaper_follow_up_is_mostly_cache_read() {
        // 15800 context and 12000 cache reads at the official 5% ratio:
        // equivalent input = 15800 - 0.95 * 12000 = 4400.
        let credits = 4_400.0 * DEFAULT_SONNET55_ALPHA;
        let split = split_usage("claude-sonnet-5.5", 15_800, 0, Some(credits));
        assert_eq!(split.cache_read_input_tokens, 12_000);
        assert_eq!(split.input_tokens, 3_800);
        assert_eq!(split.input_tokens + split.cache_read_input_tokens, 15_800);
    }

    #[test]
    fn output_uses_the_official_five_x_weight() {
        // 100 fresh input + 20 output at 5x is 200 equivalent input tokens.
        let credits = 200.0 * DEFAULT_SONNET55_ALPHA;
        let split = split_usage("claude-sonnet-5.5", 100, 20, Some(credits));
        assert_eq!(split.cache_read_input_tokens, 0);
        assert_eq!(split.output_tokens, 20);
    }

    #[test]
    fn all_cached_keeps_one_fresh_token() {
        let split = split_usage("claude-sonnet-5.5", 1_000, 0, Some(0.0));
        assert_eq!(split.input_tokens, 1);
        assert_eq!(split.cache_read_input_tokens, 999);
    }

    #[test]
    fn opus_without_alpha_is_unchanged() {
        let split = split_usage("claude-opus-5.5", 1_000, 12, Some(0.01));
        assert_eq!(split.input_tokens, 1_000);
        assert_eq!(split.cache_read_input_tokens, 0);
        assert_eq!(split.output_tokens, 12);
    }

    #[test]
    fn cache_ratio_parser_rejects_invalid_values() {
        assert_eq!(cache_ratio_from_raw(None, 0.05), 0.05);
        assert!((cache_ratio_from_raw(Some("0.10"), 0.05) - 0.10).abs() < 1e-12);
        assert_eq!(cache_ratio_from_raw(Some("1"), 0.05), 0.05);
        assert_eq!(cache_ratio_from_raw(Some("0"), 0.05), 0.05);
        assert_eq!(cache_ratio_from_raw(Some("nope"), 0.05), 0.05);
        assert_eq!(official_cache_read_ratio("claude-opus-5.5"), Some(0.05));
        assert_eq!(official_cache_read_ratio("claude-opus-5-5"), Some(0.05));
        assert_eq!(official_cache_read_ratio("claude-sonnet-5.5"), Some(0.05));
        assert_eq!(official_cache_read_ratio("claude-sonnet-5"), Some(0.10));
        assert_eq!(official_cache_read_ratio("claude-haiku-4.5"), Some(0.10));
    }

    #[test]
    fn non_positive_alpha_disables_inference() {
        assert_eq!(
            alpha_from_raw(Some("0"), Some(DEFAULT_SONNET55_ALPHA)),
            None
        );
        assert_eq!(
            alpha_from_raw(Some("-1"), Some(DEFAULT_SONNET55_ALPHA)),
            None
        );
        assert_eq!(
            alpha_from_raw(Some("nope"), Some(DEFAULT_SONNET55_ALPHA)),
            Some(DEFAULT_SONNET55_ALPHA)
        );
        assert_eq!(alpha_from_raw(None, None), None);
        let overridden = alpha_from_raw(Some("0.0000331675"), None).unwrap();
        assert!((overridden - 0.0000331675).abs() < 1e-12);
    }
}
