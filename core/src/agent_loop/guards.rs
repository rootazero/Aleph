//! Guard mechanisms for Agent Loop
//!
//! This module provides safety guards to prevent runaway loops,
//! excessive resource consumption, and dangerous operations.

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
        }
    }
}

/// Guard checker for Agent Loop
pub struct LoopGuard {
    config: LoopConfig,
    /// Track recent actions for stuck detection
    recent_actions: Vec<String>,
    /// Maximum repeated actions before stuck detection
    stuck_threshold: usize,
}

impl LoopGuard {
    /// Create a new guard with given config
    pub fn new(config: LoopConfig) -> Self {
        Self {
            config,
            recent_actions: Vec::new(),
            stuck_threshold: 3, // Same action 3 times = stuck
        }
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

        // Check for stuck loop
        if let Some(violation) = self.check_stuck() {
            return Some(violation);
        }

        None
    }

    /// Record an action for stuck detection
    pub fn record_action(&mut self, action: &str) {
        self.recent_actions.push(action.to_string());

        // Keep only recent actions
        if self.recent_actions.len() > self.stuck_threshold * 2 {
            self.recent_actions.remove(0);
        }
    }

    /// Check if stuck in a loop
    fn check_stuck(&self) -> Option<GuardViolation> {
        if self.recent_actions.len() < self.stuck_threshold {
            return None;
        }

        // Check if last N actions are identical
        let last_n = &self.recent_actions[self.recent_actions.len() - self.stuck_threshold..];
        let first = &last_n[0];

        if last_n.iter().all(|a| a == first) {
            return Some(GuardViolation::StuckLoop {
                action: first.clone(),
                repeat_count: self.stuck_threshold,
            });
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

        // Record same action 3 times
        guard.record_action("search:query");
        guard.record_action("search:query");
        guard.record_action("search:query");

        let violation = guard.check_stuck();
        assert!(matches!(violation, Some(GuardViolation::StuckLoop { .. })));
    }

    #[test]
    fn test_no_stuck_with_varied_actions() {
        let config = create_test_config();
        let mut guard = LoopGuard::new(config);

        guard.record_action("search:query1");
        guard.record_action("read_file:path");
        guard.record_action("search:query2");

        let violation = guard.check_stuck();
        assert!(violation.is_none());
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
}
