//! Guard mechanisms for Agent Loop
//!
//! This module provides safety guards to prevent runaway loops,
//! excessive resource consumption, and dangerous operations.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use super::config::LoopConfig;
use super::state::LoopState;

/// Guard violation types
#[derive(Debug, Clone, PartialEq)]
pub enum GuardViolation {
    /// Maximum step count exceeded
    MaxSteps {
        current: usize,
        limit: usize,
    },
    /// Maximum token consumption exceeded
    MaxTokens {
        current: usize,
        limit: usize,
    },
    /// Execution timeout exceeded
    Timeout {
        elapsed: Duration,
        limit: Duration,
    },
    /// Stuck in loop (same action repeated)
    StuckLoop {
        action: String,
        repeat_count: usize,
    },
    /// Repeated failures on same action pattern
    RepeatedFailures {
        pattern: String,
        failure_count: usize,
        total_attempts: usize,
    },
    /// Doom loop detected: exact same tool call with identical arguments repeated
    /// This is more precise than StuckLoop as it checks both tool name AND arguments.
    /// Inspired by OpenCode's doom loop detection.
    DoomLoop {
        tool_name: String,
        repeat_count: usize,
        /// Preview of the arguments (truncated for display)
        arguments_preview: String,
    },
    /// POE suggests switching strategy due to lack of progress
    PoeStrategySwitch {
        reason: String,
        suggestion: String,
    },
}

impl GuardViolation {
    /// Get human-readable description
    pub fn description(&self) -> String {
        match self {
            GuardViolation::MaxSteps { current, limit } => {
                format!(
                    "Maximum step count exceeded: {} steps (limit: {})",
                    current, limit
                )
            }
            GuardViolation::MaxTokens { current, limit } => {
                format!(
                    "Maximum token usage exceeded: {} tokens (limit: {})",
                    current, limit
                )
            }
            GuardViolation::Timeout { elapsed, limit } => {
                format!(
                    "Execution timeout: {:.1}s (limit: {:.1}s)",
                    elapsed.as_secs_f64(),
                    limit.as_secs_f64()
                )
            }
            GuardViolation::StuckLoop {
                action,
                repeat_count,
            } => {
                format!(
                    "Stuck in loop: '{}' repeated {} times",
                    action, repeat_count
                )
            }
            GuardViolation::RepeatedFailures {
                pattern,
                failure_count,
                total_attempts,
            } => {
                format!(
                    "Repeated failures on '{}': {} failures out of {} attempts",
                    pattern, failure_count, total_attempts
                )
            }
            GuardViolation::DoomLoop {
                tool_name,
                repeat_count,
                arguments_preview,
            } => {
                format!(
                    "Doom loop detected: tool '{}' called {} times with identical arguments: {}",
                    tool_name, repeat_count, arguments_preview
                )
            }
            GuardViolation::PoeStrategySwitch { reason, suggestion } => {
                format!("POE strategy switch: {} — suggestion: {}", reason, suggestion)
            }
        }
    }

    /// Get suggestion for how to proceed
    pub fn suggestion(&self) -> String {
        match self {
            GuardViolation::MaxSteps { .. } => {
                "The task may be too complex. Try breaking it into smaller steps or providing more specific instructions.".to_string()
            }
            GuardViolation::MaxTokens { .. } => {
                "The conversation has grown too long. Consider starting a new session or summarizing the current state.".to_string()
            }
            GuardViolation::Timeout { .. } => {
                "The task is taking too long. Check if external services are responding correctly.".to_string()
            }
            GuardViolation::StuckLoop { .. } => {
                "The agent appears to be stuck. Please provide additional guidance or clarification.".to_string()
            }
            GuardViolation::RepeatedFailures { .. } => {
                "The same action keeps failing. This might indicate an environmental issue, missing permissions, or incorrect approach. Try a different strategy.".to_string()
            }
            GuardViolation::DoomLoop { .. } => {
                "The agent is making the exact same tool call repeatedly without progress. This usually indicates a logic error or misunderstanding. Please clarify your request or try a different approach.".to_string()
            }
            GuardViolation::PoeStrategySwitch { suggestion, .. } => {
                suggestion.clone()
            }
        }
    }
}

/// Tracked action with success/failure status
#[derive(Debug, Clone)]
struct TrackedAction {
    /// Normalized action identifier (e.g., "tool:read_file" or "tool:search")
    pattern: String,
    /// Whether this action succeeded
    succeeded: bool,
}

/// Tool call record for doom loop detection
///
/// Tracks precise tool calls including their arguments for detecting
/// exact repeated calls (doom loops).
#[derive(Debug, Clone)]
struct ToolCallRecord {
    /// The tool name
    tool_name: String,
    /// Hash of the arguments for efficient comparison
    arguments_hash: u64,
    /// Original arguments (for display in error message)
    arguments: serde_json::Value,
}

impl ToolCallRecord {
    /// Create a new tool call record
    fn new(tool_name: String, arguments: serde_json::Value) -> Self {
        // Compute hash of serialized arguments for efficient comparison
        let args_str = serde_json::to_string(&arguments).unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        args_str.hash(&mut hasher);
        let arguments_hash = hasher.finish();

        Self {
            tool_name,
            arguments_hash,
            arguments,
        }
    }

    /// Check if this record matches another (same tool name and arguments)
    fn matches(&self, other: &ToolCallRecord) -> bool {
        self.tool_name == other.tool_name && self.arguments_hash == other.arguments_hash
    }

    /// Get a truncated preview of arguments for display
    fn arguments_preview(&self, max_len: usize) -> String {
        let args_str = serde_json::to_string(&self.arguments).unwrap_or_default();
        if args_str.len() <= max_len {
            args_str
        } else {
            format!("{}...", &args_str[..max_len])
        }
    }
}

/// Guard checker for Agent Loop
pub struct LoopGuard {
    config: LoopConfig,
    /// Track recent actions with their success/failure status
    recent_actions: Vec<TrackedAction>,
    /// Maximum repeated identical actions before stuck detection
    stuck_threshold: usize,
    /// Maximum consecutive failures on similar actions before intervention
    failure_threshold: usize,
    /// Window size for failure pattern detection
    failure_window: usize,
    /// Track recent tool calls for doom loop detection
    recent_tool_calls: Vec<ToolCallRecord>,
    /// Doom loop detection threshold
    doom_loop_threshold: usize,
    /// Global tool call history (not cleared on reset)
    global_tool_history: Vec<ToolCallRecord>,
    /// Global doom loop threshold (higher than local)
    global_doom_threshold: usize,
}

impl LoopGuard {
    /// Create a new guard with given config
    pub fn new(config: LoopConfig) -> Self {
        let stuck_threshold = config.stuck_threshold;
        let failure_threshold = config.failure_threshold;
        let doom_loop_threshold = config.doom_loop_threshold;
        Self {
            config,
            recent_actions: Vec::new(),
            stuck_threshold,
            failure_threshold,
            failure_window: failure_threshold + 2, // Allow some headroom
            recent_tool_calls: Vec::new(),
            doom_loop_threshold,
            global_tool_history: Vec::new(),
            global_doom_threshold: 10, // Global threshold is higher
        }
    }

    /// Create a guard with custom thresholds
    pub fn with_thresholds(
        config: LoopConfig,
        stuck_threshold: usize,
        failure_threshold: usize,
    ) -> Self {
        let doom_loop_threshold = config.doom_loop_threshold;
        Self {
            config,
            recent_actions: Vec::new(),
            stuck_threshold,
            failure_threshold,
            failure_window: failure_threshold + 2,
            recent_tool_calls: Vec::new(),
            doom_loop_threshold,
            global_tool_history: Vec::new(),
            global_doom_threshold: 10,
        }
    }

    /// Create a guard with custom doom loop threshold
    pub fn with_doom_loop_threshold(mut self, threshold: usize) -> Self {
        self.doom_loop_threshold = threshold;
        self
    }

    /// Check all guards and return violation if any
    pub fn check(&self, state: &LoopState) -> Option<GuardViolation> {
        // Check step limit
        if state.step_count >= self.config.max_steps {
            return Some(GuardViolation::MaxSteps {
                current: state.step_count,
                limit: self.config.max_steps,
            });
        }

        // Check token limit
        if state.total_tokens >= self.config.max_tokens {
            return Some(GuardViolation::MaxTokens {
                current: state.total_tokens,
                limit: self.config.max_tokens,
            });
        }

        // Check timeout
        let elapsed = state.elapsed();
        if elapsed >= self.config.timeout {
            return Some(GuardViolation::Timeout {
                elapsed,
                limit: self.config.timeout,
            });
        }

        // Check for doom loop (exact same tool call repeated) - most precise check
        if let Some(violation) = self.check_doom_loop() {
            return Some(violation);
        }

        // Check for global doom loop (across resets)
        if let Some(violation) = self.check_global_doom_loop() {
            return Some(violation);
        }

        // Check for stuck loop (same action repeated)
        if let Some(violation) = self.check_stuck() {
            return Some(violation);
        }

        // Check for repeated failures on similar actions
        if let Some(violation) = self.check_repeated_failures() {
            return Some(violation);
        }

        None
    }

    /// Record an action with its success/failure status
    pub fn record_action(&mut self, action: &str) {
        self.record_action_with_result(action, true);
    }

    /// Record an action with explicit success/failure status
    pub fn record_action_with_result(&mut self, action: &str, succeeded: bool) {
        // Normalize action pattern for grouping similar actions
        let pattern = Self::normalize_action_pattern(action);

        self.recent_actions.push(TrackedAction { pattern, succeeded });

        // Keep history bounded
        let max_history = self.stuck_threshold.max(self.failure_window) * 2;
        if self.recent_actions.len() > max_history {
            self.recent_actions.remove(0);
        }
    }

    /// Record a tool call for doom loop detection
    ///
    /// This should be called for every tool call with the exact arguments.
    pub fn record_tool_call(&mut self, tool_name: &str, arguments: &serde_json::Value) {
        let record = ToolCallRecord::new(tool_name.to_string(), arguments.clone());

        // Add to recent (will be cleared on reset)
        self.recent_tool_calls.push(record.clone());

        // Keep bounded
        let max_history = self.doom_loop_threshold * 2;
        if self.recent_tool_calls.len() > max_history {
            self.recent_tool_calls.remove(0);
        }

        // Add to global history (persists across resets)
        self.global_tool_history.push(record);

        // Keep global history bounded
        let max_global = self.global_doom_threshold * 3;
        if self.global_tool_history.len() > max_global {
            self.global_tool_history.remove(0);
        }
    }

    /// Check for doom loop (exact same tool call with identical arguments repeated)
    ///
    /// This is more precise than check_stuck() as it compares exact argument values.
    /// Inspired by OpenCode's doom loop detection.
    fn check_doom_loop(&self) -> Option<GuardViolation> {
        if self.recent_tool_calls.len() < self.doom_loop_threshold {
            return None;
        }

        // Get last N tool calls
        let last_n = &self.recent_tool_calls
            [self.recent_tool_calls.len() - self.doom_loop_threshold..];
        let first = &last_n[0];

        // Check if all last N calls are identical (same tool + same arguments)
        if last_n.iter().all(|call| call.matches(first)) {
            return Some(GuardViolation::DoomLoop {
                tool_name: first.tool_name.clone(),
                repeat_count: self.doom_loop_threshold,
                arguments_preview: first.arguments_preview(100),
            });
        }

        None
    }

    /// Check for global doom loop (across resets)
    ///
    /// This catches patterns where the user manually continues through
    /// multiple doom loop warnings, but the overall session still shows
    /// excessive repetition.
    fn check_global_doom_loop(&self) -> Option<GuardViolation> {
        if self.global_tool_history.len() < self.global_doom_threshold {
            return None;
        }

        // Count occurrences of each unique tool call
        let mut call_counts: std::collections::HashMap<(String, u64), usize> =
            std::collections::HashMap::new();

        for record in &self.global_tool_history {
            let key = (record.tool_name.clone(), record.arguments_hash);
            *call_counts.entry(key).or_insert(0) += 1;
        }

        // Find the most repeated call
        if let Some((tool_key, count)) = call_counts.iter().max_by_key(|(_, count)| *count) {
            if *count >= self.global_doom_threshold {
                // Find a representative record for preview
                let representative = self
                    .global_tool_history
                    .iter()
                    .find(|r| {
                        r.tool_name == tool_key.0 && r.arguments_hash == tool_key.1
                    })
                    .unwrap();

                return Some(GuardViolation::DoomLoop {
                    tool_name: tool_key.0.clone(),
                    repeat_count: *count,
                    arguments_preview: representative.arguments_preview(100),
                });
            }
        }

        None
    }

    /// Normalize action pattern for grouping
    ///
    /// For tool calls, the format is "tool:name" or "tool:name:operation" where
    /// operation is a semantic operation type (like "mkdir", "write", "read").
    /// The operation is preserved to distinguish different operations on the same tool.
    ///
    /// Examples:
    /// - "search:query about rust" -> "search"
    /// - "read_file:/path/to/file.rs" -> "read_file"
    /// - "tool:write_file" -> "tool:write_file"
    /// - "tool:file_ops:mkdir" -> "tool:file_ops:mkdir"
    /// - "tool:file_ops:write" -> "tool:file_ops:write"
    fn normalize_action_pattern(action: &str) -> String {
        // Split on first colon to get tool/action type
        if let Some(colon_pos) = action.find(':') {
            let prefix = &action[..colon_pos];
            // If prefix is "tool", preserve the full pattern including operation
            // Format: "tool:name" or "tool:name:operation"
            if prefix == "tool" {
                // Return the entire action string as-is
                // The action_type() method already formats it properly
                return action.to_string();
            }
            prefix.to_string()
        } else {
            action.to_string()
        }
    }

    /// Check if stuck in a loop (identical actions)
    ///
    /// CRITICAL FIX: This check now considers execution progress.
    /// If the agent is making the same type of action repeatedly BUT:
    /// - Some succeed and some fail (debugging/iterating)
    /// - At least one succeeds (making progress)
    ///
    /// Then it's NOT stuck, it's making progress through iteration.
    ///
    /// Only triggers if:
    /// - Same action pattern repeated N times
    /// - AND all attempts have the same result (all succeed or all fail)
    /// - This indicates true stuckness with no progress
    fn check_stuck(&self) -> Option<GuardViolation> {
        if self.recent_actions.len() < self.stuck_threshold {
            return None;
        }

        // Get last N actions' patterns
        let last_n = &self.recent_actions[self.recent_actions.len() - self.stuck_threshold..];
        let first_pattern = &last_n[0].pattern;

        // Check if all last N actions have identical patterns
        if !last_n.iter().all(|a| &a.pattern == first_pattern) {
            return None;
        }

        // CRITICAL FIX: Check if there's execution progress
        // If results vary (some succeed, some fail), the agent is making progress
        let has_success = last_n.iter().any(|a| a.succeeded);
        let has_failure = last_n.iter().any(|a| !a.succeeded);

        // If there's variation in results, agent is iterating/debugging (not stuck)
        if has_success && has_failure {
            return None;
        }

        // If all succeeded, it might still be making progress
        // (e.g., writing multiple files in sequence)
        // Only flag if all failed (indicating a real problem)
        if has_success && !has_failure {
            // All succeeded - check if it's a bash/code_exec tool
            // These tools often need multiple iterations for debugging
            if first_pattern.contains("bash") || first_pattern.contains("code_exec") {
                return None; // Allow bash/code_exec iteration for debugging
            }
        }

        // All attempts with same pattern and same result = truly stuck
        Some(GuardViolation::StuckLoop {
            action: first_pattern.clone(),
            repeat_count: self.stuck_threshold,
        })
    }

    /// Check for repeated failures on similar action patterns
    ///
    /// This catches scenarios where the agent keeps trying similar approaches
    /// that consistently fail, even if the exact parameters differ.
    fn check_repeated_failures(&self) -> Option<GuardViolation> {
        if self.recent_actions.len() < self.failure_threshold {
            return None;
        }

        // Look at the failure window
        let window_start = self.recent_actions.len().saturating_sub(self.failure_window);
        let window = &self.recent_actions[window_start..];

        // Group by pattern and count failures
        let mut pattern_stats: std::collections::HashMap<&str, (usize, usize)> = std::collections::HashMap::new();

        for action in window {
            let entry = pattern_stats.entry(&action.pattern).or_insert((0, 0));
            entry.1 += 1; // total attempts
            if !action.succeeded {
                entry.0 += 1; // failures
            }
        }

        // Check if any pattern has too many failures
        for (pattern, (failures, total)) in pattern_stats {
            if failures >= self.failure_threshold {
                return Some(GuardViolation::RepeatedFailures {
                    pattern: pattern.to_string(),
                    failure_count: failures,
                    total_attempts: total,
                });
            }
        }

        None
    }

    /// Check if a tool requires confirmation
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        self.config
            .require_confirmation
            .iter()
            .any(|t| t == tool_name || tool_name.starts_with(t))
    }

    /// Get remaining steps before limit
    pub fn remaining_steps(&self, state: &LoopState) -> usize {
        self.config.max_steps.saturating_sub(state.step_count)
    }

    /// Get remaining tokens before limit
    pub fn remaining_tokens(&self, state: &LoopState) -> usize {
        self.config.max_tokens.saturating_sub(state.total_tokens)
    }

    /// Get remaining time before timeout
    pub fn remaining_time(&self, state: &LoopState) -> Duration {
        self.config.timeout.saturating_sub(state.elapsed())
    }

    /// Reset stuck detection
    pub fn reset_stuck_detection(&mut self) {
        self.recent_actions.clear();
    }

    /// Reset doom loop detection
    pub fn reset_doom_loop_detection(&mut self) {
        self.recent_tool_calls.clear();
    }

    /// Reset all loop detection (stuck and doom loop)
    pub fn reset_all_detection(&mut self) {
        self.recent_actions.clear();
        self.recent_tool_calls.clear();
    }

    /// Get doom loop threshold
    pub fn doom_loop_threshold(&self) -> usize {
        self.doom_loop_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::state::RequestContext;

    fn create_test_config() -> LoopConfig {
        LoopConfig {
            max_steps: 10,
            max_tokens: 1000,
            timeout: Duration::from_secs(60),
            require_confirmation: vec!["delete".to_string(), "write_file".to_string()],
            stuck_threshold: 3,      // Use lower threshold for tests
            failure_threshold: 3,    // Use lower threshold for tests
            ..Default::default()
        }
    }

    #[test]
    fn test_max_steps_guard() {
        let config = create_test_config();
        let guard = LoopGuard::new(config);

        let mut state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );
        state.step_count = 10;

        let violation = guard.check(&state);
        assert!(matches!(violation, Some(GuardViolation::MaxSteps { .. })));
    }

    #[test]
    fn test_max_tokens_guard() {
        let config = create_test_config();
        let guard = LoopGuard::new(config);

        let mut state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );
        state.total_tokens = 1000;

        let violation = guard.check(&state);
        assert!(matches!(violation, Some(GuardViolation::MaxTokens { .. })));
    }

    #[test]
    fn test_stuck_detection() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Record same action pattern 3 times (all succeed)
        guard.record_action_with_result("search:query1", true);
        guard.record_action_with_result("search:query2", true);
        guard.record_action_with_result("search:query3", true);

        // Pattern normalizes to "search", so all 3 are identical
        let violation = guard.check_stuck();
        assert!(matches!(violation, Some(GuardViolation::StuckLoop { .. })));
    }

    #[test]
    fn test_no_stuck_with_varied_actions() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        guard.record_action_with_result("search:query1", true);
        guard.record_action_with_result("read_file:path", true);
        guard.record_action_with_result("write_file:path", true);

        let violation = guard.check_stuck();
        assert!(violation.is_none());
    }

    #[test]
    fn test_repeated_failures_detection() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Record 3 failures on similar action
        guard.record_action_with_result("search:query1", false);
        guard.record_action_with_result("search:query2", false);
        guard.record_action_with_result("search:query3", false);

        let violation = guard.check_repeated_failures();
        assert!(matches!(
            violation,
            Some(GuardViolation::RepeatedFailures { failure_count: 3, .. })
        ));
    }

    #[test]
    fn test_mixed_success_failure_no_violation() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Mix of successes and failures - should not trigger
        guard.record_action_with_result("search:query1", false);
        guard.record_action_with_result("search:query2", true);
        guard.record_action_with_result("search:query3", false);
        guard.record_action_with_result("search:query4", true);
        guard.record_action_with_result("search:query5", false);

        let violation = guard.check_repeated_failures();
        // Only 3 failures, but interleaved with successes - still triggers threshold
        assert!(violation.is_some());
    }

    #[test]
    fn test_different_patterns_no_repeated_failure() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Failures on different patterns - should not trigger
        guard.record_action_with_result("search:query1", false);
        guard.record_action_with_result("read_file:path1", false);
        guard.record_action_with_result("write_file:path1", false);

        let violation = guard.check_repeated_failures();
        assert!(violation.is_none());
    }

    #[test]
    fn test_action_pattern_normalization() {
        // Non-tool patterns: extract prefix before first colon
        assert_eq!(LoopGuard::normalize_action_pattern("search:query about rust"), "search");
        assert_eq!(LoopGuard::normalize_action_pattern("read_file:/path/to/file.rs"), "read_file");
        assert_eq!(LoopGuard::normalize_action_pattern("simple_action"), "simple_action");

        // Tool patterns: preserve full pattern including operation
        assert_eq!(LoopGuard::normalize_action_pattern("tool:write_file"), "tool:write_file");
        assert_eq!(LoopGuard::normalize_action_pattern("tool:tool_0"), "tool:tool_0");
        assert_eq!(LoopGuard::normalize_action_pattern("tool:read_file"), "tool:read_file");

        // Tool patterns with operation: preserve operation type
        assert_eq!(LoopGuard::normalize_action_pattern("tool:file_ops:mkdir"), "tool:file_ops:mkdir");
        assert_eq!(LoopGuard::normalize_action_pattern("tool:file_ops:write"), "tool:file_ops:write");
        assert_eq!(LoopGuard::normalize_action_pattern("tool:file_ops:read"), "tool:file_ops:read");
    }

    #[test]
    fn test_custom_thresholds() {
        let config = create_test_config();
        let mut guard = LoopGuard::with_thresholds(config, 5, 4);

        // 3 failures should not trigger with threshold of 4
        guard.record_action_with_result("search:q1", false);
        guard.record_action_with_result("search:q2", false);
        guard.record_action_with_result("search:q3", false);

        assert!(guard.check_repeated_failures().is_none());

        // 4th failure should trigger
        guard.record_action_with_result("search:q4", false);
        assert!(guard.check_repeated_failures().is_some());
    }

    #[test]
    fn test_requires_confirmation() {
        let config = create_test_config();
        let guard = LoopGuard::new(config);

        assert!(guard.requires_confirmation("delete"));
        assert!(guard.requires_confirmation("delete_file"));
        assert!(guard.requires_confirmation("write_file"));
        assert!(!guard.requires_confirmation("read_file"));
        assert!(!guard.requires_confirmation("search"));
    }

    #[test]
    fn test_guard_violation_descriptions() {
        let violations = vec![
            GuardViolation::MaxSteps { current: 10, limit: 10 },
            GuardViolation::MaxTokens { current: 1000, limit: 1000 },
            GuardViolation::Timeout { elapsed: Duration::from_secs(60), limit: Duration::from_secs(60) },
            GuardViolation::StuckLoop { action: "search".to_string(), repeat_count: 3 },
            GuardViolation::RepeatedFailures { pattern: "search".to_string(), failure_count: 3, total_attempts: 5 },
            GuardViolation::DoomLoop { tool_name: "web_search".to_string(), repeat_count: 3, arguments_preview: r#"{"query": "test"}"#.to_string() },
            GuardViolation::PoeStrategySwitch {
                reason: "no progress".to_string(),
                suggestion: "try a different approach".to_string(),
            },
        ];

        for violation in violations {
            assert!(!violation.description().is_empty());
            assert!(!violation.suggestion().is_empty());
        }
    }

    #[test]
    fn test_doom_loop_detection() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Record 3 identical tool calls
        let args = serde_json::json!({"query": "same query"});
        guard.record_tool_call("web_search", &args);
        guard.record_tool_call("web_search", &args);
        guard.record_tool_call("web_search", &args);

        let violation = guard.check_doom_loop();
        assert!(violation.is_some());
        if let Some(GuardViolation::DoomLoop { tool_name, repeat_count, .. }) = violation {
            assert_eq!(tool_name, "web_search");
            assert_eq!(repeat_count, 3);
        } else {
            panic!("Expected DoomLoop violation");
        }
    }

    #[test]
    fn test_no_doom_loop_with_different_args() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Same tool, different arguments - should NOT trigger doom loop
        guard.record_tool_call("web_search", &serde_json::json!({"query": "query1"}));
        guard.record_tool_call("web_search", &serde_json::json!({"query": "query2"}));
        guard.record_tool_call("web_search", &serde_json::json!({"query": "query3"}));

        let violation = guard.check_doom_loop();
        assert!(violation.is_none());
    }

    #[test]
    fn test_no_doom_loop_with_different_tools() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Different tools, same arguments - should NOT trigger doom loop
        let args = serde_json::json!({"path": "/test"});
        guard.record_tool_call("read_file", &args);
        guard.record_tool_call("write_file", &args);
        guard.record_tool_call("delete_file", &args);

        let violation = guard.check_doom_loop();
        assert!(violation.is_none());
    }

    #[test]
    fn test_doom_loop_with_complex_args() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        // Complex nested arguments
        let args = serde_json::json!({
            "options": {
                "recursive": true,
                "depth": 3,
                "filters": ["*.rs", "*.toml"]
            },
            "path": "/some/path"
        });

        guard.record_tool_call("search", &args);
        guard.record_tool_call("search", &args);
        guard.record_tool_call("search", &args);

        let violation = guard.check_doom_loop();
        assert!(violation.is_some());
    }

    #[test]
    fn test_doom_loop_reset() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        let args = serde_json::json!({"query": "test"});
        guard.record_tool_call("search", &args);
        guard.record_tool_call("search", &args);
        guard.record_tool_call("search", &args);

        assert!(guard.check_doom_loop().is_some());

        guard.reset_doom_loop_detection();
        assert!(guard.check_doom_loop().is_none());
    }

    #[test]
    fn test_tool_call_record_matching() {
        let args1 = serde_json::json!({"a": 1, "b": 2});
        let args2 = serde_json::json!({"a": 1, "b": 2});
        let args3 = serde_json::json!({"a": 1, "b": 3});

        let record1 = ToolCallRecord::new("tool".to_string(), args1);
        let record2 = ToolCallRecord::new("tool".to_string(), args2);
        let record3 = ToolCallRecord::new("tool".to_string(), args3);
        let record4 = ToolCallRecord::new("other_tool".to_string(), serde_json::json!({"a": 1, "b": 2}));

        // Same tool, same args
        assert!(record1.matches(&record2));

        // Same tool, different args
        assert!(!record1.matches(&record3));

        // Different tool, same args
        assert!(!record1.matches(&record4));
    }

    #[test]
    fn test_arguments_preview_truncation() {
        let long_args = serde_json::json!({
            "very_long_key": "a".repeat(200)
        });
        let record = ToolCallRecord::new("tool".to_string(), long_args);

        let preview = record.arguments_preview(50);
        assert!(preview.len() <= 53); // 50 + "..."
        assert!(preview.ends_with("..."));
    }
}
