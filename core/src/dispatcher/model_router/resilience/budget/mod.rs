//! Budget Management for Model Router
//!
//! This module provides cost control and budget enforcement for AI model usage.
//! It tracks spending in real-time, enforces configurable limits at multiple scopes,
//! and provides visibility into budget status for UI display.

mod estimation;
mod manager;
mod types;

// Re-export all public types for backward compatibility
pub use estimation::{CostEstimate, CostEstimator, ModelPricing, PricingSource};
pub use manager::BudgetManager;
pub use types::{
    BudgetCheckResult, BudgetEnforcement, BudgetEvent, BudgetLimit, BudgetPeriod, BudgetScope,
    BudgetState,
};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::CostTier;
    use chrono::{Datelike, TimeZone, Timelike, Utc};

    #[test]
    fn test_budget_scope_priority() {
        assert!(BudgetScope::Global.priority() < BudgetScope::Project("a".into()).priority());
        assert!(
            BudgetScope::Project("a".into()).priority()
                < BudgetScope::Session("a".into()).priority()
        );
        assert!(
            BudgetScope::Session("a".into()).priority() < BudgetScope::Model("a".into()).priority()
        );
    }

    #[test]
    fn test_budget_scope_contains() {
        let global = BudgetScope::Global;
        let project = BudgetScope::project("test");
        let session = BudgetScope::session("s1");

        assert!(global.contains(&project));
        assert!(global.contains(&session));
        assert!(!project.contains(&global));
        assert!(!session.contains(&project));
    }

    #[test]
    fn test_budget_period_daily_next_reset() {
        let period = BudgetPeriod::daily_at(0);
        let now = Utc::now();
        let next = period.next_reset_from(now);

        assert!(next > now);
        assert_eq!(next.hour(), 0);
        assert_eq!(next.minute(), 0);
    }

    #[test]
    fn test_budget_period_monthly_last_day() {
        // Test February with day 31 - should clamp
        let period = BudgetPeriod::Monthly {
            reset_day: 31,
            reset_hour: 0,
        };

        let feb = Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap();
        let next = period.next_reset_from(feb);

        // Should be Feb 29 (2024 is leap year)
        assert_eq!(next.day(), 29);
    }

    #[test]
    fn test_budget_limit_builder() {
        let limit = BudgetLimit::new("daily", 10.0)
            .with_scope(BudgetScope::Global)
            .with_period(BudgetPeriod::daily())
            .with_warning_thresholds(vec![0.5, 0.8])
            .with_enforcement(BudgetEnforcement::HardBlock);

        assert_eq!(limit.id, "daily");
        assert_eq!(limit.limit_usd, 10.0);
        assert_eq!(limit.warning_thresholds, vec![0.5, 0.8]);
        assert_eq!(limit.enforcement, BudgetEnforcement::HardBlock);
    }

    #[test]
    fn test_budget_limit_calculations() {
        let limit = BudgetLimit::new("test", 10.0);

        assert!(limit.would_exceed(9.0, 2.0));
        assert!(!limit.would_exceed(5.0, 2.0));

        assert_eq!(limit.remaining(3.0), 7.0);
        assert_eq!(limit.remaining(15.0), 0.0); // Clamped to 0

        assert!((limit.used_percent(5.0) - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_budget_state_record_cost() {
        let limit = BudgetLimit::new("test", 10.0);
        let mut state = BudgetState::new(&limit);

        state.record_cost(3.0, &limit);
        assert!((state.spent_usd - 3.0).abs() < 0.001);
        assert!((state.remaining_usd - 7.0).abs() < 0.001);
        assert!((state.used_percent - 0.3).abs() < 0.001);

        state.record_cost(5.0, &limit);
        assert!((state.spent_usd - 8.0).abs() < 0.001);
    }

    #[test]
    fn test_budget_state_warnings() {
        let limit = BudgetLimit::new("test", 10.0).with_warning_thresholds(vec![0.5, 0.8]);
        let mut state = BudgetState::new(&limit);

        state.record_cost(6.0, &limit); // 60%
        let warnings = state.check_warnings(&limit.warning_thresholds);
        assert!(warnings.contains(&0.5));
        assert!(!warnings.contains(&0.8)); // 80% not yet

        state.fire_warning(0.5);
        let warnings = state.check_warnings(&limit.warning_thresholds);
        assert!(!warnings.contains(&0.5)); // Already fired
    }

    #[test]
    fn test_budget_check_result() {
        let allowed = BudgetCheckResult::Allowed { remaining_usd: 5.0 };
        assert!(allowed.is_allowed());
        assert!(!allowed.is_blocked());
        assert_eq!(allowed.remaining(), Some(5.0));

        let blocked = BudgetCheckResult::HardBlocked {
            limit_id: "test".into(),
            spent_usd: 10.0,
            limit_usd: 10.0,
        };
        assert!(!blocked.is_allowed());
        assert!(blocked.is_blocked());
    }

    #[test]
    fn test_model_pricing_from_tier() {
        let free = ModelPricing::from_cost_tier(CostTier::Free);
        assert_eq!(free.input_price_per_1m, 0.0);

        let high = ModelPricing::from_cost_tier(CostTier::High);
        assert!(high.input_price_per_1m > 0.0);
        assert!(high.output_price_per_1m > high.input_price_per_1m);
    }

    #[test]
    fn test_model_pricing_calculate() {
        let pricing = ModelPricing {
            input_price_per_1m: 3.0,
            output_price_per_1m: 15.0,
            cached_input_price_per_1m: None,
        };

        let cost = pricing.calculate_cost(1_000_000, 100_000);
        // 1M input * $3/M + 100K output * $15/M = $3 + $1.5 = $4.5
        assert!((cost - 4.5).abs() < 0.001);
    }

    #[test]
    fn test_cost_estimator_estimate() {
        let mut estimator = CostEstimator::new().with_safety_margin(1.2);

        estimator.set_pricing(
            "gpt-4o",
            ModelPricing {
                input_price_per_1m: 5.0,
                output_price_per_1m: 15.0,
                cached_input_price_per_1m: None,
            },
        );

        let estimate = estimator.estimate("gpt-4o", 1000, Some(500));
        assert!(estimate.base_cost_usd > 0.0);
        assert!(estimate.with_margin_usd > estimate.base_cost_usd);
        assert_eq!(estimate.pricing_source, PricingSource::Profile);
    }

    #[test]
    fn test_cost_estimator_default_pricing() {
        let estimator = CostEstimator::new();
        let estimate = estimator.estimate("unknown-model", 1000, Some(500));
        assert_eq!(estimate.pricing_source, PricingSource::Default);
    }

    #[tokio::test]
    async fn test_budget_manager_check() {
        let limits =
            vec![BudgetLimit::new("daily", 10.0).with_enforcement(BudgetEnforcement::SoftBlock)];

        let manager = BudgetManager::new(limits);

        let estimate = CostEstimate {
            model_id: "test".into(),
            input_tokens: 1000,
            estimated_output_tokens: 500,
            base_cost_usd: 0.01,
            with_margin_usd: 0.012,
            pricing_source: PricingSource::Default,
        };

        let result = manager.check_budget(&BudgetScope::Global, &estimate).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_budget_manager_blocked() {
        let limits = vec![BudgetLimit::new("daily", 0.01) // Very low limit
            .with_enforcement(BudgetEnforcement::HardBlock)];

        let manager = BudgetManager::new(limits);

        // First, record some cost to exceed the limit
        {
            let mut states = manager.states().write().await;
            if let Some(state) = states.get_mut("daily") {
                state.spent_usd = 0.02; // Already exceeded
            }
        }

        let estimate = CostEstimate {
            model_id: "test".into(),
            input_tokens: 1000,
            estimated_output_tokens: 500,
            base_cost_usd: 0.01,
            with_margin_usd: 0.012,
            pricing_source: PricingSource::Default,
        };

        let result = manager.check_budget(&BudgetScope::Global, &estimate).await;
        assert!(result.is_blocked());
    }

    #[tokio::test]
    async fn test_budget_manager_disabled() {
        let manager = BudgetManager::disabled();
        assert!(!manager.is_enabled());

        let estimate = CostEstimate {
            model_id: "test".into(),
            input_tokens: 1000,
            estimated_output_tokens: 500,
            base_cost_usd: 100.0, // High cost
            with_margin_usd: 120.0,
            pricing_source: PricingSource::Default,
        };

        // Should always be allowed when disabled
        let result = manager.check_budget(&BudgetScope::Global, &estimate).await;
        assert!(result.is_allowed());
    }

    #[test]
    fn test_budget_enforcement() {
        assert!(!BudgetEnforcement::WarnOnly.blocks());
        assert!(BudgetEnforcement::SoftBlock.blocks());
        assert!(BudgetEnforcement::HardBlock.blocks());
    }
}
