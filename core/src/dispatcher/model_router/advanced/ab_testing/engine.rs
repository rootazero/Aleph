//! Main A/B testing engine
//!
//! This module provides:
//! - ABTestingEngine: Combines traffic splitting and outcome tracking

use super::analysis::ExperimentReport;
use super::tracking::{ExperimentOutcome, OutcomeTracker};
use super::traffic::TrafficSplitManager;
use super::types::{AssignmentStrategy, ExperimentConfig, VariantAssignment};
use crate::dispatcher::model_router::{PromptFeatures, TaskIntent};

// ============================================================================
// A/B Testing Engine
// ============================================================================

/// Main A/B testing engine that combines traffic splitting and outcome tracking
pub struct ABTestingEngine {
    /// Traffic split manager for variant assignment
    split_manager: TrafficSplitManager,
    /// Outcome tracker for recording results
    outcome_tracker: OutcomeTracker,
}

impl ABTestingEngine {
    /// Create a new A/B testing engine
    pub fn new(experiments: Vec<ExperimentConfig>) -> Self {
        Self {
            split_manager: TrafficSplitManager::new(experiments, AssignmentStrategy::default()),
            outcome_tracker: OutcomeTracker::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        experiments: Vec<ExperimentConfig>,
        strategy: AssignmentStrategy,
        max_raw_outcomes: usize,
    ) -> Self {
        Self {
            split_manager: TrafficSplitManager::new(experiments, strategy),
            outcome_tracker: OutcomeTracker::new(max_raw_outcomes),
        }
    }

    /// Get the traffic split manager
    pub fn split_manager(&self) -> &TrafficSplitManager {
        &self.split_manager
    }

    /// Get mutable access to the traffic split manager
    pub fn split_manager_mut(&mut self) -> &mut TrafficSplitManager {
        &mut self.split_manager
    }

    /// Get the outcome tracker
    pub fn outcome_tracker(&self) -> &OutcomeTracker {
        &self.outcome_tracker
    }

    /// Assign a request to an experiment variant
    pub fn assign(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        request_id: &str,
        intent: &TaskIntent,
        features: Option<&PromptFeatures>,
    ) -> Option<VariantAssignment> {
        self.split_manager
            .assign(user_id, session_id, request_id, intent, features)
    }

    /// Record an experiment outcome
    pub fn record_outcome(&self, outcome: ExperimentOutcome) {
        self.outcome_tracker.record(outcome);
    }

    /// Get a report for an experiment
    pub fn get_report(&self, experiment_id: &str) -> Option<ExperimentReport> {
        let config = self.split_manager.get_experiment(experiment_id)?;
        let stats = self.outcome_tracker.get_stats(experiment_id)?;
        Some(ExperimentReport::generate(config, &stats))
    }

    /// Get reports for all experiments
    pub fn get_all_reports(&self) -> Vec<ExperimentReport> {
        self.split_manager
            .experiments()
            .map(|config| {
                let stats = self
                    .outcome_tracker
                    .get_stats(&config.id)
                    .unwrap_or_default();
                ExperimentReport::generate(config, &stats)
            })
            .collect()
    }

    /// Add an experiment
    pub fn add_experiment(&mut self, experiment: ExperimentConfig) {
        self.split_manager.add_experiment(experiment);
    }

    /// Remove an experiment
    pub fn remove_experiment(&mut self, experiment_id: &str) -> Option<ExperimentConfig> {
        self.outcome_tracker.clear_experiment(experiment_id);
        self.split_manager.remove_experiment(experiment_id)
    }

    /// Enable an experiment
    pub fn enable_experiment(&mut self, experiment_id: &str) -> bool {
        self.split_manager.enable_experiment(experiment_id)
    }

    /// Disable an experiment
    pub fn disable_experiment(&mut self, experiment_id: &str) -> bool {
        self.split_manager.disable_experiment(experiment_id)
    }

    /// Get active experiment count
    pub fn active_experiment_count(&self) -> usize {
        self.split_manager.active_experiments().count()
    }

    /// Get total experiment count
    pub fn total_experiment_count(&self) -> usize {
        self.split_manager.experiments().count()
    }
}

impl Default for ABTestingEngine {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}
