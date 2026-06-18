use serde::Serialize;

use crate::model::config::CacheSimulationConfig;

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheSimulationDecision {
    cache_ratio_basis_points: Option<u16>,
    minimum_input_tokens: u32,
    minimum_uncached_tokens: u32,
}

impl CacheSimulationDecision {
    pub fn sample(config: CacheSimulationConfig) -> Self {
        let hit = config.enabled
            && config.hit_probability > 0
            && config.min_cache_ratio <= config.max_cache_ratio
            && config.max_cache_ratio <= 100
            && fastrand::u8(0..100) < config.hit_probability;

        let cache_ratio_basis_points = hit.then(|| {
            let min = u16::from(config.min_cache_ratio) * 100;
            let max = u16::from(config.max_cache_ratio) * 100;
            fastrand::u16(min..=max)
        });

        Self {
            cache_ratio_basis_points,
            minimum_input_tokens: config.minimum_input_tokens,
            minimum_uncached_tokens: config.minimum_uncached_tokens,
        }
    }

    pub fn apply(self, total_input_tokens: i32, output_tokens: i32) -> AnthropicUsage {
        let total_input_tokens = total_input_tokens.max(0);
        let total_input_tokens_u32 = total_input_tokens as u32;
        let cache_read_input_tokens = match self.cache_ratio_basis_points {
            Some(ratio) if total_input_tokens_u32 >= self.minimum_input_tokens => {
                let proportional =
                    (u64::from(total_input_tokens_u32) * u64::from(ratio) / 10_000) as u32;
                let maximum_cacheable =
                    total_input_tokens_u32.saturating_sub(self.minimum_uncached_tokens);
                proportional.min(maximum_cacheable) as i32
            }
            _ => 0,
        };

        AnthropicUsage {
            input_tokens: total_input_tokens - cache_read_input_tokens,
            output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AnthropicUsage {
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cache_creation_input_tokens: i32,
    pub cache_read_input_tokens: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_cache_tokens_without_increasing_total_input() {
        let decision = CacheSimulationDecision {
            cache_ratio_basis_points: Some(8_500),
            minimum_input_tokens: 0,
            minimum_uncached_tokens: 0,
        };
        let usage = decision.apply(1_000, 25);

        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.cache_read_input_tokens, 850);
        assert_eq!(usage.input_tokens + usage.cache_read_input_tokens, 1_000);
    }

    #[test]
    fn preserves_minimum_uncached_tokens() {
        let decision = CacheSimulationDecision {
            cache_ratio_basis_points: Some(9_000),
            minimum_input_tokens: 0,
            minimum_uncached_tokens: 64,
        };
        let usage = decision.apply(120, 5);

        assert_eq!(usage.input_tokens, 64);
        assert_eq!(usage.cache_read_input_tokens, 56);
    }

    #[test]
    fn samples_a_hit_at_one_hundred_percent() {
        let decision = CacheSimulationDecision::sample(CacheSimulationConfig {
            enabled: true,
            hit_probability: 100,
            min_cache_ratio: 80,
            max_cache_ratio: 80,
            minimum_input_tokens: 0,
            minimum_uncached_tokens: 0,
        });

        assert_eq!(decision.apply(1_000, 0).cache_read_input_tokens, 800);
    }

    #[test]
    fn disabled_simulation_never_hits() {
        let decision = CacheSimulationDecision::sample(CacheSimulationConfig {
            enabled: false,
            hit_probability: 100,
            min_cache_ratio: 80,
            max_cache_ratio: 90,
            minimum_input_tokens: 0,
            minimum_uncached_tokens: 0,
        });

        assert_eq!(decision.apply(1_000, 0).cache_read_input_tokens, 0);
    }

    #[test]
    fn short_requests_do_not_hit() {
        let decision = CacheSimulationDecision::sample(CacheSimulationConfig {
            enabled: true,
            hit_probability: 100,
            min_cache_ratio: 80,
            max_cache_ratio: 90,
            minimum_input_tokens: 1024,
            minimum_uncached_tokens: 64,
        });

        assert_eq!(decision.apply(800, 0).cache_read_input_tokens, 0);
    }
}
