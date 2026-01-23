//! Budget Manager
//!
//! Central budget management and enforcement.

use super::estimation::{CostEstimate, CostEstimator};
use super::types::{
    BudgetCheckResult, BudgetEnforcement, BudgetEvent, BudgetLimit, BudgetScope, BudgetState,
};
use crate::dispatcher::model_router::CallRecord;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Central budget management
pub struct BudgetManager {
    /// Configured limits
    limits: Vec<BudgetLimit>,
    /// Current state per limit
    states: Arc<RwLock<HashMap<String, BudgetState>>>,
    /// Cost estimator
    estimator: Arc<RwLock<CostEstimator>>,
    /// Event sender
    event_tx: Option<tokio::sync::broadcast::Sender<BudgetEvent>>,
}

impl BudgetManager {
    /// Create a new budget manager with limits
    pub fn new(limits: Vec<BudgetLimit>) -> Self {
        let states: HashMap<String, BudgetState> = limits
            .iter()
            .map(|l| (l.id.clone(), BudgetState::new(l)))
            .collect();

        let (event_tx, _) = tokio::sync::broadcast::channel(100);

        Self {
            limits,
            states: Arc::new(RwLock::new(states)),
            estimator: Arc::new(RwLock::new(CostEstimator::new())),
            event_tx: Some(event_tx),
        }
    }

    /// Create with no limits (disabled)
    pub fn disabled() -> Self {
        Self {
            limits: Vec::new(),
            states: Arc::new(RwLock::new(HashMap::new())),
            estimator: Arc::new(RwLock::new(CostEstimator::new())),
            event_tx: None,
        }
    }

    /// Check if budget management is enabled
    pub fn is_enabled(&self) -> bool {
        !self.limits.is_empty()
    }

    /// Subscribe to budget events
    pub fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<BudgetEvent>> {
        self.event_tx.as_ref().map(|tx| tx.subscribe())
    }

    /// Set cost estimator
    pub async fn set_estimator(&self, estimator: CostEstimator) {
        let mut est = self.estimator.write().await;
        *est = estimator;
    }

    /// Get cost estimator reference
    pub async fn estimator(&self) -> CostEstimator {
        self.estimator.read().await.clone()
    }

    /// Check budget before execution
    pub async fn check_budget(
        &self,
        scope: &BudgetScope,
        estimate: &CostEstimate,
    ) -> BudgetCheckResult {
        if self.limits.is_empty() {
            return BudgetCheckResult::Allowed {
                remaining_usd: f64::MAX,
            };
        }

        // Check for resets first
        self.check_and_apply_resets().await;

        let states = self.states.read().await;
        let mut min_remaining = f64::MAX;
        let mut warning_to_fire: Option<(String, f64, f64, f64)> = None;

        for limit in &self.limits {
            // Check if limit applies to this scope
            if !limit.scope.contains(scope) && limit.scope != BudgetScope::Global {
                continue;
            }

            if let Some(state) = states.get(&limit.id) {
                let would_spend = state.spent_usd + estimate.cost();

                // Check if would exceed limit
                if would_spend > limit.limit_usd {
                    match limit.enforcement {
                        BudgetEnforcement::HardBlock => {
                            return BudgetCheckResult::HardBlocked {
                                limit_id: limit.id.clone(),
                                spent_usd: state.spent_usd,
                                limit_usd: limit.limit_usd,
                            };
                        }
                        BudgetEnforcement::SoftBlock => {
                            return BudgetCheckResult::SoftBlocked {
                                limit_id: limit.id.clone(),
                                spent_usd: state.spent_usd,
                                limit_usd: limit.limit_usd,
                            };
                        }
                        BudgetEnforcement::WarnOnly => {
                            // Continue checking, will return warning
                        }
                    }
                }

                // Check warning thresholds
                let new_percent = would_spend / limit.limit_usd;
                for &threshold in &limit.warning_thresholds {
                    if new_percent >= threshold && !state.warnings_fired.contains(&threshold) {
                        warning_to_fire =
                            Some((limit.id.clone(), threshold, would_spend, limit.limit_usd));
                        break;
                    }
                }

                // Track minimum remaining
                let remaining = limit.limit_usd - would_spend;
                if remaining < min_remaining {
                    min_remaining = remaining;
                }
            }
        }

        // Return warning if any threshold would be crossed
        if let Some((limit_id, threshold, spent, limit)) = warning_to_fire {
            return BudgetCheckResult::Warning {
                threshold,
                remaining_usd: (limit - spent).max(0.0),
                message: format!(
                    "Budget {}% used on '{}': ${:.2}/${:.2}",
                    (threshold * 100.0) as u32,
                    limit_id,
                    spent,
                    limit
                ),
            };
        }

        BudgetCheckResult::Allowed {
            remaining_usd: min_remaining.max(0.0),
        }
    }

    /// Record actual cost after call completes
    pub async fn record_cost(&self, scope: &BudgetScope, record: &CallRecord) {
        let cost = record.cost_usd.unwrap_or_else(|| {
            // Estimate from tokens if actual cost not available
            let estimator = self.estimator.blocking_read();
            let estimate = estimator.estimate(
                &record.model_id,
                record.input_tokens,
                Some(record.output_tokens),
            );
            estimate.base_cost_usd
        });

        let mut states = self.states.write().await;

        for limit in &self.limits {
            if !limit.scope.contains(scope) && limit.scope != BudgetScope::Global {
                continue;
            }

            if let Some(state) = states.get_mut(&limit.id) {
                let _old_percent = state.used_percent;
                state.record_cost(cost, limit);

                // Check for new warning thresholds
                let new_warnings = state.check_warnings(&limit.warning_thresholds);
                for threshold in &new_warnings {
                    state.fire_warning(*threshold);

                    // Emit warning event
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(BudgetEvent::Warning {
                            limit_id: limit.id.clone(),
                            threshold: *threshold,
                            spent_usd: state.spent_usd,
                            limit_usd: limit.limit_usd,
                        });
                    }
                }

                // Emit cost recorded event
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(BudgetEvent::CostRecorded {
                        limit_id: limit.id.clone(),
                        cost_usd: cost,
                        spent_usd: state.spent_usd,
                        remaining_usd: state.remaining_usd,
                    });
                }
            }
        }

        // Update estimator with actual cost
        if record.cost_usd.is_some() {
            let mut estimator = self.estimator.write().await;
            estimator.learn_from_actual(record);
        }
    }

    /// Get current status for a scope
    pub async fn get_status(&self, scope: &BudgetScope) -> Vec<BudgetState> {
        let states = self.states.read().await;

        self.limits
            .iter()
            .filter(|l| l.scope.contains(scope) || l.scope == BudgetScope::Global)
            .filter_map(|l| states.get(&l.id).cloned())
            .collect()
    }

    /// Get all budget states
    pub async fn all_states(&self) -> HashMap<String, BudgetState> {
        self.states.read().await.clone()
    }

    /// Manually reset a limit
    pub async fn reset_limit(&self, limit_id: &str) {
        let limit = self.limits.iter().find(|l| l.id == limit_id);
        if let Some(limit) = limit {
            let mut states = self.states.write().await;
            if let Some(state) = states.get_mut(limit_id) {
                state.reset(limit);

                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(BudgetEvent::Reset {
                        limit_id: limit_id.to_string(),
                    });
                }
            }
        }
    }

    /// Convenience method: estimate cost for a call
    pub fn estimate_cost(
        &self,
        model_id: &str,
        input_tokens: u32,
        estimated_output_tokens: u32,
    ) -> CostEstimate {
        let estimator = self.estimator.blocking_read();
        estimator.estimate(model_id, input_tokens, Some(estimated_output_tokens))
    }

    /// Convenience method: record cost directly without a CallRecord
    pub async fn record_cost_direct(&self, scope: &BudgetScope, cost_usd: f64) {
        let mut states = self.states.write().await;

        for limit in &self.limits {
            if !limit.scope.contains(scope) && limit.scope != BudgetScope::Global {
                continue;
            }

            if let Some(state) = states.get_mut(&limit.id) {
                state.record_cost(cost_usd, limit);

                // Check for new warning thresholds
                let new_warnings = state.check_warnings(&limit.warning_thresholds);
                for threshold in &new_warnings {
                    state.fire_warning(*threshold);

                    // Emit warning event
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(BudgetEvent::Warning {
                            limit_id: limit.id.clone(),
                            threshold: *threshold,
                            spent_usd: state.spent_usd,
                            limit_usd: limit.limit_usd,
                        });
                    }
                }

                // Emit cost recorded event
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(BudgetEvent::CostRecorded {
                        limit_id: limit.id.clone(),
                        cost_usd,
                        spent_usd: state.spent_usd,
                        remaining_usd: state.remaining_usd,
                    });
                }
            }
        }
    }

    /// Record actual cost after call completes using a CallRecord
    pub async fn record_cost_from_call(&self, scope: &BudgetScope, record: &CallRecord) {
        let cost = record.cost_usd.unwrap_or_else(|| {
            // Estimate from tokens if actual cost not available
            let estimator = self.estimator.blocking_read();
            let estimate = estimator.estimate(
                &record.model_id,
                record.input_tokens,
                Some(record.output_tokens),
            );
            estimate.base_cost_usd
        });

        self.record_cost_direct(scope, cost).await;

        // Update estimator with actual cost
        if record.cost_usd.is_some() {
            let mut estimator = self.estimator.write().await;
            estimator.learn_from_actual(record);
        }
    }

    /// Check and apply any due resets
    async fn check_and_apply_resets(&self) {
        let mut states = self.states.write().await;

        for limit in &self.limits {
            if let Some(state) = states.get_mut(&limit.id) {
                if state.needs_reset() {
                    state.reset(limit);

                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(BudgetEvent::Reset {
                            limit_id: limit.id.clone(),
                        });
                    }
                }
            }
        }
    }

    /// Get states for testing
    #[cfg(test)]
    pub(crate) fn states(&self) -> &Arc<RwLock<HashMap<String, BudgetState>>> {
        &self.states
    }
}
