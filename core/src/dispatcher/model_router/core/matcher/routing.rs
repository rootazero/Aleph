//! ModelRouter trait and selection logic

use super::super::{Capability, ModelProfile, TaskIntent};
use super::core::ModelMatcher;
use super::types::RoutingError;
use crate::dispatcher::agent_types::Task;

/// Trait for routing tasks to AI models
pub trait ModelRouter: Send + Sync {
    /// Route a task to the optimal model profile
    fn route(&self, task: &Task) -> Result<ModelProfile, RoutingError>;

    /// Get a model profile by ID
    fn get_profile(&self, id: &str) -> Option<&ModelProfile>;

    /// Get all available model profiles
    fn profiles(&self) -> &[ModelProfile];

    /// Check if a profile supports a specific capability
    fn supports_capability(&self, profile_id: &str, capability: &Capability) -> bool;

    /// Find the best model for a specific capability
    fn find_best_for(&self, capability: Capability) -> Option<ModelProfile>;

    /// Find a balanced model (medium cost, medium latency)
    fn find_balanced(&self) -> Option<ModelProfile>;

    /// Find the cheapest model with a specific capability
    fn find_cheapest_with(&self, capability: Capability) -> Option<ModelProfile>;
}

impl ModelRouter for ModelMatcher {
    fn route(&self, task: &Task) -> Result<ModelProfile, RoutingError> {
        let hints = self.extract_task_hints(task);

        // 1. Check for explicit model preference
        if let Some(ref pref) = hints.model_preference {
            if let Some(profile) = self.profiles_by_id.get(pref) {
                return Ok(profile.clone());
            }
            // Log warning but continue with automatic routing
            tracing::warn!(
                preferred_model = pref,
                "Preferred model not found, falling back to automatic routing"
            );
        }

        // 2. Handle privacy requirement
        if hints.requires_privacy {
            if let Some(profile) = self.route_by_capability(Capability::LocalPrivacy) {
                return Ok(profile);
            }
            // Try to find any local model
            if let Some(profile) = self.profiles_vec.iter().find(|p| p.local) {
                return Ok(profile.clone());
            }
        }

        // 3. Handle image tasks
        if hints.has_images {
            if let Some(profile) = self.route_by_capability(Capability::ImageUnderstanding) {
                return Ok(profile);
            }
        }

        // 4. Handle long context tasks
        if hints.needs_long_context {
            if let Some(profile) = self.route_by_capability(Capability::LongContext) {
                return Ok(profile);
            }
        }

        // 5. Route by task type hint
        if let Some(ref type_hint) = hints.task_type_hint {
            if let Some(profile) = self.route_by_task_type(type_hint) {
                return Ok(profile);
            }
        }

        // 6. Route by task category
        let category = task.task_type.category();
        if let Some(profile) = self.route_by_task_type(category) {
            return Ok(profile);
        }

        // 7. Try to find a balanced model
        if let Some(profile) = self.find_balanced() {
            return Ok(profile);
        }

        // 8. Use default model
        if let Some(profile) = self.get_default_profile() {
            return Ok(profile);
        }

        // 9. Return first available profile
        if let Some(profile) = self.profiles_vec.first() {
            return Ok(profile.clone());
        }

        Err(RoutingError::NoModelAvailable {
            task_type: category.to_string(),
        })
    }

    fn get_profile(&self, id: &str) -> Option<&ModelProfile> {
        self.profiles_by_id.get(id)
    }

    fn profiles(&self) -> &[ModelProfile] {
        &self.profiles_vec
    }

    fn supports_capability(&self, profile_id: &str, capability: &Capability) -> bool {
        self.profiles_by_id
            .get(profile_id)
            .map(|p| p.has_capability(*capability))
            .unwrap_or(false)
    }

    fn find_best_for(&self, capability: Capability) -> Option<ModelProfile> {
        ModelMatcher::find_best_for(self, capability)
    }

    fn find_balanced(&self) -> Option<ModelProfile> {
        ModelMatcher::find_balanced(self)
    }

    fn find_cheapest_with(&self, capability: Capability) -> Option<ModelProfile> {
        ModelMatcher::find_cheapest_with(self, capability)
    }
}

// =========================================================================
// TaskIntent-based Routing (Unified Router Integration)
// =========================================================================

impl ModelMatcher {
    /// Route based on TaskIntent
    ///
    /// This method bridges the legacy routing system with the Model Router,
    /// providing a unified entry point for model selection based on user intent.
    ///
    /// # Routing Priority
    ///
    /// 1. Explicit task type mapping from `[cowork.model_routing]`
    /// 2. Capability-based routing if intent requires specific capability
    /// 3. Cost strategy application
    /// 4. Default model fallback
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let intent = TaskIntent::CodeGeneration;
    /// let profile = matcher.route_by_intent(&intent)?;
    /// // Returns profile mapped to "code_generation" in config
    /// ```
    pub fn route_by_intent(&self, intent: &TaskIntent) -> Option<ModelProfile> {
        // 1. Check for explicit task type mapping
        let task_type = intent.to_task_type();
        if let Some(profile) = self.route_by_task_type(task_type) {
            tracing::debug!(
                intent = %intent,
                task_type = task_type,
                model = %profile.id,
                "Routed by task type mapping"
            );
            return Some(profile);
        }

        // 2. Check for capability requirement
        if let Some(capability) = intent.required_capability() {
            if let Some(profile) = self.route_by_capability(capability) {
                tracing::debug!(
                    intent = %intent,
                    capability = ?capability,
                    model = %profile.id,
                    "Routed by capability requirement"
                );
                return Some(profile);
            }
        }

        // 3. Apply cost strategy to find any suitable model
        let candidates: Vec<&ModelProfile> = self.profiles_vec.iter().collect();
        if let Some(profile) = self.apply_cost_strategy(&candidates) {
            tracing::debug!(
                intent = %intent,
                model = %profile.id,
                strategy = ?self.rules.cost_strategy,
                "Routed by cost strategy"
            );
            return Some(profile);
        }

        // 4. Fall back to default model
        if let Some(profile) = self.get_default_profile() {
            tracing::debug!(
                intent = %intent,
                model = %profile.id,
                "Routed to default model"
            );
            return Some(profile);
        }

        // 5. Fall back to fallback_provider (from default_provider config)
        if let Some(fallback) = &self.fallback_provider {
            let profile = fallback.to_model_profile();
            tracing::info!(
                intent = %intent,
                provider = %fallback.provider,
                model = %profile.model,
                "Routed to fallback provider (default_provider)"
            );
            return Some(profile);
        }

        tracing::warn!(
            intent = %intent,
            has_profiles = !self.profiles_vec.is_empty(),
            has_default = self.rules.default_model.is_some(),
            has_fallback = self.fallback_provider.is_some(),
            "No suitable model found for intent"
        );
        None
    }

    /// Route by intent with explicit model preference override
    ///
    /// This allows routing rules to specify a preferred model that overrides
    /// the automatic selection.
    pub fn route_by_intent_with_preference(
        &self,
        intent: &TaskIntent,
        preferred_model: Option<&str>,
    ) -> Option<ModelProfile> {
        // Check preferred model first
        if let Some(model_id) = preferred_model {
            if let Some(profile) = self.profiles_by_id.get(model_id) {
                tracing::debug!(
                    intent = %intent,
                    model = model_id,
                    "Using preferred model override"
                );
                return Some(profile.clone());
            }
            tracing::warn!(
                intent = %intent,
                preferred = model_id,
                "Preferred model not found, falling back to automatic routing"
            );
        }

        // Fall back to intent-based routing
        self.route_by_intent(intent)
    }

    /// Get the model profile for a specific provider/model combination
    ///
    /// This is useful for migrating from legacy `provider` field to Model Router.
    pub fn find_by_provider_model(
        &self,
        provider: &str,
        model: Option<&str>,
    ) -> Option<ModelProfile> {
        self.profiles_vec
            .iter()
            .find(|p| {
                p.provider.eq_ignore_ascii_case(provider)
                    && model.is_none_or(|m| p.model.eq_ignore_ascii_case(m))
            })
            .cloned()
    }
}
