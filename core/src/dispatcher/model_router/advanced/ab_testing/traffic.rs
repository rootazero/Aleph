//! Traffic splitting for A/B experiments
//!
//! This module provides:
//! - TrafficSplitManager: Manages traffic splitting using consistent hashing

use super::types::{
    AssignmentStrategy, ExperimentConfig, ExperimentId, VariantAssignment, VariantConfig,
};
use crate::dispatcher::model_router::{PromptFeatures, TaskIntent};
use siphasher::sip::SipHasher24;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::SystemTime;

// ============================================================================
// Traffic Split Manager
// ============================================================================

/// Manages traffic splitting for A/B experiments using consistent hashing
pub struct TrafficSplitManager {
    /// Active experiments indexed by ID
    experiments: HashMap<ExperimentId, ExperimentConfig>,
    /// Assignment strategy
    strategy: AssignmentStrategy,
    /// Hash seed for reproducibility
    hash_seed: u64,
}

impl TrafficSplitManager {
    /// Create a new traffic split manager
    pub fn new(experiments: Vec<ExperimentConfig>, strategy: AssignmentStrategy) -> Self {
        let experiments_map = experiments.into_iter().map(|e| (e.id.clone(), e)).collect();

        Self {
            experiments: experiments_map,
            strategy,
            hash_seed: 0x517cc1b727220a95, // Deterministic seed for consistent hashing
        }
    }

    /// Create with a custom hash seed
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.hash_seed = seed;
        self
    }

    /// Get the assignment strategy
    pub fn strategy(&self) -> AssignmentStrategy {
        self.strategy
    }

    /// Set the assignment strategy
    pub fn set_strategy(&mut self, strategy: AssignmentStrategy) {
        self.strategy = strategy;
    }

    /// Add an experiment
    pub fn add_experiment(&mut self, experiment: ExperimentConfig) {
        self.experiments.insert(experiment.id.clone(), experiment);
    }

    /// Remove an experiment
    pub fn remove_experiment(&mut self, experiment_id: &str) -> Option<ExperimentConfig> {
        self.experiments.remove(&ExperimentId::from(experiment_id))
    }

    /// Get an experiment by ID
    pub fn get_experiment(&self, experiment_id: &str) -> Option<&ExperimentConfig> {
        self.experiments.get(&ExperimentId::from(experiment_id))
    }

    /// Get all experiments
    pub fn experiments(&self) -> impl Iterator<Item = &ExperimentConfig> {
        self.experiments.values()
    }

    /// Get active experiments (enabled and within time window)
    pub fn active_experiments(&self) -> impl Iterator<Item = &ExperimentConfig> {
        self.experiments.values().filter(|e| e.is_active())
    }

    /// Assign a request to an experiment variant
    ///
    /// Returns `Some(VariantAssignment)` if the request is assigned to an experiment,
    /// or `None` if the request is not in any experiment.
    pub fn assign(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        request_id: &str,
        intent: &TaskIntent,
        features: Option<&PromptFeatures>,
    ) -> Option<VariantAssignment> {
        // Determine the assignment key based on strategy
        let assignment_key = self.get_assignment_key(user_id, session_id, request_id);

        // Try each active experiment
        for experiment in self.active_experiments() {
            // Filter by target intent
            if let Some(ref target_intent) = experiment.target_intent {
                if target_intent != intent {
                    continue;
                }
            }

            // Filter by minimum complexity
            if let Some(min_complexity) = experiment.min_complexity {
                if let Some(features) = features {
                    if features.complexity_score < min_complexity {
                        continue;
                    }
                }
            }

            // Check if this request falls into the experiment's traffic sample
            if self.is_in_traffic_sample(
                &assignment_key,
                &experiment.id,
                experiment.traffic_percentage,
            ) {
                // Assign to a variant
                if let Some(variant) = self.select_variant(&assignment_key, experiment) {
                    return Some(VariantAssignment::from_configs(experiment, variant));
                }
            }
        }

        None
    }

    /// Get the assignment key based on strategy
    fn get_assignment_key(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        request_id: &str,
    ) -> String {
        match self.strategy {
            AssignmentStrategy::UserId => user_id.or(session_id).unwrap_or(request_id).to_string(),
            AssignmentStrategy::SessionId => {
                session_id.or(user_id).unwrap_or(request_id).to_string()
            }
            AssignmentStrategy::RequestId => request_id.to_string(),
        }
    }

    /// Check if assignment key falls into experiment's traffic sample
    fn is_in_traffic_sample(&self, key: &str, experiment_id: &str, traffic_percentage: u8) -> bool {
        let hash = self.compute_hash(key, experiment_id);
        let sample = (hash % 100) as u8;
        sample < traffic_percentage
    }

    /// Select a variant based on weighted distribution
    fn select_variant<'a>(
        &self,
        key: &str,
        experiment: &'a ExperimentConfig,
    ) -> Option<&'a VariantConfig> {
        let total_weight = experiment.total_weight();
        if total_weight == 0 {
            return None;
        }

        // Use a different hash for variant selection
        let hash = self.compute_hash(key, &format!("{}-variant", experiment.id));
        let bucket = (hash % total_weight as u64) as u32;

        // Find the variant that contains this bucket
        let mut cumulative = 0u32;
        for variant in &experiment.variants {
            cumulative += variant.weight;
            if bucket < cumulative {
                return Some(variant);
            }
        }

        // Fallback to first variant (shouldn't happen)
        experiment.variants.first()
    }

    /// Compute a deterministic hash for the given key and salt
    fn compute_hash(&self, key: &str, salt: &str) -> u64 {
        let mut hasher = SipHasher24::new_with_keys(self.hash_seed, 0);
        key.hash(&mut hasher);
        salt.hash(&mut hasher);
        hasher.finish()
    }

    /// Enable an experiment
    pub fn enable_experiment(&mut self, experiment_id: &str) -> bool {
        if let Some(experiment) = self.experiments.get_mut(&ExperimentId::from(experiment_id)) {
            experiment.enabled = true;
            if experiment.start_time.is_none() {
                experiment.start_time = Some(SystemTime::now());
            }
            true
        } else {
            false
        }
    }

    /// Disable an experiment
    pub fn disable_experiment(&mut self, experiment_id: &str) -> bool {
        if let Some(experiment) = self.experiments.get_mut(&ExperimentId::from(experiment_id)) {
            experiment.enabled = false;
            true
        } else {
            false
        }
    }
}

impl Default for TrafficSplitManager {
    fn default() -> Self {
        Self::new(Vec::new(), AssignmentStrategy::default())
    }
}
