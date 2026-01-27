//! Model routing configuration
//!
//! Contains ModelRoutingConfigToml for defining how tasks are routed
//! to different AI models based on task type, capabilities, and cost.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::agent_loop::ModelRoutingConfig as AgentLoopModelRoutingConfig;
use crate::dispatcher::model_router::{Capability, CostStrategy, ModelRoutingRules};

use super::ab_testing::ABTestingConfigToml;
use super::ensemble::EnsembleConfigToml;
use super::health::HealthConfigToml;
use super::metrics::MetricsConfigToml;
use super::prompt_analysis::PromptAnalysisConfigToml;
use super::semantic_cache::SemanticCacheConfigToml;

// =============================================================================
// ModelRoutingConfigToml
// =============================================================================

/// Model routing configuration from TOML
///
/// Defines how tasks are routed to different AI models based on task type,
/// required capabilities, and cost optimization strategy.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing]
/// code_generation = "claude-opus"
/// code_review = "claude-sonnet"
/// image_analysis = "gpt-4o"
/// video_understanding = "gemini-pro"
/// long_document = "gemini-pro"
/// quick_tasks = "claude-haiku"
/// privacy_sensitive = "ollama-llama"
/// reasoning = "claude-opus"
/// cost_strategy = "balanced"
/// enable_pipelines = true
/// default_model = "claude-sonnet"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoutingConfigToml {
    /// Model for code generation tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_generation: Option<String>,

    /// Model for code review tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_review: Option<String>,

    /// Model for image analysis tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_analysis: Option<String>,

    /// Model for video understanding tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_understanding: Option<String>,

    /// Model for long document processing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_document: Option<String>,

    /// Model for quick/simple tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_tasks: Option<String>,

    /// Model for privacy-sensitive tasks (should be local)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_sensitive: Option<String>,

    /// Model for complex reasoning tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    /// Cost optimization strategy
    #[serde(default)]
    pub cost_strategy: CostStrategy,

    /// Enable multi-model pipeline execution
    #[serde(default = "default_enable_pipelines")]
    pub enable_pipelines: bool,

    /// Default model when no specific routing rule matches
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// User overrides for specific task types
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub overrides: HashMap<String, String>,

    /// Metrics collection configuration
    #[serde(default)]
    pub metrics: MetricsConfigToml,

    /// Health check configuration
    #[serde(default)]
    pub health: HealthConfigToml,

    /// Retry and failover configuration (P1 improvements)
    #[serde(default)]
    pub retry: crate::config::types::dispatcher::RetryConfigToml,

    /// Budget management configuration (P1 improvements)
    #[serde(default)]
    pub budget: crate::config::types::dispatcher::BudgetConfigToml,

    /// Prompt analysis configuration (P2 improvements)
    #[serde(default)]
    pub prompt_analysis: PromptAnalysisConfigToml,

    /// Semantic cache configuration (P2 improvements)
    #[serde(default)]
    pub semantic_cache: SemanticCacheConfigToml,

    /// A/B testing configuration (P3 improvements)
    #[serde(default)]
    pub ab_testing: ABTestingConfigToml,

    /// Multi-model ensemble configuration (P3 improvements)
    #[serde(default)]
    pub ensemble: EnsembleConfigToml,
}

fn default_enable_pipelines() -> bool {
    true
}

impl Default for ModelRoutingConfigToml {
    fn default() -> Self {
        Self {
            code_generation: None,
            code_review: None,
            image_analysis: None,
            video_understanding: None,
            long_document: None,
            quick_tasks: None,
            privacy_sensitive: None,
            reasoning: None,
            cost_strategy: CostStrategy::default(),
            enable_pipelines: true,
            default_model: None,
            overrides: HashMap::new(),
            metrics: MetricsConfigToml::default(),
            health: HealthConfigToml::default(),
            retry: crate::config::types::dispatcher::RetryConfigToml::default(),
            budget: crate::config::types::dispatcher::BudgetConfigToml::default(),
            prompt_analysis: PromptAnalysisConfigToml::default(),
            semantic_cache: SemanticCacheConfigToml::default(),
            ab_testing: ABTestingConfigToml::default(),
            ensemble: EnsembleConfigToml::default(),
        }
    }
}

impl ModelRoutingConfigToml {
    /// Convert to ModelRoutingRules
    #[allow(clippy::field_reassign_with_default)]
    pub fn to_routing_rules(&self) -> ModelRoutingRules {
        let mut rules = ModelRoutingRules::default();

        // Set cost strategy
        rules.cost_strategy = self.cost_strategy;
        rules.enable_pipelines = self.enable_pipelines;
        rules.default_model = self.default_model.clone();

        // Add task type mappings
        if let Some(ref model) = self.code_generation {
            rules
                .task_type_mappings
                .insert("code_generation".to_string(), model.clone());
        }
        if let Some(ref model) = self.code_review {
            rules
                .task_type_mappings
                .insert("code_review".to_string(), model.clone());
        }
        if let Some(ref model) = self.image_analysis {
            rules
                .task_type_mappings
                .insert("image_analysis".to_string(), model.clone());
        }
        if let Some(ref model) = self.video_understanding {
            rules
                .task_type_mappings
                .insert("video_understanding".to_string(), model.clone());
        }
        if let Some(ref model) = self.long_document {
            rules
                .task_type_mappings
                .insert("long_document".to_string(), model.clone());
        }
        if let Some(ref model) = self.quick_tasks {
            rules
                .task_type_mappings
                .insert("quick_tasks".to_string(), model.clone());
        }
        if let Some(ref model) = self.privacy_sensitive {
            rules
                .task_type_mappings
                .insert("privacy_sensitive".to_string(), model.clone());
        }
        if let Some(ref model) = self.reasoning {
            rules
                .task_type_mappings
                .insert("reasoning".to_string(), model.clone());
        }

        // Add user overrides
        for (task_type, model) in &self.overrides {
            rules
                .task_type_mappings
                .insert(task_type.clone(), model.clone());
        }

        // Add capability mappings based on task types
        if let Some(ref model) = self.code_generation {
            rules
                .capability_mappings
                .insert(Capability::CodeGeneration, model.clone());
        }
        if let Some(ref model) = self.code_review {
            rules
                .capability_mappings
                .insert(Capability::CodeReview, model.clone());
        }
        if let Some(ref model) = self.image_analysis {
            rules
                .capability_mappings
                .insert(Capability::ImageUnderstanding, model.clone());
        }
        if let Some(ref model) = self.video_understanding {
            rules
                .capability_mappings
                .insert(Capability::VideoUnderstanding, model.clone());
        }
        if let Some(ref model) = self.long_document {
            rules
                .capability_mappings
                .insert(Capability::LongDocument, model.clone());
        }
        if let Some(ref model) = self.quick_tasks {
            rules
                .capability_mappings
                .insert(Capability::FastResponse, model.clone());
        }
        if let Some(ref model) = self.privacy_sensitive {
            rules
                .capability_mappings
                .insert(Capability::LocalPrivacy, model.clone());
        }
        if let Some(ref model) = self.reasoning {
            rules
                .capability_mappings
                .insert(Capability::Reasoning, model.clone());
        }

        rules
    }

    /// Validate routing configuration against available model profiles
    pub fn validate(&self, available_profiles: &[&str]) -> Result<(), String> {
        let profile_set: std::collections::HashSet<&str> =
            available_profiles.iter().copied().collect();

        // Helper to validate a model reference
        let validate_model = |model: &Option<String>, field: &str| -> Result<(), String> {
            if let Some(ref model_id) = model {
                if !profile_set.contains(model_id.as_str()) {
                    return Err(format!(
                        "agent.model_routing.{} references unknown profile '{}'. Available: {:?}",
                        field, model_id, available_profiles
                    ));
                }
            }
            Ok(())
        };

        // Validate all model references
        validate_model(&self.code_generation, "code_generation")?;
        validate_model(&self.code_review, "code_review")?;
        validate_model(&self.image_analysis, "image_analysis")?;
        validate_model(&self.video_understanding, "video_understanding")?;
        validate_model(&self.long_document, "long_document")?;
        validate_model(&self.quick_tasks, "quick_tasks")?;
        validate_model(&self.privacy_sensitive, "privacy_sensitive")?;
        validate_model(&self.reasoning, "reasoning")?;
        validate_model(&self.default_model, "default_model")?;

        // Validate overrides
        for (task_type, model_id) in &self.overrides {
            if !profile_set.contains(model_id.as_str()) {
                return Err(format!(
                    "agent.model_routing.overrides.{} references unknown profile '{}'. Available: {:?}",
                    task_type, model_id, available_profiles
                ));
            }
        }

        // Validate metrics configuration
        self.metrics.validate()?;

        // Validate health configuration
        self.health.validate()?;

        // Validate retry configuration (P1 improvements)
        self.retry.validate()?;

        // Validate budget configuration (P1 improvements)
        self.budget.validate()?;

        // Validate prompt analysis configuration (P2 improvements)
        self.prompt_analysis.validate()?;

        // Validate semantic cache configuration (P2 improvements)
        self.semantic_cache.validate()?;

        // Validate A/B testing configuration (P3 improvements)
        self.ab_testing.validate(available_profiles)?;

        // Validate ensemble configuration (P3 improvements)
        self.ensemble.validate(available_profiles)?;

        Ok(())
    }

    /// Get all model IDs referenced in routing config
    pub fn referenced_model_ids(&self) -> Vec<&str> {
        let mut ids = Vec::new();

        if let Some(ref m) = self.code_generation {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.code_review {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.image_analysis {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.video_understanding {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.long_document {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.quick_tasks {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.privacy_sensitive {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.reasoning {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.default_model {
            ids.push(m.as_str());
        }

        for m in self.overrides.values() {
            ids.push(m.as_str());
        }

        ids
    }

    /// Convert to agent_loop::ModelRoutingConfig for simple routing scenarios
    ///
    /// This bridges the gap between the full TOML configuration and the
    /// simplified agent loop model routing. Maps:
    /// - `default_model` or first available model → `default_model`
    /// - `image_analysis` → `vision_model`
    /// - `reasoning` → `reasoning_model`
    /// - `quick_tasks` → `fast_model`
    pub fn to_agent_loop_config(&self) -> AgentLoopModelRoutingConfig {
        // Default model names from agent_loop::config
        let default_model_name = "claude-sonnet-4-20250514".to_string();
        let default_fast_model = "claude-3-5-haiku-20241022".to_string();

        AgentLoopModelRoutingConfig {
            default_model: self
                .default_model
                .clone()
                .or_else(|| self.code_generation.clone())
                .unwrap_or_else(|| default_model_name.clone()),
            vision_model: self
                .image_analysis
                .clone()
                .unwrap_or_else(|| default_model_name.clone()),
            reasoning_model: self
                .reasoning
                .clone()
                .unwrap_or_else(|| default_model_name.clone()),
            fast_model: self
                .quick_tasks
                .clone()
                .unwrap_or(default_fast_model),
            auto_route: self.enable_pipelines,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_routing_config_default() {
        let config = ModelRoutingConfigToml::default();
        assert!(config.code_generation.is_none());
        assert!(config.default_model.is_none());
        assert_eq!(config.cost_strategy, CostStrategy::Balanced);
        assert!(config.enable_pipelines);
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_model_routing_config_to_rules() {
        let config = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            code_review: Some("claude-sonnet".to_string()),
            image_analysis: Some("gpt-4o".to_string()),
            video_understanding: None,
            long_document: None,
            quick_tasks: Some("claude-haiku".to_string()),
            privacy_sensitive: Some("ollama-llama".to_string()),
            reasoning: None,
            cost_strategy: CostStrategy::Balanced,
            enable_pipelines: true,
            default_model: Some("claude-sonnet".to_string()),
            overrides: HashMap::new(),
            ..Default::default()
        };

        let rules = config.to_routing_rules();
        assert_eq!(
            rules.get_for_task_type("code_generation"),
            Some("claude-opus")
        );
        assert_eq!(
            rules.get_for_task_type("code_review"),
            Some("claude-sonnet")
        );
        assert_eq!(rules.get_for_task_type("image_analysis"), Some("gpt-4o"));
        assert_eq!(rules.get_for_task_type("quick_tasks"), Some("claude-haiku"));
        assert_eq!(rules.get_default(), Some("claude-sonnet"));
        assert_eq!(rules.cost_strategy, CostStrategy::Balanced);
        assert!(rules.enable_pipelines);
    }

    #[test]
    fn test_model_routing_config_with_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("code_generation".to_string(), "gpt-4-turbo".to_string());

        let config = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            overrides,
            ..Default::default()
        };

        let rules = config.to_routing_rules();
        // Override should win
        assert_eq!(
            rules.get_for_task_type("code_generation"),
            Some("gpt-4-turbo")
        );
    }

    #[test]
    fn test_model_routing_config_validation() {
        let available = ["claude-opus", "claude-sonnet", "gpt-4o"];

        // Valid config
        let valid = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            default_model: Some("claude-sonnet".to_string()),
            ..Default::default()
        };
        assert!(valid.validate(&available).is_ok());

        // Invalid profile reference
        let invalid = ModelRoutingConfigToml {
            code_generation: Some("nonexistent-model".to_string()),
            ..Default::default()
        };
        assert!(invalid.validate(&available).is_err());

        // Invalid default model
        let invalid_default = ModelRoutingConfigToml {
            default_model: Some("nonexistent-model".to_string()),
            ..Default::default()
        };
        assert!(invalid_default.validate(&available).is_err());
    }

    #[test]
    fn test_model_routing_referenced_ids() {
        let config = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            image_analysis: Some("gpt-4o".to_string()),
            default_model: Some("claude-sonnet".to_string()),
            ..Default::default()
        };

        let ids = config.referenced_model_ids();
        assert!(ids.contains(&"claude-opus"));
        assert!(ids.contains(&"gpt-4o"));
        assert!(ids.contains(&"claude-sonnet"));
    }

    #[test]
    fn test_model_routing_toml_deserialization() {
        let toml_str = r#"
            code_generation = "claude-opus"
            code_review = "claude-sonnet"
            image_analysis = "gpt-4o"
            cost_strategy = "balanced"
            enable_pipelines = true
            default_model = "claude-sonnet"
        "#;

        let config: ModelRoutingConfigToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.code_generation, Some("claude-opus".to_string()));
        assert_eq!(config.code_review, Some("claude-sonnet".to_string()));
        assert_eq!(config.image_analysis, Some("gpt-4o".to_string()));
        assert_eq!(config.cost_strategy, CostStrategy::Balanced);
        assert!(config.enable_pipelines);
        assert_eq!(config.default_model, Some("claude-sonnet".to_string()));
    }

    #[test]
    fn test_model_routing_with_retry_budget_deserialization() {
        let toml_str = r#"
            code_generation = "claude-opus"
            default_model = "claude-sonnet"

            [retry]
            enabled = true
            max_attempts = 3
            attempt_timeout_ms = 300000
            total_timeout_ms = 120000
            failover_on_non_retryable = true
            retryable_errors = ["rate_limit", "timeout", "server_error"]

            [retry.backoff]
            strategy = "exponential_jitter"
            initial_ms = 1000
            max_ms = 30000
            multiplier = 2.0
            jitter_factor = 0.1

            [budget]
            enabled = true
            default_enforcement = "warn_only"
            estimation_safety_margin = 1.2

            [[budget.limits]]
            id = "daily-global"
            scope = "global"
            period = "daily"
            reset_hour = 0
            limit_usd = 10.0
            warning_thresholds = [0.5, 0.8, 0.95]
            enforcement = "soft_block"

            [[budget.limits]]
            id = "monthly-project"
            scope = "project"
            scope_value = "aether"
            period = "monthly"
            reset_day = 1
            reset_hour = 0
            limit_usd = 100.0
            warning_thresholds = [0.7, 0.9]
        "#;

        let config: ModelRoutingConfigToml = toml::from_str(toml_str).unwrap();

        // Verify basic routing config
        assert_eq!(config.code_generation, Some("claude-opus".to_string()));
        assert_eq!(config.default_model, Some("claude-sonnet".to_string()));

        // Verify retry config
        assert!(config.retry.enabled);
        assert_eq!(config.retry.max_attempts, 3);
        assert_eq!(config.retry.attempt_timeout_ms, 30000);
        assert!(config.retry.failover_on_non_retryable);
        assert_eq!(config.retry.retryable_errors.len(), 3);
        assert!(config
            .retry
            .retryable_errors
            .contains(&"rate_limit".to_string()));
        assert_eq!(config.retry.backoff.strategy, "exponential_jitter");
        assert_eq!(config.retry.backoff.multiplier, 2.0);

        // Verify budget config
        assert!(config.budget.enabled);
        assert_eq!(config.budget.default_enforcement, "warn_only");
        assert!((config.budget.estimation_safety_margin - 1.2).abs() < 0.001);
        assert_eq!(config.budget.limits.len(), 2);

        // Verify first budget limit
        let limit1 = &config.budget.limits[0];
        assert_eq!(limit1.id, "daily-global");
        assert_eq!(limit1.scope, "global");
        assert_eq!(limit1.period, "daily");
        assert!((limit1.limit_usd - 10.0).abs() < 0.001);
        assert_eq!(limit1.warning_thresholds.len(), 3);
        assert_eq!(limit1.enforcement, Some("soft_block".to_string()));

        // Verify second budget limit
        let limit2 = &config.budget.limits[1];
        assert_eq!(limit2.id, "monthly-project");
        assert_eq!(limit2.scope, "project");
        assert_eq!(limit2.scope_value, Some("aether".to_string()));
        assert_eq!(limit2.period, "monthly");
        assert!((limit2.limit_usd - 100.0).abs() < 0.001);
    }

    #[test]
    fn test_model_routing_retry_budget_validation() {
        let mut config = ModelRoutingConfigToml::default();

        // Default config should be valid
        let available_profiles: Vec<&str> = vec![];
        assert!(config.validate(&available_profiles).is_ok());

        // Invalid retry config (max_attempts = 0)
        config.retry.max_attempts = 0;
        assert!(config.validate(&available_profiles).is_err());
        config.retry.max_attempts = 3; // Reset

        // Invalid budget config (negative limit)
        let mut limit = crate::config::BudgetLimitConfigToml::default();
        limit.id = "test".to_string();
        limit.limit_usd = -10.0;
        config.budget.limits.push(limit);
        assert!(config.validate(&available_profiles).is_err());
    }

    #[test]
    fn test_model_routing_to_budget_limit() {
        let mut config = ModelRoutingConfigToml::default();
        config.budget.enabled = true;
        config.budget.default_enforcement = "warn_only".to_string();

        let mut limit_config = crate::config::BudgetLimitConfigToml::default();
        limit_config.id = "test-limit".to_string();
        limit_config.scope = "global".to_string();
        limit_config.period = "daily".to_string();
        limit_config.limit_usd = 50.0;
        limit_config.warning_thresholds = vec![0.8, 0.95];
        config.budget.limits.push(limit_config);

        let limit = config.budget.limits[0].to_budget_limit(&config.budget.default_enforcement);

        assert_eq!(limit.id, "test-limit");
        assert_eq!(
            limit.scope,
            crate::dispatcher::model_router::BudgetScope::Global
        );
        assert!((limit.limit_usd - 50.0).abs() < 0.001);
        assert_eq!(limit.warning_thresholds.len(), 2);
        assert_eq!(
            limit.enforcement,
            crate::dispatcher::model_router::BudgetEnforcement::WarnOnly
        );
    }
}
