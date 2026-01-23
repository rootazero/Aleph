//! A/B testing configuration (P3)
//!
//! Contains ABTestingConfigToml, ExperimentConfigToml, and VariantConfigToml
//! for configuring A/B testing experiments for model routing decisions.

use serde::{Deserialize, Serialize};

// =============================================================================
// ABTestingConfigToml
// =============================================================================

/// A/B testing configuration from TOML
///
/// Configures A/B testing experiments for model routing decisions.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.ab_testing]
/// enabled = true
/// max_concurrent_experiments = 10
/// max_raw_outcomes = 100000
///
/// [[cowork.model_routing.ab_testing.experiments]]
/// id = "opus-vs-sonnet-code"
/// enabled = true
/// traffic_percentage = 20
/// [[cowork.model_routing.ab_testing.experiments.variants]]
/// id = "control"
/// model_override = "claude-opus"
/// weight = 50
/// [[cowork.model_routing.ab_testing.experiments.variants]]
/// id = "treatment"
/// model_override = "claude-sonnet"
/// weight = 50
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ABTestingConfigToml {
    /// Enable A/B testing
    #[serde(default = "default_ab_testing_enabled")]
    pub enabled: bool,

    /// Maximum number of concurrent experiments
    #[serde(default = "default_max_concurrent_experiments")]
    pub max_concurrent_experiments: usize,

    /// Maximum raw outcomes to retain per experiment
    #[serde(default = "default_max_raw_outcomes")]
    pub max_raw_outcomes: usize,

    /// Minimum sample size before significance testing
    #[serde(default = "default_min_sample_size")]
    pub min_sample_size: usize,

    /// Default significance level (alpha) for hypothesis testing
    #[serde(default = "default_significance_level")]
    pub significance_level: f64,

    /// Experiments configuration
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub experiments: Vec<ExperimentConfigToml>,
}

impl Default for ABTestingConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_ab_testing_enabled(),
            max_concurrent_experiments: default_max_concurrent_experiments(),
            max_raw_outcomes: default_max_raw_outcomes(),
            min_sample_size: default_min_sample_size(),
            significance_level: default_significance_level(),
            experiments: Vec::new(),
        }
    }
}

impl ABTestingConfigToml {
    /// Validate A/B testing configuration
    pub fn validate(&self, available_profiles: &[&str]) -> Result<(), String> {
        if self.max_concurrent_experiments == 0 {
            return Err("max_concurrent_experiments must be greater than 0".to_string());
        }

        if self.max_raw_outcomes == 0 {
            return Err("max_raw_outcomes must be greater than 0".to_string());
        }

        if self.min_sample_size == 0 {
            return Err("min_sample_size must be greater than 0".to_string());
        }

        if self.significance_level <= 0.0 || self.significance_level >= 1.0 {
            return Err(format!(
                "significance_level must be between 0.0 and 1.0 (exclusive), got {}",
                self.significance_level
            ));
        }

        // Check for duplicate experiment IDs
        let mut seen_ids = std::collections::HashSet::new();
        for exp in &self.experiments {
            if !seen_ids.insert(&exp.id) {
                return Err(format!("Duplicate experiment id: '{}'", exp.id));
            }
            exp.validate(available_profiles)?;
        }

        // Check concurrent experiment limit
        let enabled_count = self.experiments.iter().filter(|e| e.enabled).count();
        if enabled_count > self.max_concurrent_experiments {
            return Err(format!(
                "Too many enabled experiments ({}) exceeds max_concurrent_experiments ({})",
                enabled_count, self.max_concurrent_experiments
            ));
        }

        Ok(())
    }
}

// =============================================================================
// ExperimentConfigToml
// =============================================================================

/// Experiment configuration from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentConfigToml {
    /// Unique experiment identifier
    pub id: String,

    /// Whether experiment is enabled
    #[serde(default = "default_experiment_enabled")]
    pub enabled: bool,

    /// Description of the experiment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Percentage of traffic to include (0-100)
    #[serde(default = "default_traffic_percentage")]
    pub traffic_percentage: u8,

    /// Assignment strategy: "user_id", "session_id", "request_id"
    #[serde(default = "default_assignment_strategy")]
    pub assignment_strategy: String,

    /// Task intents to target (empty = all)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_intents: Vec<String>,

    /// Metrics to track
    #[serde(default = "default_tracked_metrics")]
    pub metrics: Vec<String>,

    /// Experiment variants
    pub variants: Vec<VariantConfigToml>,

    /// Start time (ISO 8601)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,

    /// End time (ISO 8601)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
}

impl ExperimentConfigToml {
    /// Validate experiment configuration
    pub fn validate(&self, available_profiles: &[&str]) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("Experiment id cannot be empty".to_string());
        }

        if self.traffic_percentage > 100 {
            return Err(format!(
                "Experiment '{}': traffic_percentage must be 0-100, got {}",
                self.id, self.traffic_percentage
            ));
        }

        let valid_strategies = ["user_id", "session_id", "request_id"];
        if !valid_strategies.contains(&self.assignment_strategy.as_str()) {
            return Err(format!(
                "Experiment '{}': invalid assignment_strategy '{}'. Valid: {:?}",
                self.id, self.assignment_strategy, valid_strategies
            ));
        }

        if self.variants.is_empty() {
            return Err(format!(
                "Experiment '{}': at least one variant is required",
                self.id
            ));
        }

        if self.variants.len() < 2 {
            return Err(format!(
                "Experiment '{}': at least two variants are required for A/B testing",
                self.id
            ));
        }

        // Check for duplicate variant IDs
        let mut seen_ids = std::collections::HashSet::new();
        for variant in &self.variants {
            if !seen_ids.insert(&variant.id) {
                return Err(format!(
                    "Experiment '{}': duplicate variant id '{}'",
                    self.id, variant.id
                ));
            }
            variant.validate(&self.id, available_profiles)?;
        }

        // Check weights sum to > 0
        let total_weight: u32 = self.variants.iter().map(|v| v.weight as u32).sum();
        if total_weight == 0 {
            return Err(format!(
                "Experiment '{}': total variant weights must be > 0",
                self.id
            ));
        }

        Ok(())
    }
}

// =============================================================================
// VariantConfigToml
// =============================================================================

/// Variant configuration from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantConfigToml {
    /// Unique variant identifier within experiment
    pub id: String,

    /// Model profile to use (overrides default routing)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,

    /// Weight for traffic distribution (relative to other variants)
    #[serde(default = "default_variant_weight")]
    pub weight: u8,

    /// Whether this is the control variant
    #[serde(default)]
    pub is_control: bool,

    /// Additional parameters to pass to model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl VariantConfigToml {
    /// Validate variant configuration
    pub fn validate(&self, experiment_id: &str, available_profiles: &[&str]) -> Result<(), String> {
        if self.id.is_empty() {
            return Err(format!(
                "Experiment '{}': variant id cannot be empty",
                experiment_id
            ));
        }

        // Validate model_override if specified
        if let Some(ref model) = self.model_override {
            let profile_set: std::collections::HashSet<&str> =
                available_profiles.iter().copied().collect();
            if !profile_set.contains(model.as_str()) {
                return Err(format!(
                    "Experiment '{}', variant '{}': model_override '{}' references unknown profile. Available: {:?}",
                    experiment_id, self.id, model, available_profiles
                ));
            }
        }

        Ok(())
    }
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_ab_testing_enabled() -> bool {
    false // Disabled by default
}

fn default_max_concurrent_experiments() -> usize {
    10
}

fn default_max_raw_outcomes() -> usize {
    100_000
}

fn default_min_sample_size() -> usize {
    30
}

fn default_significance_level() -> f64 {
    0.05
}

fn default_experiment_enabled() -> bool {
    true
}

fn default_traffic_percentage() -> u8 {
    10
}

fn default_assignment_strategy() -> String {
    "user_id".to_string()
}

fn default_tracked_metrics() -> Vec<String> {
    vec![
        "latency".to_string(),
        "cost".to_string(),
        "success_rate".to_string(),
    ]
}

fn default_variant_weight() -> u8 {
    50
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ab_testing_config_default() {
        let config = ABTestingConfigToml::default();
        assert!(!config.enabled);
        assert_eq!(config.max_concurrent_experiments, 10);
        assert_eq!(config.max_raw_outcomes, 100_000);
        assert_eq!(config.min_sample_size, 30);
        assert!((config.significance_level - 0.05).abs() < 0.001);
        assert!(config.experiments.is_empty());
    }

    #[test]
    fn test_ab_testing_config_validation() {
        let mut config = ABTestingConfigToml::default();
        let profiles: Vec<&str> = vec!["claude-opus", "claude-sonnet"];

        // Default should be valid
        assert!(config.validate(&profiles).is_ok());

        // Invalid max_concurrent_experiments
        config.max_concurrent_experiments = 0;
        assert!(config.validate(&profiles).is_err());
        config.max_concurrent_experiments = 10;

        // Invalid significance_level
        config.significance_level = 1.5;
        assert!(config.validate(&profiles).is_err());
        config.significance_level = 0.05;

        // Valid experiment
        let exp = ExperimentConfigToml {
            id: "test-exp".to_string(),
            enabled: true,
            description: None,
            traffic_percentage: 20,
            assignment_strategy: "user_id".to_string(),
            target_intents: vec![],
            metrics: vec!["latency".to_string()],
            variants: vec![
                VariantConfigToml {
                    id: "control".to_string(),
                    model_override: Some("claude-opus".to_string()),
                    weight: 50,
                    is_control: true,
                    parameters: None,
                },
                VariantConfigToml {
                    id: "treatment".to_string(),
                    model_override: Some("claude-sonnet".to_string()),
                    weight: 50,
                    is_control: false,
                    parameters: None,
                },
            ],
            start_time: None,
            end_time: None,
        };
        config.experiments.push(exp);
        assert!(config.validate(&profiles).is_ok());

        // Invalid model reference
        config.experiments[0].variants[0].model_override = Some("unknown-model".to_string());
        assert!(config.validate(&profiles).is_err());
    }

    #[test]
    fn test_ab_testing_experiment_validation() {
        let profiles: Vec<&str> = vec!["claude-opus", "claude-sonnet"];

        // Missing variants
        let exp = ExperimentConfigToml {
            id: "test".to_string(),
            enabled: true,
            description: None,
            traffic_percentage: 10,
            assignment_strategy: "user_id".to_string(),
            target_intents: vec![],
            metrics: vec![],
            variants: vec![],
            start_time: None,
            end_time: None,
        };
        assert!(exp.validate(&profiles).is_err());

        // Single variant (need at least 2)
        let exp2 = ExperimentConfigToml {
            id: "test".to_string(),
            enabled: true,
            description: None,
            traffic_percentage: 10,
            assignment_strategy: "user_id".to_string(),
            target_intents: vec![],
            metrics: vec![],
            variants: vec![VariantConfigToml {
                id: "control".to_string(),
                model_override: None,
                weight: 100,
                is_control: true,
                parameters: None,
            }],
            start_time: None,
            end_time: None,
        };
        assert!(exp2.validate(&profiles).is_err());

        // Invalid traffic percentage
        let exp3 = ExperimentConfigToml {
            id: "test".to_string(),
            enabled: true,
            description: None,
            traffic_percentage: 150,
            assignment_strategy: "user_id".to_string(),
            target_intents: vec![],
            metrics: vec![],
            variants: vec![
                VariantConfigToml {
                    id: "a".to_string(),
                    model_override: None,
                    weight: 50,
                    is_control: false,
                    parameters: None,
                },
                VariantConfigToml {
                    id: "b".to_string(),
                    model_override: None,
                    weight: 50,
                    is_control: false,
                    parameters: None,
                },
            ],
            start_time: None,
            end_time: None,
        };
        assert!(exp3.validate(&profiles).is_err());
    }

    #[test]
    fn test_ab_testing_toml_deserialization() {
        let toml_str = r#"
            enabled = true
            max_concurrent_experiments = 5
            significance_level = 0.01

            [[experiments]]
            id = "model-comparison"
            enabled = true
            traffic_percentage = 25
            assignment_strategy = "session_id"

            [[experiments.variants]]
            id = "control"
            model_override = "claude-opus"
            weight = 50
            is_control = true

            [[experiments.variants]]
            id = "treatment"
            model_override = "claude-sonnet"
            weight = 50
        "#;

        let config: ABTestingConfigToml = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.max_concurrent_experiments, 5);
        assert!((config.significance_level - 0.01).abs() < 0.001);
        assert_eq!(config.experiments.len(), 1);

        let exp = &config.experiments[0];
        assert_eq!(exp.id, "model-comparison");
        assert_eq!(exp.traffic_percentage, 25);
        assert_eq!(exp.assignment_strategy, "session_id");
        assert_eq!(exp.variants.len(), 2);
        assert!(exp.variants[0].is_control);
    }
}
