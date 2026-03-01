//! Quota Manager
//!
//! Implements concurrency and resource quota enforcement
//! for the Multi-Agent Resilience architecture.

use crate::error::AlephError;
use super::super::types::SessionStatus;
use crate::resilience::database::StateDatabase;
use crate::sync_primitives::Arc;
use tracing::{debug, info};

/// Quota configuration
#[derive(Debug, Clone)]
pub struct QuotaConfig {
    /// Maximum concurrent running subagents
    pub max_running: usize,

    /// Maximum idle subagents in memory
    pub max_idle: usize,

    /// Maximum recursion depth
    pub max_depth: u32,

    /// Token budget per session
    pub token_budget: u64,

    /// Maximum total subagents (including swapped)
    pub max_total: usize,

    /// Maximum tool calls per task
    pub max_tool_calls_per_task: u64,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            max_running: 5,
            max_idle: 10,
            max_depth: 3,
            token_budget: 100_000,
            max_total: 50,
            max_tool_calls_per_task: 100,
        }
    }
}

/// Quota violation types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaViolation {
    /// Too many running subagents
    MaxRunningExceeded { current: usize, limit: usize },

    /// Too many idle subagents in memory
    MaxIdleExceeded { current: usize, limit: usize },

    /// Recursion depth limit exceeded
    MaxDepthExceeded { current: u32, limit: u32 },

    /// Token budget exceeded
    TokenBudgetExceeded { used: u64, budget: u64 },

    /// Total subagent limit exceeded
    MaxTotalExceeded { current: usize, limit: usize },

    /// Tool calls limit exceeded
    MaxToolCallsExceeded { current: u64, limit: u64 },
}

impl std::fmt::Display for QuotaViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuotaViolation::MaxRunningExceeded { current, limit } => {
                write!(f, "Max running subagents exceeded: {} (limit: {})", current, limit)
            }
            QuotaViolation::MaxIdleExceeded { current, limit } => {
                write!(f, "Max idle subagents exceeded: {} (limit: {})", current, limit)
            }
            QuotaViolation::MaxDepthExceeded { current, limit } => {
                write!(f, "Max recursion depth exceeded: {} (limit: {})", current, limit)
            }
            QuotaViolation::TokenBudgetExceeded { used, budget } => {
                write!(f, "Token budget exceeded: {} (budget: {})", used, budget)
            }
            QuotaViolation::MaxTotalExceeded { current, limit } => {
                write!(f, "Max total subagents exceeded: {} (limit: {})", current, limit)
            }
            QuotaViolation::MaxToolCallsExceeded { current, limit } => {
                write!(f, "Max tool calls exceeded: {} (limit: {})", current, limit)
            }
        }
    }
}

/// Quota check result
#[derive(Debug, Clone)]
pub struct QuotaCheckResult {
    /// Whether quotas are satisfied
    pub passed: bool,

    /// List of violations (empty if passed)
    pub violations: Vec<QuotaViolation>,

    /// Current usage snapshot
    pub usage: QuotaUsage,
}

/// Current quota usage
#[derive(Debug, Clone, Default)]
pub struct QuotaUsage {
    pub running_subagents: usize,
    pub idle_subagents: usize,
    pub swapped_subagents: usize,
    pub total_subagents: usize,
}

/// Quota Manager for enforcing resource limits
pub struct QuotaManager {
    db: Arc<StateDatabase>,
    config: QuotaConfig,
}

impl QuotaManager {
    /// Create a new Quota Manager
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self::with_config(db, QuotaConfig::default())
    }

    /// Create a Quota Manager with custom config
    pub fn with_config(db: Arc<StateDatabase>, config: QuotaConfig) -> Self {
        info!(
            max_running = config.max_running,
            max_idle = config.max_idle,
            max_depth = config.max_depth,
            "QuotaManager initialized"
        );

        Self { db, config }
    }

    /// Check if a new subagent can be spawned
    pub async fn check_spawn(&self, depth: u32) -> Result<QuotaCheckResult, AlephError> {
        let usage = self.get_usage().await?;
        let mut violations = Vec::new();

        // Check running limit
        if usage.running_subagents >= self.config.max_running {
            violations.push(QuotaViolation::MaxRunningExceeded {
                current: usage.running_subagents,
                limit: self.config.max_running,
            });
        }

        // Check total limit
        if usage.total_subagents >= self.config.max_total {
            violations.push(QuotaViolation::MaxTotalExceeded {
                current: usage.total_subagents,
                limit: self.config.max_total,
            });
        }

        // Check depth limit
        if depth > self.config.max_depth {
            violations.push(QuotaViolation::MaxDepthExceeded {
                current: depth,
                limit: self.config.max_depth,
            });
        }

        let passed = violations.is_empty();

        if !passed {
            debug!(
                violations = ?violations,
                "Quota check failed for spawn"
            );
        }

        Ok(QuotaCheckResult {
            passed,
            violations,
            usage,
        })
    }

    /// Check if tokens can be consumed
    pub fn check_tokens(&self, used: u64) -> Result<(), QuotaViolation> {
        if used > self.config.token_budget {
            Err(QuotaViolation::TokenBudgetExceeded {
                used,
                budget: self.config.token_budget,
            })
        } else {
            Ok(())
        }
    }

    /// Check if tool calls limit is reached
    pub fn check_tool_calls(&self, current: u64) -> Result<(), QuotaViolation> {
        if current >= self.config.max_tool_calls_per_task {
            Err(QuotaViolation::MaxToolCallsExceeded {
                current,
                limit: self.config.max_tool_calls_per_task,
            })
        } else {
            Ok(())
        }
    }

    /// Check if idle sessions need cleanup (swap)
    pub async fn check_idle_pressure(&self) -> Result<bool, AlephError> {
        let idle_count = self
            .db
            .count_sessions_by_status(SessionStatus::Idle)
            .await?;

        let needs_cleanup = idle_count > self.config.max_idle as u64;

        if needs_cleanup {
            debug!(
                idle_count = idle_count,
                max_idle = self.config.max_idle,
                "Idle pressure detected, cleanup needed"
            );
        }

        Ok(needs_cleanup)
    }

    /// Get current quota usage
    pub async fn get_usage(&self) -> Result<QuotaUsage, AlephError> {
        let running = self
            .db
            .count_sessions_by_status(SessionStatus::Active)
            .await? as usize;

        let idle = self
            .db
            .count_sessions_by_status(SessionStatus::Idle)
            .await? as usize;

        let swapped = self
            .db
            .count_sessions_by_status(SessionStatus::Swapped)
            .await? as usize;

        Ok(QuotaUsage {
            running_subagents: running,
            idle_subagents: idle,
            swapped_subagents: swapped,
            total_subagents: running + idle + swapped,
        })
    }

    /// Get remaining capacity for new subagents
    pub async fn get_remaining_capacity(&self) -> Result<RemainingCapacity, AlephError> {
        let usage = self.get_usage().await?;

        Ok(RemainingCapacity {
            running: self.config.max_running.saturating_sub(usage.running_subagents),
            idle: self.config.max_idle.saturating_sub(usage.idle_subagents),
            total: self.config.max_total.saturating_sub(usage.total_subagents),
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &QuotaConfig {
        &self.config
    }
}

/// Remaining capacity for resources
#[derive(Debug, Clone)]
pub struct RemainingCapacity {
    /// Remaining running slots
    pub running: usize,

    /// Remaining idle slots (before swap)
    pub idle: usize,

    /// Remaining total slots
    pub total: usize,
}

impl RemainingCapacity {
    /// Check if any capacity remains
    pub fn has_capacity(&self) -> bool {
        self.running > 0 && self.total > 0
    }
}

impl std::fmt::Debug for QuotaManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuotaManager")
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_config_default() {
        let config = QuotaConfig::default();
        assert_eq!(config.max_running, 5);
        assert_eq!(config.max_idle, 10);
        assert_eq!(config.max_depth, 3);
        assert_eq!(config.token_budget, 100_000);
    }

    #[test]
    fn test_quota_violation_display() {
        let violation = QuotaViolation::MaxRunningExceeded {
            current: 6,
            limit: 5,
        };
        assert!(violation.to_string().contains("6"));
        assert!(violation.to_string().contains("5"));
    }

    #[test]
    fn test_check_tokens() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_quota_{}.db", uuid::Uuid::new_v4()));
        let db = Arc::new(StateDatabase::new(db_path).unwrap());

        let manager = QuotaManager::new(db);

        assert!(manager.check_tokens(50_000).is_ok());
        assert!(manager.check_tokens(100_000).is_ok());
        assert!(manager.check_tokens(100_001).is_err());
    }

    #[test]
    fn test_remaining_capacity() {
        let capacity = RemainingCapacity {
            running: 3,
            idle: 5,
            total: 10,
        };

        assert!(capacity.has_capacity());

        let no_running = RemainingCapacity {
            running: 0,
            idle: 5,
            total: 10,
        };
        assert!(!no_running.has_capacity());
    }
}
