//! Configuration for Agent Loop
//!
//! This module defines configuration options for the Agent Loop,
//! including guard limits, compression settings, and tool policies.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for Agent Loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfig {
    /// Maximum number of steps before guard triggers (default: 50)
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,

    /// Maximum total tokens before guard triggers (default: 100000)
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,

    /// Maximum execution time in seconds before timeout (default: 600 = 10 minutes)
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Maximum execution time (derived from timeout_secs)
    #[serde(skip)]
    pub timeout: Duration,

    /// Tools that require user confirmation before execution
    #[serde(default)]
    pub require_confirmation: Vec<String>,

    /// Compression configuration
    #[serde(default)]
    pub compression: CompressionConfig,

    /// Model routing configuration
    #[serde(default)]
    pub model_routing: ModelRoutingConfig,

    /// Whether to enable streaming for thinking output
    #[serde(default = "default_true")]
    pub enable_thinking_stream: bool,

    /// Whether to save session to database
    #[serde(default = "default_true")]
    pub persist_session: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        let timeout_secs = default_timeout_secs();
        Self {
            max_steps: default_max_steps(),
            max_tokens: default_max_tokens(),
            timeout_secs,
            timeout: Duration::from_secs(timeout_secs),
            require_confirmation: default_dangerous_tools(),
            compression: CompressionConfig::default(),
            model_routing: ModelRoutingConfig::default(),
            enable_thinking_stream: true,
            persist_session: true,
        }
    }
}

/// Compression configuration for context management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Number of steps before triggering compression (default: 5)
    #[serde(default = "default_compress_after")]
    pub compress_after_steps: usize,

    /// Number of recent steps to keep in full detail (default: 3)
    #[serde(default = "default_window_size")]
    pub recent_window_size: usize,

    /// Target token count for compressed summary (default: 500)
    #[serde(default = "default_summary_tokens")]
    pub target_summary_tokens: usize,

    /// Whether to preserve important tool outputs in summary
    #[serde(default = "default_true")]
    pub preserve_tool_outputs: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            compress_after_steps: default_compress_after(),
            recent_window_size: default_window_size(),
            target_summary_tokens: default_summary_tokens(),
            preserve_tool_outputs: true,
        }
    }
}

/// Model routing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoutingConfig {
    /// Default model for general tasks
    #[serde(default = "default_model")]
    pub default_model: String,

    /// Model for vision tasks (images)
    #[serde(default = "default_vision_model")]
    pub vision_model: String,

    /// Model for complex reasoning tasks
    #[serde(default = "default_reasoning_model")]
    pub reasoning_model: String,

    /// Model for simple/fast tasks
    #[serde(default = "default_fast_model")]
    pub fast_model: String,

    /// Whether to auto-select model based on task
    #[serde(default = "default_true")]
    pub auto_route: bool,
}

impl Default for ModelRoutingConfig {
    fn default() -> Self {
        Self {
            default_model: default_model(),
            vision_model: default_vision_model(),
            reasoning_model: default_reasoning_model(),
            fast_model: default_fast_model(),
            auto_route: true,
        }
    }
}

// Default value functions

fn default_max_steps() -> usize {
    50
}

fn default_max_tokens() -> usize {
    100_000
}

fn default_timeout_secs() -> u64 {
    600 // 10 minutes
}

fn default_compress_after() -> usize {
    5
}

fn default_window_size() -> usize {
    3
}

fn default_summary_tokens() -> usize {
    500
}

fn default_true() -> bool {
    true
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_vision_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_reasoning_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_fast_model() -> String {
    "claude-3-5-haiku-20241022".to_string()
}

fn default_dangerous_tools() -> Vec<String> {
    vec![
        "delete".to_string(),
        "remove".to_string(),
        "write_file".to_string(),
        "execute_code".to_string(),
        "run_command".to_string(),
        "send_email".to_string(),
        "make_purchase".to_string(),
    ]
}

impl LoopConfig {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder pattern: set max steps
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Builder pattern: set max tokens
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Builder pattern: set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.timeout_secs = timeout.as_secs();
        self
    }

    /// Builder pattern: set confirmation tools
    pub fn with_require_confirmation(mut self, tools: Vec<String>) -> Self {
        self.require_confirmation = tools;
        self
    }

    /// Builder pattern: add a tool that requires confirmation
    pub fn add_confirmation_tool(mut self, tool: &str) -> Self {
        if !self.require_confirmation.contains(&tool.to_string()) {
            self.require_confirmation.push(tool.to_string());
        }
        self
    }

    /// Create a config for testing with lower limits
    pub fn for_testing() -> Self {
        Self {
            max_steps: 10,
            max_tokens: 10_000,
            timeout_secs: 30,
            timeout: Duration::from_secs(30),
            require_confirmation: vec![],
            compression: CompressionConfig {
                compress_after_steps: 3,
                recent_window_size: 2,
                target_summary_tokens: 200,
                preserve_tool_outputs: true,
            },
            model_routing: ModelRoutingConfig::default(),
            enable_thinking_stream: false,
            persist_session: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LoopConfig::default();
        assert_eq!(config.max_steps, 50);
        assert_eq!(config.max_tokens, 100_000);
        assert_eq!(config.timeout, Duration::from_secs(600));
        assert!(!config.require_confirmation.is_empty());
    }

    #[test]
    fn test_builder_pattern() {
        let config = LoopConfig::new()
            .with_max_steps(100)
            .with_max_tokens(50_000)
            .with_timeout(Duration::from_secs(300))
            .add_confirmation_tool("custom_danger");

        assert_eq!(config.max_steps, 100);
        assert_eq!(config.max_tokens, 50_000);
        assert_eq!(config.timeout, Duration::from_secs(300));
        assert!(config.require_confirmation.contains(&"custom_danger".to_string()));
    }

    #[test]
    fn test_testing_config() {
        let config = LoopConfig::for_testing();
        assert_eq!(config.max_steps, 10);
        assert!(config.require_confirmation.is_empty());
        assert!(!config.persist_session);
    }

    #[test]
    fn test_serialization() {
        let config = LoopConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: LoopConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_steps, config.max_steps);
    }
}
