//! Core matching logic for model routing

use super::super::{Capability, CostStrategy, ModelProfile, ModelRoutingRules};
use super::types::{FallbackProvider, TaskHints};
use crate::dispatcher::agent_types::{Task, TaskType};
use std::collections::HashMap;

/// Model matcher that routes tasks to optimal AI models
#[derive(Clone)]
pub struct ModelMatcher {
    /// Model profiles indexed by ID
    pub(crate) profiles_by_id: HashMap<String, ModelProfile>,

    /// Model profiles as a vector (for iteration)
    pub(crate) profiles_vec: Vec<ModelProfile>,

    /// Routing rules configuration
    pub(crate) rules: ModelRoutingRules,

    /// Fallback provider when no suitable model is found
    pub(crate) fallback_provider: Option<FallbackProvider>,

    /// Profiles indexed by capability for fast lookup
    pub(crate) capability_index: HashMap<Capability, Vec<String>>,
}

impl ModelMatcher {
    /// Create a new ModelMatcher with profiles and routing rules
    pub fn new(profiles: Vec<ModelProfile>, rules: ModelRoutingRules) -> Self {
        let mut profiles_by_id = HashMap::new();
        let mut capability_index: HashMap<Capability, Vec<String>> = HashMap::new();

        // Index profiles by ID and capability
        for profile in &profiles {
            profiles_by_id.insert(profile.id.clone(), profile.clone());

            for capability in &profile.capabilities {
                capability_index
                    .entry(*capability)
                    .or_default()
                    .push(profile.id.clone());
            }
        }

        Self {
            profiles_by_id,
            profiles_vec: profiles,
            rules,
            fallback_provider: None,
            capability_index,
        }
    }

    /// Set the fallback provider for when no suitable model is found
    ///
    /// This integrates with the system's `default_provider` configuration.
    /// When all routing rules fail (no profiles, no capability match, no default_model),
    /// the fallback provider will be used to create a basic ModelProfile.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let matcher = ModelMatcher::new(profiles, rules)
    ///     .with_fallback_provider("openai");
    /// ```
    pub fn with_fallback_provider(mut self, provider: impl Into<String>) -> Self {
        self.fallback_provider = Some(FallbackProvider::new(provider));
        self
    }

    /// Set the fallback provider with a specific model
    pub fn with_fallback_provider_and_model(
        mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.fallback_provider = Some(FallbackProvider::new(provider).with_model(model));
        self
    }

    /// Set fallback provider from FallbackProvider instance
    pub fn set_fallback_provider(&mut self, fallback: Option<FallbackProvider>) {
        self.fallback_provider = fallback;
    }

    /// Get the fallback provider
    pub fn fallback_provider(&self) -> Option<&FallbackProvider> {
        self.fallback_provider.as_ref()
    }

    /// Check if a fallback provider is configured
    pub fn has_fallback(&self) -> bool {
        self.fallback_provider.is_some()
    }

    /// Get the routing rules
    pub fn rules(&self) -> &ModelRoutingRules {
        &self.rules
    }

    /// Extract routing hints from a task
    #[allow(clippy::field_reassign_with_default)]
    pub(crate) fn extract_task_hints(&self, task: &Task) -> TaskHints {
        let mut hints = TaskHints::default();

        // Check for explicit model preference
        hints.model_preference = task.model_preference.clone();

        // Extract hints from task type
        match &task.task_type {
            TaskType::AiInference(ai_task) => {
                hints.requires_privacy = ai_task.requires_privacy;
                hints.has_images = ai_task.has_images;

                // Check prompt length for long context hint
                if ai_task.prompt.len() > 50_000 {
                    hints.needs_long_context = true;
                }
            }
            TaskType::CodeExecution(_) => {
                hints.task_type_hint = Some("code_generation".to_string());
            }
            TaskType::DocumentGeneration(_) => {
                hints.task_type_hint = Some("long_document".to_string());
            }
            _ => {}
        }

        // Check parameters for additional hints
        if let Some(obj) = task.parameters.as_object() {
            if let Some(ctx_len) = obj.get("context_length").and_then(|v| v.as_u64()) {
                if ctx_len > 100_000 {
                    hints.needs_long_context = true;
                }
            }
        }

        hints
    }

    /// Route based on task type mapping
    pub(crate) fn route_by_task_type(&self, task_type: &str) -> Option<ModelProfile> {
        self.rules
            .get_for_task_type(task_type)
            .and_then(|id| self.profiles_by_id.get(id))
            .cloned()
    }

    /// Route based on capability requirements
    pub(crate) fn route_by_capability(&self, capability: Capability) -> Option<ModelProfile> {
        // First check if there's a specific mapping in rules
        if let Some(id) = self.rules.get_for_capability(capability) {
            if let Some(profile) = self.profiles_by_id.get(id) {
                return Some(profile.clone());
            }
        }

        // Fall back to finding best profile with this capability
        self.find_best_for(capability)
    }

    /// Apply cost strategy to select from multiple candidates
    pub(crate) fn apply_cost_strategy(&self, candidates: &[&ModelProfile]) -> Option<ModelProfile> {
        if candidates.is_empty() {
            return None;
        }

        match self.rules.cost_strategy {
            CostStrategy::Cheapest => candidates
                .iter()
                .min_by_key(|p| p.cost_tier)
                .map(|p| (*p).clone()),
            CostStrategy::BestQuality => {
                // Higher cost = better quality (in general)
                candidates
                    .iter()
                    .max_by_key(|p| p.cost_tier)
                    .map(|p| (*p).clone())
            }
            CostStrategy::Balanced => {
                // Prefer medium cost, then look at latency
                candidates
                    .iter()
                    .min_by(|a, b| {
                        // Score: prefer Medium cost, then Fast latency
                        let score_a = cost_balance_score(a);
                        let score_b = cost_balance_score(b);
                        score_a.cmp(&score_b)
                    })
                    .map(|p| (*p).clone())
            }
        }
    }

    /// Get default model profile
    pub(crate) fn get_default_profile(&self) -> Option<ModelProfile> {
        self.rules
            .get_default()
            .and_then(|id| self.profiles_by_id.get(id))
            .cloned()
    }

    /// Find the best model for a specific capability
    pub fn find_best_for(&self, capability: Capability) -> Option<ModelProfile> {
        // Get all profiles with this capability
        let profile_ids = self.capability_index.get(&capability)?;

        let candidates: Vec<&ModelProfile> = profile_ids
            .iter()
            .filter_map(|id| self.profiles_by_id.get(id))
            .collect();

        self.apply_cost_strategy(&candidates)
    }

    /// Find a balanced model (medium cost, medium latency)
    pub fn find_balanced(&self) -> Option<ModelProfile> {
        if self.profiles_vec.is_empty() {
            return None;
        }

        let candidates: Vec<&ModelProfile> = self.profiles_vec.iter().collect();
        self.apply_cost_strategy(&candidates)
    }

    /// Find the cheapest model with a specific capability
    pub fn find_cheapest_with(&self, capability: Capability) -> Option<ModelProfile> {
        let profile_ids = self.capability_index.get(&capability)?;

        profile_ids
            .iter()
            .filter_map(|id| self.profiles_by_id.get(id))
            .min_by_key(|p| p.cost_tier)
            .cloned()
    }
}

/// Compute a balance score for a profile (lower is better for balanced strategy)
pub(crate) fn cost_balance_score(profile: &ModelProfile) -> u32 {
    use super::super::{CostTier, LatencyTier};

    let cost_score = match profile.cost_tier {
        CostTier::Free => 2, // Slightly penalize free (may have limitations)
        CostTier::Low => 1,
        CostTier::Medium => 0, // Preferred
        CostTier::High => 2,
    };

    let latency_score = match profile.latency_tier {
        LatencyTier::Fast => 0,
        LatencyTier::Medium => 1,
        LatencyTier::Slow => 2,
    };

    cost_score + latency_score
}
