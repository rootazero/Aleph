//! Multi-model ensemble configuration (P3)
//!
//! Contains EnsembleConfigToml, EnsembleStrategyConfigToml, and
//! HighComplexityEnsembleConfigToml for configuring ensemble execution.

use serde::{Deserialize, Serialize};

// =============================================================================
// EnsembleConfigToml
// =============================================================================

/// Multi-model ensemble configuration from TOML
///
/// Configures ensemble execution for combining responses from multiple models.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.ensemble]
/// enabled = true
/// default_mode = "best_of_n"
/// default_timeout_secs = 60
/// max_parallel_models = 5
///
/// [[cowork.model_routing.ensemble.strategies]]
/// intent = "reasoning"
/// mode = "consensus"
/// models = ["claude-opus", "gpt-4o"]
/// quality_threshold = 0.8
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleConfigToml {
    /// Enable ensemble execution
    #[serde(default = "default_ensemble_enabled")]
    pub enabled: bool,

    /// Default ensemble mode: "disabled", "best_of_n", "voting", "consensus", "cascade"
    #[serde(default = "default_ensemble_mode")]
    pub default_mode: String,

    /// Default timeout for parallel model execution (seconds)
    #[serde(default = "default_ensemble_timeout")]
    pub default_timeout_secs: u64,

    /// Maximum number of models to run in parallel
    #[serde(default = "default_max_parallel_models")]
    pub max_parallel_models: usize,

    /// Quality scorer to use: "length", "structure", "length_and_structure", "confidence"
    #[serde(default = "default_quality_scorer")]
    pub quality_scorer: String,

    /// Minimum quality threshold for cascade early termination (0.0-1.0)
    #[serde(default = "default_quality_threshold")]
    pub quality_threshold: f64,

    /// Consensus similarity threshold for voting/consensus modes (0.0-1.0)
    #[serde(default = "default_consensus_threshold")]
    pub consensus_threshold: f64,

    /// Per-intent strategy configurations
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strategies: Vec<EnsembleStrategyConfigToml>,

    /// Enable ensemble for high complexity prompts automatically
    #[serde(default)]
    pub high_complexity_ensemble: HighComplexityEnsembleConfigToml,
}

impl Default for EnsembleConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_ensemble_enabled(),
            default_mode: default_ensemble_mode(),
            default_timeout_secs: default_ensemble_timeout(),
            max_parallel_models: default_max_parallel_models(),
            quality_scorer: default_quality_scorer(),
            quality_threshold: default_quality_threshold(),
            consensus_threshold: default_consensus_threshold(),
            strategies: Vec::new(),
            high_complexity_ensemble: HighComplexityEnsembleConfigToml::default(),
        }
    }
}

impl EnsembleConfigToml {
    /// Validate ensemble configuration
    pub fn validate(&self, available_profiles: &[&str]) -> Result<(), String> {
        let valid_modes = ["disabled", "best_of_n", "voting", "consensus", "cascade"];
        if !valid_modes.contains(&self.default_mode.as_str()) {
            return Err(format!(
                "Invalid default_mode '{}'. Valid: {:?}",
                self.default_mode, valid_modes
            ));
        }

        if self.default_timeout_secs == 0 {
            return Err("default_timeout_secs must be greater than 0".to_string());
        }

        if self.max_parallel_models == 0 {
            return Err("max_parallel_models must be greater than 0".to_string());
        }

        let valid_scorers = [
            "length",
            "structure",
            "length_and_structure",
            "confidence",
            "relevance",
        ];
        if !valid_scorers.contains(&self.quality_scorer.as_str()) {
            return Err(format!(
                "Invalid quality_scorer '{}'. Valid: {:?}",
                self.quality_scorer, valid_scorers
            ));
        }

        if self.quality_threshold < 0.0 || self.quality_threshold > 1.0 {
            return Err(format!(
                "quality_threshold must be between 0.0 and 1.0, got {}",
                self.quality_threshold
            ));
        }

        if self.consensus_threshold < 0.0 || self.consensus_threshold > 1.0 {
            return Err(format!(
                "consensus_threshold must be between 0.0 and 1.0, got {}",
                self.consensus_threshold
            ));
        }

        // Validate strategies
        for strategy in &self.strategies {
            strategy.validate(available_profiles)?;
        }

        // Validate high complexity ensemble config
        self.high_complexity_ensemble.validate(available_profiles)?;

        Ok(())
    }
}

// =============================================================================
// EnsembleStrategyConfigToml
// =============================================================================

/// Per-intent ensemble strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleStrategyConfigToml {
    /// Task intent to apply this strategy to
    pub intent: String,

    /// Ensemble mode for this intent
    pub mode: String,

    /// Models to use for ensemble (references model profile IDs)
    pub models: Vec<String>,

    /// Quality threshold override for this strategy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_threshold: Option<f64>,

    /// Quality scorer override
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_scorer: Option<String>,

    /// Timeout override (seconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

impl EnsembleStrategyConfigToml {
    /// Validate strategy configuration
    pub fn validate(&self, available_profiles: &[&str]) -> Result<(), String> {
        let profile_set: std::collections::HashSet<&str> =
            available_profiles.iter().copied().collect();

        if self.intent.is_empty() {
            return Err("Strategy intent cannot be empty".to_string());
        }

        let valid_modes = ["disabled", "best_of_n", "voting", "consensus", "cascade"];
        if !valid_modes.contains(&self.mode.as_str()) {
            return Err(format!(
                "Strategy '{}': invalid mode '{}'. Valid: {:?}",
                self.intent, self.mode, valid_modes
            ));
        }

        if self.models.is_empty() && self.mode != "disabled" {
            return Err(format!(
                "Strategy '{}': at least one model is required when mode is not 'disabled'",
                self.intent
            ));
        }

        for model in &self.models {
            if !profile_set.contains(model.as_str()) {
                return Err(format!(
                    "Strategy '{}': model '{}' references unknown profile. Available: {:?}",
                    self.intent, model, available_profiles
                ));
            }
        }

        if let Some(threshold) = self.quality_threshold {
            if !(0.0..=1.0).contains(&threshold) {
                return Err(format!(
                    "Strategy '{}': quality_threshold must be between 0.0 and 1.0, got {}",
                    self.intent, threshold
                ));
            }
        }

        if let Some(ref scorer) = self.quality_scorer {
            let valid_scorers = [
                "length",
                "structure",
                "length_and_structure",
                "confidence",
                "relevance",
            ];
            if !valid_scorers.contains(&scorer.as_str()) {
                return Err(format!(
                    "Strategy '{}': invalid quality_scorer '{}'. Valid: {:?}",
                    self.intent, scorer, valid_scorers
                ));
            }
        }

        Ok(())
    }
}

// =============================================================================
// HighComplexityEnsembleConfigToml
// =============================================================================

/// High complexity automatic ensemble configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighComplexityEnsembleConfigToml {
    /// Enable automatic ensemble for high complexity prompts
    #[serde(default)]
    pub enabled: bool,

    /// Complexity threshold to trigger ensemble (0.0-1.0)
    #[serde(default = "default_high_complexity_trigger")]
    pub complexity_threshold: f64,

    /// Ensemble mode for high complexity prompts
    #[serde(default = "default_high_complexity_mode")]
    pub mode: String,

    /// Models to use for high complexity ensemble
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

impl Default for HighComplexityEnsembleConfigToml {
    fn default() -> Self {
        Self {
            enabled: false,
            complexity_threshold: default_high_complexity_trigger(),
            mode: default_high_complexity_mode(),
            models: Vec::new(),
        }
    }
}

impl HighComplexityEnsembleConfigToml {
    /// Validate high complexity ensemble configuration
    pub fn validate(&self, available_profiles: &[&str]) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        if self.complexity_threshold < 0.0 || self.complexity_threshold > 1.0 {
            return Err(format!(
                "high_complexity_ensemble.complexity_threshold must be between 0.0 and 1.0, got {}",
                self.complexity_threshold
            ));
        }

        let valid_modes = ["best_of_n", "voting", "consensus"];
        if !valid_modes.contains(&self.mode.as_str()) {
            return Err(format!(
                "high_complexity_ensemble.mode must be one of {:?}, got '{}'",
                valid_modes, self.mode
            ));
        }

        if self.models.is_empty() {
            return Err("high_complexity_ensemble.models cannot be empty when enabled".to_string());
        }

        let profile_set: std::collections::HashSet<&str> =
            available_profiles.iter().copied().collect();
        for model in &self.models {
            if !profile_set.contains(model.as_str()) {
                return Err(format!(
                    "high_complexity_ensemble.models: '{}' references unknown profile. Available: {:?}",
                    model, available_profiles
                ));
            }
        }

        Ok(())
    }
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_ensemble_enabled() -> bool {
    false // Disabled by default
}

fn default_ensemble_mode() -> String {
    "disabled".to_string()
}

fn default_ensemble_timeout() -> u64 {
    60 // 60 seconds
}

fn default_max_parallel_models() -> usize {
    5
}

fn default_quality_scorer() -> String {
    "length_and_structure".to_string()
}

fn default_quality_threshold() -> f64 {
    0.7
}

fn default_consensus_threshold() -> f64 {
    0.6
}

fn default_high_complexity_trigger() -> f64 {
    0.8
}

fn default_high_complexity_mode() -> String {
    "consensus".to_string()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensemble_config_default() {
        let config = EnsembleConfigToml::default();
        assert!(!config.enabled);
        assert_eq!(config.default_mode, "disabled");
        assert_eq!(config.default_timeout_secs, 60);
        assert_eq!(config.max_parallel_models, 5);
        assert_eq!(config.quality_scorer, "length_and_structure");
        assert!((config.quality_threshold - 0.7).abs() < 0.001);
        assert!((config.consensus_threshold - 0.6).abs() < 0.001);
        assert!(config.strategies.is_empty());
    }

    #[test]
    fn test_ensemble_config_validation() {
        let mut config = EnsembleConfigToml::default();
        let profiles: Vec<&str> = vec!["claude-opus", "claude-sonnet", "gpt-4o"];

        // Default should be valid
        assert!(config.validate(&profiles).is_ok());

        // Invalid mode
        config.default_mode = "invalid_mode".to_string();
        assert!(config.validate(&profiles).is_err());
        config.default_mode = "best_of_n".to_string();

        // Invalid timeout
        config.default_timeout_secs = 0;
        assert!(config.validate(&profiles).is_err());
        config.default_timeout_secs = 60;

        // Invalid quality scorer
        config.quality_scorer = "invalid_scorer".to_string();
        assert!(config.validate(&profiles).is_err());
        config.quality_scorer = "length_and_structure".to_string();

        // Invalid threshold
        config.quality_threshold = 1.5;
        assert!(config.validate(&profiles).is_err());
        config.quality_threshold = 0.7;

        // Valid strategy
        let strategy = EnsembleStrategyConfigToml {
            intent: "reasoning".to_string(),
            mode: "consensus".to_string(),
            models: vec!["claude-opus".to_string(), "gpt-4o".to_string()],
            quality_threshold: None,
            quality_scorer: None,
            timeout_secs: None,
        };
        config.strategies.push(strategy);
        assert!(config.validate(&profiles).is_ok());

        // Invalid model in strategy
        config.strategies[0]
            .models
            .push("unknown-model".to_string());
        assert!(config.validate(&profiles).is_err());
    }

    #[test]
    fn test_high_complexity_ensemble_validation() {
        let profiles: Vec<&str> = vec!["claude-opus", "claude-sonnet"];

        let mut config = HighComplexityEnsembleConfigToml::default();
        // Disabled by default, should be valid
        assert!(config.validate(&profiles).is_ok());

        // Enable but no models
        config.enabled = true;
        assert!(config.validate(&profiles).is_err());

        // Add models
        config.models = vec!["claude-opus".to_string(), "claude-sonnet".to_string()];
        assert!(config.validate(&profiles).is_ok());

        // Invalid threshold
        config.complexity_threshold = 1.5;
        assert!(config.validate(&profiles).is_err());
        config.complexity_threshold = 0.8;

        // Invalid mode
        config.mode = "cascade".to_string(); // cascade not allowed for high complexity
        assert!(config.validate(&profiles).is_err());
    }

    #[test]
    fn test_ensemble_toml_deserialization() {
        let toml_str = r#"
            enabled = true
            default_mode = "best_of_n"
            default_timeout_secs = 300
            quality_threshold = 0.8

            [[strategies]]
            intent = "code_generation"
            mode = "voting"
            models = ["claude-opus", "gpt-4o"]
            quality_threshold = 0.9

            [high_complexity_ensemble]
            enabled = true
            complexity_threshold = 0.85
            mode = "consensus"
            models = ["claude-opus", "claude-sonnet"]
        "#;

        let config: EnsembleConfigToml = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.default_mode, "best_of_n");
        assert_eq!(config.default_timeout_secs, 300); // As specified in TOML
        assert!((config.quality_threshold - 0.8).abs() < 0.001);
        assert_eq!(config.strategies.len(), 1);

        let strategy = &config.strategies[0];
        assert_eq!(strategy.intent, "code_generation");
        assert_eq!(strategy.mode, "voting");
        assert_eq!(strategy.models.len(), 2);

        assert!(config.high_complexity_ensemble.enabled);
        assert!((config.high_complexity_ensemble.complexity_threshold - 0.85).abs() < 0.001);
        assert_eq!(config.high_complexity_ensemble.mode, "consensus");
    }
}
