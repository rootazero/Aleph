//! Configuration for Agent Loop
//!
//! This module defines configuration options for the Agent Loop,
//! including guard limits, compression settings, tool policies,
//! and permission modes (Normal, AutoAcceptEdits, PlanMode).

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Permission mode for Agent Loop
///
/// Controls how the agent handles write operations and confirmations.
/// Inspired by Claude Code's permission model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PermissionMode {
    /// Default mode: file edits and shell commands require user confirmation
    #[default]
    Normal,

    /// Auto-accept edits: file changes are applied automatically,
    /// but shell commands still require confirmation
    AutoAcceptEdits,

    /// Plan mode: read-only exploration, all write operations are blocked.
    /// The agent can only read files, search, and explore the codebase.
    /// Useful for designing implementation approaches before writing code.
    PlanMode,
}

impl PermissionMode {
    /// Check if this mode allows write operations
    pub fn allows_writes(&self) -> bool {
        !matches!(self, PermissionMode::PlanMode)
    }

    /// Check if file edits require confirmation
    pub fn requires_edit_confirmation(&self) -> bool {
        matches!(self, PermissionMode::Normal)
    }

    /// Check if shell commands require confirmation
    pub fn requires_shell_confirmation(&self) -> bool {
        !matches!(self, PermissionMode::AutoAcceptEdits)
    }

    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            PermissionMode::Normal => "Normal mode: edits and commands require confirmation",
            PermissionMode::AutoAcceptEdits => "Auto-accept: edits are automatic, commands need confirmation",
            PermissionMode::PlanMode => "Plan mode: read-only exploration, no write operations",
        }
    }

    /// Get the display name
    pub fn display_name(&self) -> &'static str {
        match self {
            PermissionMode::Normal => "Normal",
            PermissionMode::AutoAcceptEdits => "Auto-Accept Edits",
            PermissionMode::PlanMode => "Plan Mode",
        }
    }
}

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

    /// Permission mode controlling write operations and confirmations
    #[serde(default)]
    pub permission_mode: PermissionMode,

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

    /// Stuck detection threshold: number of identical actions before triggering guard
    /// Default is 3. Higher values are recommended for multi-step skills that
    /// legitimately call the same tool multiple times (e.g., knowledge-graph skill
    /// calling file_ops for mkdir, write, write operations).
    #[serde(default = "default_stuck_threshold")]
    pub stuck_threshold: usize,

    /// Failure threshold: number of consecutive failures on same action pattern
    /// before triggering guard. Default is 3.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: usize,

    /// Doom loop detection threshold: number of consecutive identical tool calls
    /// with exact same arguments before triggering guard. Default is 3.
    /// This is more precise than stuck_threshold as it checks exact argument match.
    #[serde(default = "default_doom_loop_threshold")]
    pub doom_loop_threshold: usize,

    /// Retry configuration for think operations (LLM calls)
    #[serde(default)]
    pub think_retry: ThinkRetryConfig,

    // ========================================================================
    // Unified Session Model Feature Flags (Phase 1-3 Migration)
    // ========================================================================

    /// Use unified ExecutionSession model (Phase 1)
    ///
    /// When enabled, the agent loop creates and maintains an ExecutionSession
    /// alongside LoopState, syncing state between them using SessionSync.
    /// This is the first step toward deprecating LoopState.
    #[serde(default)]
    pub use_unified_session: bool,

    /// Use new MessageBuilder for prompt construction (Phase 2)
    ///
    /// When enabled, the agent loop uses MessageBuilder to convert
    /// SessionParts to LLM messages, including system reminder injection.
    /// Requires use_unified_session to be true.
    #[serde(default)]
    pub use_message_builder: bool,

    /// Enable real-time overflow detection (Phase 2)
    ///
    /// When enabled, the agent loop checks for context overflow before
    /// each iteration and triggers compaction when needed.
    /// Requires use_unified_session to be true.
    #[serde(default)]
    pub use_realtime_overflow: bool,
}

/// Retry configuration for think operations (LLM calls)
///
/// Inspired by OpenCode's retry.ts with exponential backoff
/// and retry-after header support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRetryConfig {
    /// Maximum number of retry attempts (default: 3)
    #[serde(default = "default_think_max_retries")]
    pub max_retries: u32,

    /// Initial backoff duration in milliseconds (default: 2000)
    #[serde(default = "default_think_initial_backoff_ms")]
    pub initial_backoff_ms: u64,

    /// Backoff multiplier for exponential growth (default: 2.0)
    #[serde(default = "default_think_backoff_multiplier")]
    pub backoff_multiplier: f64,

    /// Maximum backoff duration in milliseconds (default: 30000)
    #[serde(default = "default_think_max_backoff_ms")]
    pub max_backoff_ms: u64,

    /// Whether to respect Retry-After headers from providers (default: true)
    #[serde(default = "default_true")]
    pub respect_retry_after: bool,
}

impl Default for ThinkRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_think_max_retries(),
            initial_backoff_ms: default_think_initial_backoff_ms(),
            backoff_multiplier: default_think_backoff_multiplier(),
            max_backoff_ms: default_think_max_backoff_ms(),
            respect_retry_after: true,
        }
    }
}

impl ThinkRetryConfig {
    /// Calculate delay for a given attempt number
    ///
    /// Uses exponential backoff: initial_backoff_ms * multiplier^(attempt-1)
    /// Capped at max_backoff_ms.
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }
        let delay_ms = (self.initial_backoff_ms as f64)
            * self.backoff_multiplier.powi((attempt - 1) as i32);
        let capped_ms = delay_ms.min(self.max_backoff_ms as f64) as u64;
        Duration::from_millis(capped_ms)
    }

    /// Calculate delay respecting retry-after header if provided
    pub fn calculate_delay_with_retry_after(
        &self,
        attempt: u32,
        retry_after_ms: Option<u64>,
    ) -> Duration {
        if self.respect_retry_after {
            if let Some(ms) = retry_after_ms {
                // Use retry-after value, but still respect max_backoff
                let capped = ms.min(self.max_backoff_ms);
                return Duration::from_millis(capped);
            }
        }
        self.calculate_delay(attempt)
    }
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
            permission_mode: PermissionMode::default(),
            compression: CompressionConfig::default(),
            model_routing: ModelRoutingConfig::default(),
            enable_thinking_stream: true,
            persist_session: true,
            stuck_threshold: default_stuck_threshold(),
            failure_threshold: default_failure_threshold(),
            doom_loop_threshold: default_doom_loop_threshold(),
            think_retry: ThinkRetryConfig::default(),
            // Unified session model flags (all disabled by default for safe rollout)
            use_unified_session: false,
            use_message_builder: false,
            use_realtime_overflow: false,
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
    10 // 增加到10步，确保短期记忆不丢失关键信息
}

fn default_summary_tokens() -> usize {
    500
}

fn default_true() -> bool {
    true
}

fn default_stuck_threshold() -> usize {
    5 // Higher than before (was 3) to allow multi-step skills
}

fn default_failure_threshold() -> usize {
    3
}

fn default_doom_loop_threshold() -> usize {
    3 // Match OpenCode: DOOM_LOOP_THRESHOLD = 3
}

fn default_think_max_retries() -> u32 {
    3 // Match OpenCode retry settings
}

fn default_think_initial_backoff_ms() -> u64 {
    2000 // 2 seconds, matches OpenCode: RETRY_INITIAL_DELAY = 2000
}

fn default_think_backoff_multiplier() -> f64 {
    2.0 // Match OpenCode: RETRY_BACKOFF_FACTOR = 2
}

fn default_think_max_backoff_ms() -> u64 {
    30_000 // 30 seconds, matches OpenCode: RETRY_MAX_DELAY_NO_HEADERS = 30_000
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

    /// Builder pattern: set permission mode
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Builder pattern: set stuck detection threshold
    pub fn with_stuck_threshold(mut self, threshold: usize) -> Self {
        self.stuck_threshold = threshold;
        self
    }

    /// Builder pattern: set failure threshold
    pub fn with_failure_threshold(mut self, threshold: usize) -> Self {
        self.failure_threshold = threshold;
        self
    }

    /// Builder pattern: set doom loop threshold
    pub fn with_doom_loop_threshold(mut self, threshold: usize) -> Self {
        self.doom_loop_threshold = threshold;
        self
    }

    /// Builder pattern: set think retry configuration
    pub fn with_think_retry(mut self, config: ThinkRetryConfig) -> Self {
        self.think_retry = config;
        self
    }

    /// Builder pattern: enable unified session model
    pub fn with_unified_session(mut self, enabled: bool) -> Self {
        self.use_unified_session = enabled;
        self
    }

    /// Builder pattern: enable message builder
    pub fn with_message_builder(mut self, enabled: bool) -> Self {
        self.use_message_builder = enabled;
        self
    }

    /// Builder pattern: enable realtime overflow detection
    pub fn with_realtime_overflow(mut self, enabled: bool) -> Self {
        self.use_realtime_overflow = enabled;
        self
    }

    /// Enable all unified session model features
    pub fn with_all_unified_features(mut self) -> Self {
        self.use_unified_session = true;
        self.use_message_builder = true;
        self.use_realtime_overflow = true;
        self
    }

    /// Check if the current mode allows a specific tool
    ///
    /// In PlanMode, only read operations are allowed.
    /// Returns Ok(()) if allowed, Err with reason if blocked.
    pub fn check_tool_permission(&self, tool_name: &str) -> Result<(), String> {
        if self.permission_mode == PermissionMode::PlanMode {
            // In plan mode, only allow read operations
            let read_only_tools = [
                "read", "read_file", "search", "grep", "glob", "find",
                "list", "ls", "dir", "tree", "cat", "head", "tail",
                "git_status", "git_log", "git_diff", "git_show",
                "web_search", "web_fetch", "fetch_url",
                "ask_user", "ask_question",
            ];

            let tool_lower = tool_name.to_lowercase();
            let is_read_only = read_only_tools.iter().any(|t| {
                tool_lower == *t || tool_lower.starts_with(&format!("{}_", t))
            });

            if !is_read_only {
                return Err(format!(
                    "Plan mode is active. Tool '{}' is blocked because it may modify files or execute commands. \
                     Switch to Normal mode to enable write operations.",
                    tool_name
                ));
            }
        }
        Ok(())
    }

    /// Check if confirmation is needed for a tool
    pub fn needs_confirmation(&self, tool_name: &str) -> bool {
        match self.permission_mode {
            PermissionMode::PlanMode => {
                // Plan mode - all modifications are blocked, no confirmation needed
                false
            }
            PermissionMode::AutoAcceptEdits => {
                // Auto-accept edits - only confirm shell commands
                let shell_tools = ["shell", "execute", "run", "bash", "cmd", "exec"];
                shell_tools.iter().any(|t| tool_name.to_lowercase().contains(t))
            }
            PermissionMode::Normal => {
                // Normal mode - use the configured list
                self.require_confirmation.iter().any(|t| {
                    tool_name == t || tool_name.starts_with(t)
                })
            }
        }
    }

    /// Create a config for testing with lower limits
    pub fn for_testing() -> Self {
        Self {
            max_steps: 10,
            max_tokens: 10_000,
            timeout_secs: 300,
            timeout: Duration::from_secs(300),
            require_confirmation: vec![],
            permission_mode: PermissionMode::Normal,
            compression: CompressionConfig {
                compress_after_steps: 3,
                recent_window_size: 2,
                target_summary_tokens: 200,
                preserve_tool_outputs: true,
            },
            model_routing: ModelRoutingConfig::default(),
            enable_thinking_stream: false,
            persist_session: false,
            stuck_threshold: 3,
            failure_threshold: 3,
            doom_loop_threshold: 3,
            think_retry: ThinkRetryConfig {
                max_retries: 2,
                initial_backoff_ms: 100, // Faster for tests
                backoff_multiplier: 2.0,
                max_backoff_ms: 1000,
                respect_retry_after: true,
            },
            // Enable unified session features for testing
            use_unified_session: true,
            use_message_builder: true,
            use_realtime_overflow: true,
        }
    }

    /// Create a config for plan mode (read-only exploration)
    pub fn for_plan_mode() -> Self {
        Self {
            permission_mode: PermissionMode::PlanMode,
            ..Self::default()
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
        assert_eq!(config.permission_mode, PermissionMode::Normal);
    }

    #[test]
    fn test_builder_pattern() {
        let config = LoopConfig::new()
            .with_max_steps(100)
            .with_max_tokens(50_000)
            .with_timeout(Duration::from_secs(300))
            .add_confirmation_tool("custom_danger")
            .with_permission_mode(PermissionMode::PlanMode);

        assert_eq!(config.max_steps, 100);
        assert_eq!(config.max_tokens, 50_000);
        assert_eq!(config.timeout, Duration::from_secs(300));
        assert!(config.require_confirmation.contains(&"custom_danger".to_string()));
        assert_eq!(config.permission_mode, PermissionMode::PlanMode);
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

    #[test]
    fn test_permission_mode_properties() {
        assert!(PermissionMode::Normal.allows_writes());
        assert!(PermissionMode::AutoAcceptEdits.allows_writes());
        assert!(!PermissionMode::PlanMode.allows_writes());

        assert!(PermissionMode::Normal.requires_edit_confirmation());
        assert!(!PermissionMode::AutoAcceptEdits.requires_edit_confirmation());
        assert!(!PermissionMode::PlanMode.requires_edit_confirmation());
    }

    #[test]
    fn test_plan_mode_tool_permissions() {
        let config = LoopConfig::for_plan_mode();

        // Read operations should be allowed
        assert!(config.check_tool_permission("read_file").is_ok());
        assert!(config.check_tool_permission("search").is_ok());
        assert!(config.check_tool_permission("grep").is_ok());
        assert!(config.check_tool_permission("glob").is_ok());
        assert!(config.check_tool_permission("git_status").is_ok());
        assert!(config.check_tool_permission("web_search").is_ok());

        // Write operations should be blocked
        assert!(config.check_tool_permission("write_file").is_err());
        assert!(config.check_tool_permission("delete").is_err());
        assert!(config.check_tool_permission("execute_code").is_err());
        assert!(config.check_tool_permission("run_command").is_err());
    }

    #[test]
    fn test_normal_mode_all_tools_allowed() {
        let config = LoopConfig::default();

        // All tools should be allowed (permission-wise)
        assert!(config.check_tool_permission("read_file").is_ok());
        assert!(config.check_tool_permission("write_file").is_ok());
        assert!(config.check_tool_permission("delete").is_ok());
        assert!(config.check_tool_permission("execute_code").is_ok());
    }

    #[test]
    fn test_needs_confirmation() {
        let normal_config = LoopConfig::default();
        let auto_config = LoopConfig::default().with_permission_mode(PermissionMode::AutoAcceptEdits);
        let plan_config = LoopConfig::for_plan_mode();

        // Normal mode - dangerous tools need confirmation
        assert!(normal_config.needs_confirmation("write_file"));
        assert!(normal_config.needs_confirmation("delete"));

        // Auto-accept mode - only shell commands need confirmation
        assert!(!auto_config.needs_confirmation("write_file"));
        assert!(auto_config.needs_confirmation("shell_execute"));
        assert!(auto_config.needs_confirmation("bash"));

        // Plan mode - nothing needs confirmation (writes are blocked entirely)
        assert!(!plan_config.needs_confirmation("write_file"));
        assert!(!plan_config.needs_confirmation("delete"));
    }

    #[test]
    fn test_permission_mode_serialization() {
        let config = LoopConfig::default().with_permission_mode(PermissionMode::PlanMode);
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("PlanMode"));

        let parsed: LoopConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.permission_mode, PermissionMode::PlanMode);
    }

    #[test]
    fn test_doom_loop_threshold() {
        let config = LoopConfig::default();
        assert_eq!(config.doom_loop_threshold, 3);

        let config = LoopConfig::default().with_doom_loop_threshold(5);
        assert_eq!(config.doom_loop_threshold, 5);
    }

    #[test]
    fn test_think_retry_config_defaults() {
        let config = ThinkRetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_backoff_ms, 2000);
        assert_eq!(config.backoff_multiplier, 2.0);
        assert_eq!(config.max_backoff_ms, 30_000);
        assert!(config.respect_retry_after);
    }

    #[test]
    fn test_think_retry_calculate_delay() {
        let config = ThinkRetryConfig::default();

        // Attempt 0 should return zero
        assert_eq!(config.calculate_delay(0), Duration::ZERO);

        // Attempt 1: 2000ms
        assert_eq!(config.calculate_delay(1), Duration::from_millis(2000));

        // Attempt 2: 2000 * 2 = 4000ms
        assert_eq!(config.calculate_delay(2), Duration::from_millis(4000));

        // Attempt 3: 2000 * 4 = 8000ms
        assert_eq!(config.calculate_delay(3), Duration::from_millis(8000));

        // Attempt 4: 2000 * 8 = 16000ms
        assert_eq!(config.calculate_delay(4), Duration::from_millis(16000));

        // Attempt 5: 2000 * 16 = 32000ms, but capped at 30000ms
        assert_eq!(config.calculate_delay(5), Duration::from_millis(30000));
    }

    #[test]
    fn test_think_retry_with_retry_after() {
        let config = ThinkRetryConfig::default();

        // With retry-after header
        let delay = config.calculate_delay_with_retry_after(1, Some(5000));
        assert_eq!(delay, Duration::from_millis(5000));

        // With retry-after exceeding max
        let delay = config.calculate_delay_with_retry_after(1, Some(60000));
        assert_eq!(delay, Duration::from_millis(30000)); // Capped

        // Without retry-after header, use exponential backoff
        let delay = config.calculate_delay_with_retry_after(2, None);
        assert_eq!(delay, Duration::from_millis(4000));

        // With respect_retry_after = false
        let mut config = ThinkRetryConfig::default();
        config.respect_retry_after = false;
        let delay = config.calculate_delay_with_retry_after(1, Some(5000));
        assert_eq!(delay, Duration::from_millis(2000)); // Ignores retry-after
    }

    #[test]
    fn test_loop_config_with_think_retry() {
        let retry_config = ThinkRetryConfig {
            max_retries: 5,
            initial_backoff_ms: 1000,
            backoff_multiplier: 1.5,
            max_backoff_ms: 10000,
            respect_retry_after: false,
        };

        let config = LoopConfig::default().with_think_retry(retry_config.clone());
        assert_eq!(config.think_retry.max_retries, 5);
        assert_eq!(config.think_retry.initial_backoff_ms, 1000);
    }

    // ========================================================================
    // Unified Session Model Feature Flags Tests
    // ========================================================================

    #[test]
    fn test_unified_session_feature_flags_default() {
        let config = LoopConfig::default();

        // All flags disabled by default for safe rollout
        assert!(!config.use_unified_session);
        assert!(!config.use_message_builder);
        assert!(!config.use_realtime_overflow);
    }

    #[test]
    fn test_unified_session_feature_flags_testing() {
        let config = LoopConfig::for_testing();

        // Testing config enables all features
        assert!(config.use_unified_session);
        assert!(config.use_message_builder);
        assert!(config.use_realtime_overflow);
    }

    #[test]
    fn test_unified_session_builder_methods() {
        let config = LoopConfig::default()
            .with_unified_session(true)
            .with_message_builder(true)
            .with_realtime_overflow(true);

        assert!(config.use_unified_session);
        assert!(config.use_message_builder);
        assert!(config.use_realtime_overflow);
    }

    #[test]
    fn test_with_all_unified_features() {
        let config = LoopConfig::default().with_all_unified_features();

        assert!(config.use_unified_session);
        assert!(config.use_message_builder);
        assert!(config.use_realtime_overflow);
    }

    #[test]
    fn test_unified_session_serialization() {
        let config = LoopConfig::default().with_all_unified_features();
        let json = serde_json::to_string(&config).unwrap();

        assert!(json.contains("use_unified_session"));
        assert!(json.contains("use_message_builder"));
        assert!(json.contains("use_realtime_overflow"));

        let parsed: LoopConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.use_unified_session);
        assert!(parsed.use_message_builder);
        assert!(parsed.use_realtime_overflow);
    }
}
