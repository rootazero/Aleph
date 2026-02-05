use std::collections::HashMap;
use crate::agents::sub_agents::Lane;

/// Configuration for a single lane
#[derive(Debug, Clone)]
pub struct LaneQuota {
    pub max_concurrent: usize,
    pub token_budget_per_min: u64,  // 0 = unlimited
    pub priority: i8,
}

impl LaneQuota {
    pub fn new(max_concurrent: usize, priority: i8) -> Self {
        Self {
            max_concurrent,
            token_budget_per_min: 0,
            priority,
        }
    }

    pub fn with_token_budget(mut self, budget: u64) -> Self {
        self.token_budget_per_min = budget;
        self
    }
}

/// Global lane scheduler configuration
#[derive(Debug, Clone)]
pub struct LaneConfig {
    pub quotas: HashMap<Lane, LaneQuota>,
    pub global_max_concurrent: usize,
    pub anti_starvation_threshold_ms: u64,
    pub max_recursion_depth: usize,
    pub priority_boost_per_30s: i8,
}

impl Default for LaneConfig {
    fn default() -> Self {
        let mut quotas = HashMap::new();
        quotas.insert(Lane::Main, LaneQuota::new(2, 10));
        quotas.insert(Lane::Nested, LaneQuota::new(4, 8).with_token_budget(200_000));
        quotas.insert(Lane::Subagent, LaneQuota::new(8, 5).with_token_budget(500_000));
        quotas.insert(Lane::Cron, LaneQuota::new(2, 0).with_token_budget(100_000));

        Self {
            quotas,
            global_max_concurrent: 16,
            anti_starvation_threshold_ms: 30_000,
            max_recursion_depth: 5,
            priority_boost_per_30s: 1,
        }
    }
}

impl LaneConfig {
    pub fn get_quota(&self, lane: &Lane) -> Option<&LaneQuota> {
        self.quotas.get(lane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_lane_config() {
        let config = LaneConfig::default();
        assert_eq!(config.global_max_concurrent, 16);
        assert_eq!(config.max_recursion_depth, 5);
        assert_eq!(config.anti_starvation_threshold_ms, 30_000);
        assert_eq!(config.priority_boost_per_30s, 1);
    }

    #[test]
    fn test_default_quotas() {
        let config = LaneConfig::default();

        // Main lane: 2 concurrent, unlimited tokens, priority 10
        let main_quota = config.get_quota(&Lane::Main).unwrap();
        assert_eq!(main_quota.max_concurrent, 2);
        assert_eq!(main_quota.token_budget_per_min, 0);
        assert_eq!(main_quota.priority, 10);

        // Subagent lane: 8 concurrent, 500k tokens/min, priority 5
        let subagent_quota = config.get_quota(&Lane::Subagent).unwrap();
        assert_eq!(subagent_quota.max_concurrent, 8);
        assert_eq!(subagent_quota.token_budget_per_min, 500_000);
        assert_eq!(subagent_quota.priority, 5);

        // Cron lane: 2 concurrent, 100k tokens/min, priority 0
        let cron_quota = config.get_quota(&Lane::Cron).unwrap();
        assert_eq!(cron_quota.max_concurrent, 2);
        assert_eq!(cron_quota.token_budget_per_min, 100_000);
        assert_eq!(cron_quota.priority, 0);

        // Nested lane: 4 concurrent, 200k tokens/min, priority 8
        let nested_quota = config.get_quota(&Lane::Nested).unwrap();
        assert_eq!(nested_quota.max_concurrent, 4);
        assert_eq!(nested_quota.token_budget_per_min, 200_000);
        assert_eq!(nested_quota.priority, 8);
    }

    #[test]
    fn test_get_quota_with_fallback() {
        let config = LaneConfig::default();

        // Existing lane
        assert!(config.get_quota(&Lane::Main).is_some());

        // All lanes should be present in default config
        assert!(config.get_quota(&Lane::Subagent).is_some());
        assert!(config.get_quota(&Lane::Cron).is_some());
        assert!(config.get_quota(&Lane::Nested).is_some());
    }
}
