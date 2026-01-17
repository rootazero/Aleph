//! Model Routing Rules
//!
//! This module defines routing rules that map task types and capabilities
//! to specific model profiles.

use super::Capability;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Cost optimization strategy for model selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CostStrategy {
    /// Always choose the cheapest model that can handle the task
    Cheapest,
    /// Balance cost and quality (default)
    #[default]
    Balanced,
    /// Always choose the best quality model regardless of cost
    BestQuality,
}

impl CostStrategy {
    /// Get human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            CostStrategy::Cheapest => "Cheapest",
            CostStrategy::Balanced => "Balanced",
            CostStrategy::BestQuality => "Best Quality",
        }
    }

    /// Get all available strategies
    pub fn all() -> &'static [CostStrategy] {
        &[
            CostStrategy::Cheapest,
            CostStrategy::Balanced,
            CostStrategy::BestQuality,
        ]
    }
}

impl std::fmt::Display for CostStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Model routing rules configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoutingRules {
    /// Task type to model profile ID mapping
    /// Key: task type string (e.g., "code_generation", "image_analysis")
    /// Value: model profile ID (e.g., "claude-opus", "gpt-4o")
    #[serde(default)]
    pub task_type_mappings: HashMap<String, String>,

    /// Capability to model profile ID mapping (fallback when no task type match)
    /// Used when task type is not explicitly mapped
    #[serde(default)]
    pub capability_mappings: HashMap<Capability, String>,

    /// Cost optimization strategy for tie-breaking
    #[serde(default)]
    pub cost_strategy: CostStrategy,

    /// Default model profile ID when no rule matches
    #[serde(default)]
    pub default_model: Option<String>,

    /// Enable multi-model pipeline execution
    #[serde(default = "default_enable_pipelines")]
    pub enable_pipelines: bool,
}

fn default_enable_pipelines() -> bool {
    true
}

impl Default for ModelRoutingRules {
    fn default() -> Self {
        Self {
            task_type_mappings: HashMap::new(),
            capability_mappings: HashMap::new(),
            cost_strategy: CostStrategy::default(),
            default_model: None,
            enable_pipelines: true,
        }
    }
}

impl ModelRoutingRules {
    /// Create new routing rules with a default model
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            default_model: Some(default_model.into()),
            ..Default::default()
        }
    }

    /// Builder method to add task type mapping
    pub fn with_task_type(
        mut self,
        task_type: impl Into<String>,
        model_id: impl Into<String>,
    ) -> Self {
        self.task_type_mappings
            .insert(task_type.into(), model_id.into());
        self
    }

    /// Builder method to add capability mapping
    pub fn with_capability(mut self, capability: Capability, model_id: impl Into<String>) -> Self {
        self.capability_mappings.insert(capability, model_id.into());
        self
    }

    /// Builder method to set cost strategy
    pub fn with_cost_strategy(mut self, strategy: CostStrategy) -> Self {
        self.cost_strategy = strategy;
        self
    }

    /// Builder method to enable/disable pipelines
    pub fn with_pipelines(mut self, enabled: bool) -> Self {
        self.enable_pipelines = enabled;
        self
    }

    /// Get model profile ID for a task type
    pub fn get_for_task_type(&self, task_type: &str) -> Option<&str> {
        self.task_type_mappings.get(task_type).map(|s| s.as_str())
    }

    /// Get model profile ID for a capability
    pub fn get_for_capability(&self, capability: Capability) -> Option<&str> {
        self.capability_mappings
            .get(&capability)
            .map(|s| s.as_str())
    }

    /// Get default model profile ID
    pub fn get_default(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    /// Check if a task type has explicit mapping
    pub fn has_task_type_mapping(&self, task_type: &str) -> bool {
        self.task_type_mappings.contains_key(task_type)
    }

    /// Get all configured task types
    pub fn task_types(&self) -> impl Iterator<Item = &str> {
        self.task_type_mappings.keys().map(|s| s.as_str())
    }

    /// Get all configured capabilities
    pub fn capabilities(&self) -> impl Iterator<Item = &Capability> {
        self.capability_mappings.keys()
    }

    /// Validate that all model IDs reference valid profiles
    pub fn validate(&self, valid_model_ids: &[&str]) -> Result<(), ValidationError> {
        let valid_set: std::collections::HashSet<&str> = valid_model_ids.iter().copied().collect();

        // Check task type mappings
        for (task_type, model_id) in &self.task_type_mappings {
            if !valid_set.contains(model_id.as_str()) {
                return Err(ValidationError::InvalidModelReference {
                    context: format!("task type '{}'", task_type),
                    model_id: model_id.clone(),
                    available: valid_model_ids.iter().map(|s| s.to_string()).collect(),
                });
            }
        }

        // Check capability mappings
        for (capability, model_id) in &self.capability_mappings {
            if !valid_set.contains(model_id.as_str()) {
                return Err(ValidationError::InvalidModelReference {
                    context: format!("capability '{}'", capability),
                    model_id: model_id.clone(),
                    available: valid_model_ids.iter().map(|s| s.to_string()).collect(),
                });
            }
        }

        // Check default model
        if let Some(ref default) = self.default_model {
            if !valid_set.contains(default.as_str()) {
                return Err(ValidationError::InvalidModelReference {
                    context: "default_model".to_string(),
                    model_id: default.clone(),
                    available: valid_model_ids.iter().map(|s| s.to_string()).collect(),
                });
            }
        }

        Ok(())
    }
}

/// Validation error for routing rules
#[derive(Debug, Clone, thiserror::Error)]
pub enum ValidationError {
    #[error(
        "Invalid model reference in {context}: '{model_id}' not found. Available: {available:?}"
    )]
    InvalidModelReference {
        context: String,
        model_id: String,
        available: Vec<String>,
    },
}

/// Standard task type names used in routing
///
/// These constants are provided as reference values for configuration.
/// Use these when configuring `[cowork.model_routing]` in config.toml.
#[allow(unused)]
pub mod task_types {
    pub const CODE_GENERATION: &str = "code_generation";
    pub const CODE_REVIEW: &str = "code_review";
    pub const IMAGE_ANALYSIS: &str = "image_analysis";
    pub const VIDEO_UNDERSTANDING: &str = "video_understanding";
    pub const LONG_DOCUMENT: &str = "long_document";
    pub const QUICK_TASKS: &str = "quick_tasks";
    pub const PRIVACY_SENSITIVE: &str = "privacy_sensitive";
    pub const REASONING: &str = "reasoning";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_strategy_default() {
        let strategy: CostStrategy = Default::default();
        assert_eq!(strategy, CostStrategy::Balanced);
    }

    #[test]
    fn test_cost_strategy_all() {
        let all = CostStrategy::all();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&CostStrategy::Cheapest));
        assert!(all.contains(&CostStrategy::Balanced));
        assert!(all.contains(&CostStrategy::BestQuality));
    }

    #[test]
    fn test_routing_rules_builder() {
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_task_type("code_generation", "claude-opus")
            .with_task_type("image_analysis", "gpt-4o")
            .with_capability(Capability::LocalPrivacy, "ollama-llama")
            .with_cost_strategy(CostStrategy::Balanced);

        assert_eq!(rules.get_default(), Some("claude-sonnet"));
        assert_eq!(
            rules.get_for_task_type("code_generation"),
            Some("claude-opus")
        );
        assert_eq!(rules.get_for_task_type("image_analysis"), Some("gpt-4o"));
        assert_eq!(rules.get_for_task_type("unknown"), None);
        assert_eq!(
            rules.get_for_capability(Capability::LocalPrivacy),
            Some("ollama-llama")
        );
        assert_eq!(rules.cost_strategy, CostStrategy::Balanced);
    }

    #[test]
    fn test_routing_rules_has_mapping() {
        let rules =
            ModelRoutingRules::new("default").with_task_type("code_generation", "claude-opus");

        assert!(rules.has_task_type_mapping("code_generation"));
        assert!(!rules.has_task_type_mapping("unknown"));
    }

    #[test]
    fn test_routing_rules_task_types_iterator() {
        let rules = ModelRoutingRules::new("default")
            .with_task_type("code_generation", "claude-opus")
            .with_task_type("image_analysis", "gpt-4o");

        let task_types: Vec<&str> = rules.task_types().collect();
        assert_eq!(task_types.len(), 2);
        assert!(task_types.contains(&"code_generation"));
        assert!(task_types.contains(&"image_analysis"));
    }

    #[test]
    fn test_routing_rules_validation_success() {
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_task_type("code_generation", "claude-opus")
            .with_capability(Capability::LocalPrivacy, "ollama-llama");

        let valid_ids = ["claude-sonnet", "claude-opus", "ollama-llama", "gpt-4o"];
        assert!(rules.validate(&valid_ids).is_ok());
    }

    #[test]
    fn test_routing_rules_validation_invalid_task_type() {
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_task_type("code_generation", "nonexistent-model");

        let valid_ids = ["claude-sonnet", "claude-opus"];
        let result = rules.validate(&valid_ids);
        assert!(result.is_err());

        let err = result.unwrap_err();
        match err {
            ValidationError::InvalidModelReference {
                context, model_id, ..
            } => {
                assert!(context.contains("code_generation"));
                assert_eq!(model_id, "nonexistent-model");
            }
        }
    }

    #[test]
    fn test_routing_rules_validation_invalid_capability() {
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_capability(Capability::ImageUnderstanding, "nonexistent-model");

        let valid_ids = ["claude-sonnet"];
        let result = rules.validate(&valid_ids);
        assert!(result.is_err());
    }

    #[test]
    fn test_routing_rules_validation_invalid_default() {
        let rules = ModelRoutingRules::new("nonexistent-default");

        let valid_ids = ["claude-sonnet"];
        let result = rules.validate(&valid_ids);
        assert!(result.is_err());

        let err = result.unwrap_err();
        match err {
            ValidationError::InvalidModelReference { context, .. } => {
                assert!(context.contains("default_model"));
            }
        }
    }

    #[test]
    fn test_routing_rules_serialization() {
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_task_type("code_generation", "claude-opus")
            .with_cost_strategy(CostStrategy::BestQuality);

        let json = serde_json::to_string(&rules).unwrap();
        assert!(json.contains("claude-sonnet"));
        assert!(json.contains("code_generation"));
        assert!(json.contains("best_quality"));

        let parsed: ModelRoutingRules = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.get_default(), Some("claude-sonnet"));
        assert_eq!(parsed.cost_strategy, CostStrategy::BestQuality);
    }

    #[test]
    fn test_routing_rules_default_values() {
        let rules: ModelRoutingRules = Default::default();

        assert!(rules.task_type_mappings.is_empty());
        assert!(rules.capability_mappings.is_empty());
        assert_eq!(rules.cost_strategy, CostStrategy::Balanced);
        assert!(rules.default_model.is_none());
        assert!(rules.enable_pipelines);
    }

    #[test]
    fn test_routing_rules_pipelines_config() {
        let rules_enabled = ModelRoutingRules::default().with_pipelines(true);
        assert!(rules_enabled.enable_pipelines);

        let rules_disabled = ModelRoutingRules::default().with_pipelines(false);
        assert!(!rules_disabled.enable_pipelines);
    }

    #[test]
    fn test_task_type_constants() {
        use task_types::*;

        assert_eq!(CODE_GENERATION, "code_generation");
        assert_eq!(CODE_REVIEW, "code_review");
        assert_eq!(IMAGE_ANALYSIS, "image_analysis");
        assert_eq!(VIDEO_UNDERSTANDING, "video_understanding");
        assert_eq!(LONG_DOCUMENT, "long_document");
        assert_eq!(QUICK_TASKS, "quick_tasks");
        assert_eq!(PRIVACY_SENSITIVE, "privacy_sensitive");
        assert_eq!(REASONING, "reasoning");
    }

    #[test]
    fn test_capability_mapping_with_multiple() {
        let rules = ModelRoutingRules::default()
            .with_capability(Capability::CodeGeneration, "claude-opus")
            .with_capability(Capability::ImageUnderstanding, "gpt-4o")
            .with_capability(Capability::LocalPrivacy, "ollama-llama");

        let caps: Vec<_> = rules.capabilities().collect();
        assert_eq!(caps.len(), 3);
    }
}
