//! Resource Governor
//!
//! Implements Lane-based priority isolation and resource governance
//! for the Multi-Agent Resilience architecture.

use crate::error::AlephError;
use super::super::types::Lane;
use crate::resilience::database::StateDatabase;
use std::collections::HashMap;
use crate::sync_primitives::{AtomicU64, Ordering};
use crate::sync_primitives::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, warn};

/// Governor configuration
#[derive(Debug, Clone)]
pub struct GovernorConfig {
    /// Maximum concurrent running subagents
    pub max_running_subagents: usize,

    /// Maximum idle subagents in memory (before swap)
    pub max_idle_in_memory: usize,

    /// Maximum recursion depth for task spawning
    pub max_recursion_depth: u32,

    /// Token budget per session
    pub token_budget_per_session: u64,

    /// Reserved capacity for main lane (percentage 0-100)
    pub main_lane_reserve_percent: u8,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            max_running_subagents: 5,
            max_idle_in_memory: 10,
            max_recursion_depth: 3,
            token_budget_per_session: 100_000,
            main_lane_reserve_percent: 20,
        }
    }
}

/// Lane-specific resource allocation
#[derive(Debug)]
struct LaneResources {
    /// Semaphore for concurrency limiting
    semaphore: Semaphore,

    /// Current active count
    active_count: AtomicU64,

    /// Maximum capacity
    max_capacity: usize,
}

impl LaneResources {
    fn new(max_capacity: usize) -> Self {
        Self {
            semaphore: Semaphore::new(max_capacity),
            active_count: AtomicU64::new(0),
            max_capacity,
        }
    }

    fn active(&self) -> u64 {
        self.active_count.load(Ordering::SeqCst)
    }

    fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// Resource Governor for multi-agent resource management
///
/// Implements:
/// - Lane-based priority isolation
/// - Concurrency limits per lane
/// - Recursion depth tracking
/// - Token budget enforcement
pub struct ResourceGovernor {
    db: Arc<StateDatabase>,
    config: GovernorConfig,

    /// Resources per lane
    main_lane: LaneResources,
    subagent_lane: LaneResources,

    /// Token usage per session
    session_tokens: RwLock<HashMap<String, AtomicU64>>,
}

impl ResourceGovernor {
    /// Create a new Resource Governor
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self::with_config(db, GovernorConfig::default())
    }

    /// Create a Resource Governor with custom config
    pub fn with_config(db: Arc<StateDatabase>, config: GovernorConfig) -> Self {
        // Calculate lane capacities based on reserve percentage
        let total_capacity = config.max_running_subagents + 2; // +2 for main lane minimum
        let main_capacity =
            (total_capacity * config.main_lane_reserve_percent as usize / 100).max(1);
        let subagent_capacity = config.max_running_subagents;

        info!(
            main_capacity = main_capacity,
            subagent_capacity = subagent_capacity,
            "ResourceGovernor initialized with lane capacities"
        );

        Self {
            db,
            config,
            main_lane: LaneResources::new(main_capacity),
            subagent_lane: LaneResources::new(subagent_capacity),
            session_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Acquire resources for a task in the specified lane
    ///
    /// Returns a permit that must be released when the task completes.
    pub async fn acquire(&self, lane: Lane) -> Result<ResourcePermit, AlephError> {
        let resources = match lane {
            Lane::Main => &self.main_lane,
            Lane::Subagent => &self.subagent_lane,
        };

        // Try to acquire permit
        let permit = resources
            .semaphore
            .acquire()
            .await
            .map_err(|_| AlephError::config("Resource acquisition cancelled".to_string()))?;

        // Increment active count
        resources.active_count.fetch_add(1, Ordering::SeqCst);

        debug!(
            lane = ?lane,
            active = resources.active(),
            available = resources.available(),
            "Resource acquired"
        );

        // Convert to owned permit
        permit.forget();

        Ok(ResourcePermit {
            lane,
            released: false,
        })
    }

    /// Try to acquire resources without blocking
    pub fn try_acquire(&self, lane: Lane) -> Option<ResourcePermit> {
        let resources = match lane {
            Lane::Main => &self.main_lane,
            Lane::Subagent => &self.subagent_lane,
        };

        match resources.semaphore.try_acquire() {
            Ok(permit) => {
                resources.active_count.fetch_add(1, Ordering::SeqCst);
                permit.forget();
                Some(ResourcePermit {
                    lane,
                    released: false,
                })
            }
            Err(_) => None,
        }
    }

    /// Release resources for a task
    pub fn release(&self, permit: ResourcePermit) {
        if permit.released {
            return;
        }

        let resources = match permit.lane {
            Lane::Main => &self.main_lane,
            Lane::Subagent => &self.subagent_lane,
        };

        resources.active_count.fetch_sub(1, Ordering::SeqCst);
        resources.semaphore.add_permits(1);

        debug!(
            lane = ?permit.lane,
            active = resources.active(),
            available = resources.available(),
            "Resource released"
        );
    }

    /// Check if a lane has available capacity
    pub fn has_capacity(&self, lane: Lane) -> bool {
        let resources = match lane {
            Lane::Main => &self.main_lane,
            Lane::Subagent => &self.subagent_lane,
        };

        resources.available() > 0
    }

    /// Get current resource statistics
    pub fn get_stats(&self) -> GovernorStats {
        GovernorStats {
            main_lane_active: self.main_lane.active(),
            main_lane_capacity: self.main_lane.max_capacity,
            subagent_lane_active: self.subagent_lane.active(),
            subagent_lane_capacity: self.subagent_lane.max_capacity,
        }
    }

    /// Track token usage for a session
    pub async fn record_tokens(&self, session_id: &str, tokens: u64) -> Result<bool, AlephError> {
        let mut session_tokens = self.session_tokens.write().await;

        let counter = session_tokens
            .entry(session_id.to_string())
            .or_insert_with(|| AtomicU64::new(0));

        let new_total = counter.fetch_add(tokens, Ordering::SeqCst) + tokens;

        // Check if budget exceeded
        if new_total > self.config.token_budget_per_session {
            warn!(
                session_id = %session_id,
                tokens_used = new_total,
                budget = self.config.token_budget_per_session,
                "Token budget exceeded"
            );
            return Ok(false);
        }

        Ok(true)
    }

    /// Get token usage for a session
    pub async fn get_token_usage(&self, session_id: &str) -> u64 {
        let session_tokens = self.session_tokens.read().await;
        session_tokens
            .get(session_id)
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    /// Reset token tracking for a session
    pub async fn reset_session_tokens(&self, session_id: &str) {
        let mut session_tokens = self.session_tokens.write().await;
        session_tokens.remove(session_id);
    }

    /// Get the configuration
    pub fn config(&self) -> &GovernorConfig {
        &self.config
    }

    /// Get database reference
    pub fn database(&self) -> &Arc<StateDatabase> {
        &self.db
    }
}

/// Resource permit that must be released
#[derive(Debug)]
pub struct ResourcePermit {
    lane: Lane,
    released: bool,
}

impl ResourcePermit {
    /// Mark the permit as released
    pub fn mark_released(&mut self) {
        self.released = true;
    }

    /// Get the lane this permit is for
    pub fn lane(&self) -> Lane {
        self.lane
    }
}

/// Governor statistics
#[derive(Debug, Clone)]
pub struct GovernorStats {
    pub main_lane_active: u64,
    pub main_lane_capacity: usize,
    pub subagent_lane_active: u64,
    pub subagent_lane_capacity: usize,
}

impl GovernorStats {
    /// Total active across all lanes
    pub fn total_active(&self) -> u64 {
        self.main_lane_active + self.subagent_lane_active
    }

    /// Main lane utilization (0.0 to 1.0)
    pub fn main_lane_utilization(&self) -> f64 {
        if self.main_lane_capacity == 0 {
            0.0
        } else {
            self.main_lane_active as f64 / self.main_lane_capacity as f64
        }
    }

    /// Subagent lane utilization (0.0 to 1.0)
    pub fn subagent_lane_utilization(&self) -> f64 {
        if self.subagent_lane_capacity == 0 {
            0.0
        } else {
            self.subagent_lane_active as f64 / self.subagent_lane_capacity as f64
        }
    }
}

impl std::fmt::Debug for ResourceGovernor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceGovernor")
            .field("config", &self.config)
            .field("stats", &self.get_stats())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_governor_config_default() {
        let config = GovernorConfig::default();
        assert_eq!(config.max_running_subagents, 5);
        assert_eq!(config.max_recursion_depth, 3);
        assert_eq!(config.token_budget_per_session, 100_000);
    }

    #[test]
    fn test_governor_stats() {
        let stats = GovernorStats {
            main_lane_active: 1,
            main_lane_capacity: 2,
            subagent_lane_active: 3,
            subagent_lane_capacity: 5,
        };

        assert_eq!(stats.total_active(), 4);
        assert!((stats.main_lane_utilization() - 0.5).abs() < 0.001);
        assert!((stats.subagent_lane_utilization() - 0.6).abs() < 0.001);
    }

    #[test]
    fn test_resource_permit() {
        let mut permit = ResourcePermit {
            lane: Lane::Main,
            released: false,
        };
        assert_eq!(permit.lane(), Lane::Main);
        assert!(!permit.released);

        permit.mark_released();
        assert!(permit.released);
    }
}
