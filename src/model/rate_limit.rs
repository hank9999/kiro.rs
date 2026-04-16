use std::collections::BTreeMap;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

pub const MAX_RATE_LIMIT_WINDOW_SECS: u64 = 30 * 24 * 60 * 60;

/// 可序列化的限流规则
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitRule {
    /// 时间窗口，格式为 `正整数 + 单位`
    /// 例如：`30s`、`5m`、`2h`、`1d`
    pub window: String,
    /// 该时间窗口内允许的最大请求数
    pub max_requests: u32,
}

/// 已解析并规范化的限流规则
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRateLimitRule {
    pub window: String,
    pub window_seconds: u64,
    pub max_requests: u32,
}

pub fn parse_window_seconds(window: &str) -> anyhow::Result<u64> {
    if window.len() < 2 {
        bail!("时间窗口格式无效: {}", window);
    }

    let (number_part, unit_part) = window.split_at(window.len() - 1);
    if number_part.is_empty() || !number_part.chars().all(|c| c.is_ascii_digit()) {
        bail!("时间窗口必须是正整数加单位: {}", window);
    }

    let value = number_part
        .parse::<u64>()
        .with_context(|| format!("时间窗口数值无效: {}", window))?;
    if value == 0 {
        bail!("时间窗口必须大于 0: {}", window);
    }

    let multiplier = match unit_part {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => bail!("不支持的时间单位: {}", window),
    };

    let seconds = value
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow::anyhow!("时间窗口过大: {}", window))?;
    if seconds > MAX_RATE_LIMIT_WINDOW_SECS {
        bail!(
            "时间窗口不能超过 {} 秒（当前: {}）",
            MAX_RATE_LIMIT_WINDOW_SECS,
            window
        );
    }

    Ok(seconds)
}

pub fn canonical_window(seconds: u64) -> String {
    if seconds.is_multiple_of(24 * 60 * 60) {
        format!("{}d", seconds / (24 * 60 * 60))
    } else if seconds.is_multiple_of(60 * 60) {
        format!("{}h", seconds / (60 * 60))
    } else if seconds.is_multiple_of(60) {
        format!("{}m", seconds / 60)
    } else {
        format!("{}s", seconds)
    }
}

pub fn resolve_rate_limit_rules(
    rules: &[RateLimitRule],
    source: &str,
) -> anyhow::Result<Vec<ResolvedRateLimitRule>> {
    let mut by_window = BTreeMap::new();

    for rule in rules {
        if rule.max_requests == 0 {
            bail!("{} 中 maxRequests 必须大于 0: {}", source, rule.window);
        }

        let seconds = parse_window_seconds(&rule.window)
            .with_context(|| format!("{} 中存在非法窗口", source))?;
        let normalized = ResolvedRateLimitRule {
            window: canonical_window(seconds),
            window_seconds: seconds,
            max_requests: rule.max_requests,
        };

        if by_window.insert(seconds, normalized).is_some() {
            bail!("{} 中存在重复的时间窗口: {}", source, rule.window);
        }
    }

    Ok(by_window.into_values().collect())
}

pub fn validate_rate_limit_rules(
    rules: Option<&[RateLimitRule]>,
    source: &str,
) -> anyhow::Result<()> {
    if let Some(rules) = rules {
        resolve_rate_limit_rules(rules, source)?;
    }
    Ok(())
}

pub fn effective_rate_limit_rules(
    defaults: Option<&[RateLimitRule]>,
    overrides: Option<&[RateLimitRule]>,
) -> anyhow::Result<Vec<RateLimitRule>> {
    let chosen = if let Some(overrides) = overrides {
        resolve_rate_limit_rules(overrides, "rateLimits")?
    } else if let Some(defaults) = defaults {
        resolve_rate_limit_rules(defaults, "defaultRateLimits")?
    } else {
        Vec::new()
    };

    Ok(chosen
        .into_iter()
        .map(|rule| RateLimitRule {
            window: canonical_window(rule.window_seconds),
            max_requests: rule.max_requests,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_window_seconds() {
        assert_eq!(parse_window_seconds("30s").unwrap(), 30);
        assert_eq!(parse_window_seconds("5m").unwrap(), 300);
        assert_eq!(parse_window_seconds("2h").unwrap(), 7200);
        assert_eq!(parse_window_seconds("1d").unwrap(), 86400);
    }

    #[test]
    fn test_parse_window_seconds_rejects_invalid() {
        assert!(parse_window_seconds("0m").is_err());
        assert!(parse_window_seconds("1.5h").is_err());
        assert!(parse_window_seconds("abc").is_err());
        assert!(parse_window_seconds("31d").is_err());
    }

    #[test]
    fn test_effective_rate_limit_rules_prefers_override_set() {
        let defaults = vec![
            RateLimitRule {
                window: "5m".to_string(),
                max_requests: 100,
            },
            RateLimitRule {
                window: "24h".to_string(),
                max_requests: 3000,
            },
        ];
        let overrides = vec![
            RateLimitRule {
                window: "2m".to_string(),
                max_requests: 20,
            },
            RateLimitRule {
                window: "1d".to_string(),
                max_requests: 2000,
            },
        ];

        let merged = effective_rate_limit_rules(Some(&defaults), Some(&overrides)).unwrap();

        assert_eq!(
            merged,
            vec![
                RateLimitRule {
                    window: "2m".to_string(),
                    max_requests: 20,
                },
                RateLimitRule {
                    window: "1d".to_string(),
                    max_requests: 2000,
                },
            ]
        );
    }

    #[test]
    fn test_effective_rate_limit_rules_falls_back_to_defaults() {
        let defaults = vec![RateLimitRule {
            window: "5m".to_string(),
            max_requests: 100,
        }];

        let effective = effective_rate_limit_rules(Some(&defaults), None).unwrap();

        assert_eq!(effective, defaults);
    }
}
