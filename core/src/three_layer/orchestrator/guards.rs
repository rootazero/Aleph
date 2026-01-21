//! Guard checking for Orchestrator hard constraints

use crate::config::types::OrchestratorGuards;
use std::time::{Duration, Instant};

/// Violation of an orchestrator guard
#[derive(Debug, Clone)]
pub enum GuardViolation {
    /// Maximum rounds exceeded
    MaxRoundsExceeded { current: u32, max: u32 },
    /// Maximum tool calls exceeded
    MaxToolCallsExceeded { current: u32, max: u32 },
    /// Token budget exhausted
    TokenBudgetExhausted { current: u64, max: u64 },
    /// Timeout reached
    Timeout { elapsed: Duration, max: Duration },
    /// No progress detected
    NoProgress { rounds_without_progress: u32, threshold: u32 },
}

impl std::fmt::Display for GuardViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuardViolation::MaxRoundsExceeded { current, max } => {
                write!(f, "Maximum rounds exceeded: {} >= {}", current, max)
            }
            GuardViolation::MaxToolCallsExceeded { current, max } => {
                write!(f, "Maximum tool calls exceeded: {} >= {}", current, max)
            }
            GuardViolation::TokenBudgetExhausted { current, max } => {
                write!(f, "Token budget exhausted: {} >= {}", current, max)
            }
            GuardViolation::Timeout { elapsed, max } => {
                write!(f, "Timeout: {:?} >= {:?}", elapsed, max)
            }
            GuardViolation::NoProgress { rounds_without_progress, threshold } => {
                write!(
                    f,
                    "No progress for {} rounds (threshold: {})",
                    rounds_without_progress, threshold
                )
            }
        }
    }
}

impl std::error::Error for GuardViolation {}

/// Checker for orchestrator guards
#[derive(Debug, Clone)]
pub struct GuardChecker {
    guards: OrchestratorGuards,
}

impl GuardChecker {
    /// Create a new guard checker with the given configuration
    pub fn new(guards: OrchestratorGuards) -> Self {
        Self { guards }
    }

    /// Check if rounds limit is exceeded
    pub fn check_rounds(&self, current: u32) -> Result<(), GuardViolation> {
        if self.guards.is_rounds_exceeded(current) {
            Err(GuardViolation::MaxRoundsExceeded {
                current,
                max: self.guards.max_rounds,
            })
        } else {
            Ok(())
        }
    }

    /// Check if tool calls limit is exceeded
    pub fn check_tool_calls(&self, current: u32) -> Result<(), GuardViolation> {
        if self.guards.is_tool_calls_exceeded(current) {
            Err(GuardViolation::MaxToolCallsExceeded {
                current,
                max: self.guards.max_tool_calls,
            })
        } else {
            Ok(())
        }
    }

    /// Check if token budget is exceeded
    pub fn check_tokens(&self, current: u64) -> Result<(), GuardViolation> {
        if self.guards.is_tokens_exceeded(current) {
            Err(GuardViolation::TokenBudgetExhausted {
                current,
                max: self.guards.max_tokens,
            })
        } else {
            Ok(())
        }
    }

    /// Check if timeout is exceeded
    pub fn check_timeout(&self, start: Instant) -> Result<(), GuardViolation> {
        let elapsed = start.elapsed();
        let max = self.guards.timeout();
        if elapsed >= max {
            Err(GuardViolation::Timeout { elapsed, max })
        } else {
            Ok(())
        }
    }

    /// Check if no progress threshold is reached
    pub fn check_progress(&self, rounds_without_progress: u32) -> Result<(), GuardViolation> {
        if rounds_without_progress >= self.guards.no_progress_threshold {
            Err(GuardViolation::NoProgress {
                rounds_without_progress,
                threshold: self.guards.no_progress_threshold,
            })
        } else {
            Ok(())
        }
    }

    /// Check all guards at once
    pub fn check_all(
        &self,
        rounds: u32,
        tool_calls: u32,
        tokens: u64,
        start: Instant,
        rounds_without_progress: u32,
    ) -> Result<(), GuardViolation> {
        self.check_rounds(rounds)?;
        self.check_tool_calls(tool_calls)?;
        self.check_tokens(tokens)?;
        self.check_timeout(start)?;
        self.check_progress(rounds_without_progress)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::OrchestratorGuards;

    #[test]
    fn test_guard_checker_rounds() {
        let guards = OrchestratorGuards::default();
        let checker = GuardChecker::new(guards);

        assert!(checker.check_rounds(5).is_ok());
        assert!(checker.check_rounds(12).is_err());

        if let Err(GuardViolation::MaxRoundsExceeded { current, max }) = checker.check_rounds(15) {
            assert_eq!(current, 15);
            assert_eq!(max, 12);
        } else {
            panic!("Expected MaxRoundsExceeded");
        }
    }

    #[test]
    fn test_guard_checker_tokens() {
        let guards = OrchestratorGuards::default();
        let checker = GuardChecker::new(guards);

        assert!(checker.check_tokens(50_000).is_ok());
        assert!(checker.check_tokens(100_000).is_err());
    }

    #[test]
    fn test_guard_checker_no_progress() {
        let guards = OrchestratorGuards::default();
        let checker = GuardChecker::new(guards);

        assert!(checker.check_progress(1).is_ok());
        assert!(checker.check_progress(2).is_err());
    }

    #[test]
    fn test_guard_violation_display() {
        let violation = GuardViolation::MaxRoundsExceeded { current: 15, max: 12 };
        let display = format!("{}", violation);
        assert!(display.contains("15"));
        assert!(display.contains("12"));
    }
}
