//! Sub-Agent Configuration
//!
//! Configuration for the sub-agent synchronization system including
//! execution timeouts, concurrency limits, and result collection settings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Sub-Agent Configuration
///
/// Controls the behavior of sub-agent execution including timeouts,
/// concurrency, and result collection.
///
/// # Example Configuration (config.toml)
///
/// ```toml
/// [subagent]
/// execution_timeout_ms = 300000  # 5 minutes
/// result_ttl_ms = 3600000        # 1 hour
/// max_concurrent = 5
/// progress_events_enabled = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubAgentConfig {
    /// Maximum time to wait for a sub-agent to complete (in milliseconds)
    /// Default: 300000 (5 minutes)
    #[serde(default = "default_execution_timeout_ms")]
    pub execution_timeout_ms: u64,

    /// How long to keep completed results before cleanup (in milliseconds)
    /// Default: 3600000 (1 hour)
    #[serde(default = "default_result_ttl_ms")]
    pub result_ttl_ms: u64,

    /// Maximum concurrent sub-agent executions
    /// Default: 5
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    /// Enable real-time progress events for UI updates
    /// Default: true
    #[serde(default = "default_progress_events_enabled")]
    pub progress_events_enabled: bool,

    /// Enable detailed tool call tracking in results
    /// Default: true
    #[serde(default = "default_track_tool_calls")]
    pub track_tool_calls: bool,

    /// Maximum number of recent steps to include in context propagation
    /// Default: 10
    #[serde(default = "default_max_context_steps")]
    pub max_context_steps: usize,

    /// Maximum length for history summary in context propagation
    /// Default: 500
    #[serde(default = "default_max_history_summary_len")]
    pub max_history_summary_len: usize,
}

fn default_execution_timeout_ms() -> u64 {
    300_000 // 5 minutes
}

fn default_result_ttl_ms() -> u64 {
    3_600_000 // 1 hour
}

fn default_max_concurrent() -> usize {
    5
}

fn default_progress_events_enabled() -> bool {
    true
}

fn default_track_tool_calls() -> bool {
    true
}

fn default_max_context_steps() -> usize {
    10
}

fn default_max_history_summary_len() -> usize {
    500
}

impl Default for SubAgentConfig {
    fn default() -> Self {
        Self {
            execution_timeout_ms: default_execution_timeout_ms(),
            result_ttl_ms: default_result_ttl_ms(),
            max_concurrent: default_max_concurrent(),
            progress_events_enabled: default_progress_events_enabled(),
            track_tool_calls: default_track_tool_calls(),
            max_context_steps: default_max_context_steps(),
            max_history_summary_len: default_max_history_summary_len(),
        }
    }
}

impl SubAgentConfig {
    /// Create a new SubAgentConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert to CoordinatorConfig for use with ExecutionCoordinator
    pub fn to_coordinator_config(&self) -> crate::agents::sub_agents::CoordinatorConfig {
        crate::agents::sub_agents::CoordinatorConfig {
            execution_timeout_ms: self.execution_timeout_ms,
            result_ttl_ms: self.result_ttl_ms,
            max_concurrent: self.max_concurrent,
            progress_events_enabled: self.progress_events_enabled,
        }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.execution_timeout_ms == 0 {
            return Err("execution_timeout_ms must be greater than 0".to_string());
        }
        if self.result_ttl_ms == 0 {
            return Err("result_ttl_ms must be greater than 0".to_string());
        }
        if self.max_concurrent == 0 {
            return Err("max_concurrent must be greater than 0".to_string());
        }
        if self.max_context_steps == 0 {
            return Err("max_context_steps must be greater than 0".to_string());
        }
        if self.max_history_summary_len == 0 {
            return Err("max_history_summary_len must be greater than 0".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SubAgentConfig::default();
        assert_eq!(config.execution_timeout_ms, 300_000);
        assert_eq!(config.result_ttl_ms, 3_600_000);
        assert_eq!(config.max_concurrent, 5);
        assert!(config.progress_events_enabled);
        assert!(config.track_tool_calls);
        assert_eq!(config.max_context_steps, 10);
        assert_eq!(config.max_history_summary_len, 500);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = SubAgentConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_timeout() {
        let config = SubAgentConfig {
            execution_timeout_ms: 0,
            ..SubAgentConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_concurrent() {
        let config = SubAgentConfig {
            max_concurrent: 0,
            ..SubAgentConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_to_coordinator_config() {
        let config = SubAgentConfig {
            execution_timeout_ms: 60_000,
            result_ttl_ms: 120_000,
            max_concurrent: 10,
            progress_events_enabled: false,
            ..Default::default()
        };

        let coordinator_config = config.to_coordinator_config();
        assert_eq!(coordinator_config.execution_timeout_ms, 60_000);
        assert_eq!(coordinator_config.result_ttl_ms, 120_000);
        assert_eq!(coordinator_config.max_concurrent, 10);
        assert!(!coordinator_config.progress_events_enabled);
    }

    #[test]
    fn test_config_serialization() {
        let config = SubAgentConfig::default();
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("execution_timeout_ms"));
        assert!(serialized.contains("max_concurrent"));
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            execution_timeout_ms = 60000
            result_ttl_ms = 180000
            max_concurrent = 3
            progress_events_enabled = false
        "#;
        let config: SubAgentConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.execution_timeout_ms, 60_000);
        assert_eq!(config.result_ttl_ms, 180_000);
        assert_eq!(config.max_concurrent, 3);
        assert!(!config.progress_events_enabled);
        // Defaults should be applied for missing fields
        assert!(config.track_tool_calls);
    }
}
