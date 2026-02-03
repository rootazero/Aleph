//! DecisionConfig - Unified decision configuration
//!
//! Provides configuration structs for routing thresholds, confirmation
//! policies, and execution parameters. Used by the DecisionEngine to
//! determine how to handle user intents based on confidence scores.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;

/// Unified decision configuration
///
/// Controls routing behavior, confirmation requirements, and execution
/// parameters for the dispatcher. Each subsystem has its own config
/// struct with sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionConfig {
    /// Routing configuration (confidence thresholds)
    pub routing: RoutingConfig,
    /// Confirmation configuration (user interaction policies)
    pub confirmation: ConfirmationConfig,
    /// Execution configuration (tool call parameters)
    pub execution: ExecutionConfig,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            routing: RoutingConfig::default(),
            confirmation: ConfirmationConfig::default(),
            execution: ExecutionConfig::default(),
        }
    }
}

impl DecisionConfig {
    /// Determine the decision action based on confidence score
    ///
    /// Maps a confidence value (0.0-1.0) to an appropriate action:
    /// - Below `no_match_threshold`: NoMatch
    /// - Below `require_threshold`: RequiresConfirmation
    /// - Below `auto_execute_threshold`: OptionalConfirmation
    /// - At or above `auto_execute_threshold`: AutoExecute
    pub fn decide(&self, confidence: f32) -> DecisionAction {
        if confidence < self.routing.no_match_threshold {
            DecisionAction::NoMatch
        } else if confidence < self.confirmation.require_threshold {
            DecisionAction::RequiresConfirmation
        } else if confidence < self.confirmation.auto_execute_threshold {
            DecisionAction::OptionalConfirmation
        } else {
            DecisionAction::AutoExecute
        }
    }

    /// Load configuration from a TOML file
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn from_file(path: &Path) -> Result<Self, std::io::Error> {
        let mut file = std::fs::File::open(path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;

        toml::from_str(&contents).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to parse TOML: {}", e),
            )
        })
    }

    /// Save configuration to a TOML file
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let contents = toml::to_string_pretty(self).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to serialize TOML: {}", e),
            )
        })?;

        let mut file = std::fs::File::create(path)?;
        file.write_all(contents.as_bytes())
    }
}

/// Routing configuration - confidence thresholds for intent classification
///
/// These thresholds determine which routing tier handles the request:
/// - L1 (exact match): confidence >= l1_threshold
/// - L2 (pattern match): confidence >= l2_threshold
/// - L3 (semantic match): confidence >= l3_threshold
/// - No match: confidence < no_match_threshold
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Threshold for L1 exact match routing (default: 1.0)
    pub l1_threshold: f32,
    /// Threshold for L2 pattern match routing (default: 0.6)
    pub l2_threshold: f32,
    /// Threshold for L3 semantic match routing (default: 0.4)
    pub l3_threshold: f32,
    /// Timeout for L3 semantic matching in milliseconds (default: 5000)
    pub l3_timeout_ms: u64,
    /// Threshold below which intent is considered "no match" (default: 0.3)
    pub no_match_threshold: f32,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            l1_threshold: 1.0,
            l2_threshold: 0.6,
            l3_threshold: 0.4,
            l3_timeout_ms: 5000,
            no_match_threshold: 0.3,
        }
    }
}

/// Confirmation configuration - user interaction policies
///
/// Controls when and how the system asks for user confirmation
/// before executing actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationConfig {
    /// Master switch for confirmation prompts (default: true)
    pub enabled: bool,
    /// Confidence threshold requiring mandatory confirmation (default: 0.5)
    pub require_threshold: f32,
    /// Confidence threshold allowing auto-execution (default: 0.9)
    pub auto_execute_threshold: f32,
    /// Timeout for confirmation prompts in milliseconds (default: 30000)
    pub timeout_ms: u64,
}

impl Default for ConfirmationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            require_threshold: 0.5,
            auto_execute_threshold: 0.9,
            timeout_ms: 30000,
        }
    }
}

/// Execution configuration - tool call parameters
///
/// Controls how tool calls are executed, including parallelism,
/// timeouts, and retry behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Maximum number of parallel tool calls (default: 5)
    pub max_parallel_calls: usize,
    /// Timeout for individual tool execution in milliseconds (default: 60000)
    pub tool_timeout_ms: u64,
    /// Number of retry attempts for failed tool calls (default: 2)
    pub retry_count: u32,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_parallel_calls: 5,
            tool_timeout_ms: 60000,
            retry_count: 2,
        }
    }
}

/// Decision action based on confidence score
///
/// Represents the action the system should take for a given intent
/// based on its confidence score and the current configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionAction {
    /// Confidence too low - no matching intent found
    NoMatch,
    /// Low confidence - requires explicit user confirmation
    RequiresConfirmation,
    /// Medium confidence - confirmation optional but recommended
    OptionalConfirmation,
    /// High confidence - safe to execute automatically
    AutoExecute,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_config() {
        let config = DecisionConfig::default();

        // Check routing defaults
        assert_eq!(config.routing.l1_threshold, 1.0);
        assert_eq!(config.routing.l2_threshold, 0.6);
        assert_eq!(config.routing.l3_threshold, 0.4);
        assert_eq!(config.routing.l3_timeout_ms, 5000);
        assert_eq!(config.routing.no_match_threshold, 0.3);

        // Check confirmation defaults
        assert!(config.confirmation.enabled);
        assert_eq!(config.confirmation.require_threshold, 0.5);
        assert_eq!(config.confirmation.auto_execute_threshold, 0.9);
        assert_eq!(config.confirmation.timeout_ms, 30000);

        // Check execution defaults
        assert_eq!(config.execution.max_parallel_calls, 5);
        assert_eq!(config.execution.tool_timeout_ms, 60000);
        assert_eq!(config.execution.retry_count, 2);
    }

    #[test]
    fn test_decide_no_match() {
        let config = DecisionConfig::default();
        let action = config.decide(0.2);
        assert_eq!(action, DecisionAction::NoMatch);
    }

    #[test]
    fn test_decide_requires_confirmation() {
        let config = DecisionConfig::default();
        let action = config.decide(0.4);
        assert_eq!(action, DecisionAction::RequiresConfirmation);
    }

    #[test]
    fn test_decide_optional_confirmation() {
        let config = DecisionConfig::default();
        let action = config.decide(0.7);
        assert_eq!(action, DecisionAction::OptionalConfirmation);
    }

    #[test]
    fn test_decide_auto_execute() {
        let config = DecisionConfig::default();
        let action = config.decide(0.95);
        assert_eq!(action, DecisionAction::AutoExecute);
    }

    #[test]
    fn test_decide_boundary_no_match() {
        let config = DecisionConfig::default();
        // At exactly no_match_threshold (0.3), should be RequiresConfirmation
        let action = config.decide(0.3);
        assert_eq!(action, DecisionAction::RequiresConfirmation);
    }

    #[test]
    fn test_decide_boundary_require() {
        let config = DecisionConfig::default();
        // At exactly require_threshold (0.5), should be OptionalConfirmation
        let action = config.decide(0.5);
        assert_eq!(action, DecisionAction::OptionalConfirmation);
    }

    #[test]
    fn test_decide_boundary_auto_execute() {
        let config = DecisionConfig::default();
        // At exactly auto_execute_threshold (0.9), should be AutoExecute
        let action = config.decide(0.9);
        assert_eq!(action, DecisionAction::AutoExecute);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("decision_config.toml");

        let original = DecisionConfig::default();
        original.to_file(&path).unwrap();

        let loaded = DecisionConfig::from_file(&path).unwrap();

        // Verify all fields match
        assert_eq!(original.routing.l1_threshold, loaded.routing.l1_threshold);
        assert_eq!(original.routing.l2_threshold, loaded.routing.l2_threshold);
        assert_eq!(original.routing.l3_threshold, loaded.routing.l3_threshold);
        assert_eq!(original.routing.l3_timeout_ms, loaded.routing.l3_timeout_ms);
        assert_eq!(
            original.routing.no_match_threshold,
            loaded.routing.no_match_threshold
        );

        assert_eq!(original.confirmation.enabled, loaded.confirmation.enabled);
        assert_eq!(
            original.confirmation.require_threshold,
            loaded.confirmation.require_threshold
        );
        assert_eq!(
            original.confirmation.auto_execute_threshold,
            loaded.confirmation.auto_execute_threshold
        );
        assert_eq!(
            original.confirmation.timeout_ms,
            loaded.confirmation.timeout_ms
        );

        assert_eq!(
            original.execution.max_parallel_calls,
            loaded.execution.max_parallel_calls
        );
        assert_eq!(
            original.execution.tool_timeout_ms,
            loaded.execution.tool_timeout_ms
        );
        assert_eq!(original.execution.retry_count, loaded.execution.retry_count);
    }

    #[test]
    fn test_toml_format() {
        let config = DecisionConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();

        // Verify it contains expected sections
        assert!(toml_str.contains("[routing]"));
        assert!(toml_str.contains("[confirmation]"));
        assert!(toml_str.contains("[execution]"));
        assert!(toml_str.contains("l1_threshold"));
        assert!(toml_str.contains("auto_execute_threshold"));
        assert!(toml_str.contains("max_parallel_calls"));
    }

    #[test]
    fn test_file_not_found() {
        let result = DecisionConfig::from_file(Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_toml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("invalid.toml");

        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        let result = DecisionConfig::from_file(&path);
        assert!(result.is_err());
    }
}
