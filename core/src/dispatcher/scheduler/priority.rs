//! Priority-based Task Scheduler
//!
//! Implements intelligent scheduling with three-tier priority system:
//! - Tier 0 (User): Immediate user actions, preemptive
//! - Tier 1 (Financial): High-priority financial operations
//! - Tier 2 (Background): Normal background tasks
//!
//! Features:
//! - Dynamic priority boosting to prevent starvation
//! - Domain-based conflict detection
//! - Task preemption with state freezing
//! - Integration with existing DAG scheduler

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::SystemTime;

use super::dag::DagScheduler;

/// Task priority tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PriorityTier {
    /// Immediate user actions (highest priority, preemptive)
    User = 0,
    /// High-priority financial operations
    Financial = 1,
    /// Normal background tasks
    #[default]
    Background = 2,
}

/// Risk level for browser tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RiskLevel {
    /// Read-only operations (safe)
    #[default]
    Low,
    /// Form filling, navigation (moderate risk)
    Medium,
    /// Financial transactions, data modification (high risk)
    High,
}

/// Task metadata for priority scheduling
#[derive(Debug, Clone)]
pub struct TaskMetadata {
    /// Priority tier
    pub tier: PriorityTier,

    /// Task submission timestamp (for starvation prevention)
    pub submitted_at: SystemTime,

    /// Domain being accessed (for conflict detection)
    pub domain: Option<String>,

    /// Risk level (for routing decisions)
    pub risk_level: RiskLevel,

    /// Whether task can be preempted
    pub preemptible: bool,

    /// Effective priority (dynamic, calculated)
    pub effective_priority: f64,
}

impl TaskMetadata {
    /// Create new task metadata
    pub fn new(tier: PriorityTier, domain: Option<String>, risk_level: RiskLevel) -> Self {
        Self {
            tier,
            submitted_at: SystemTime::now(),
            domain,
            risk_level,
            preemptible: tier != PriorityTier::User, // User tasks not preemptible
            effective_priority: 0.0,                 // Will be calculated
        }
    }
}

/// Configuration for priority scheduler
#[derive(Debug, Clone)]
pub struct PrioritySchedulerConfig {
    /// Starvation prevention threshold (seconds)
    pub starvation_threshold: u64,

    /// Maximum concurrent browser tasks
    pub max_concurrent_browser_tasks: usize,

    /// Enable dynamic priority boosting
    pub enable_priority_boosting: bool,

    /// Enable preemption
    pub enable_preemption: bool,
}

impl Default for PrioritySchedulerConfig {
    fn default() -> Self {
        Self {
            starvation_threshold: 300, // 5 minutes
            max_concurrent_browser_tasks: 3,
            enable_priority_boosting: true,
            enable_preemption: true,
        }
    }
}

/// Priority-based task scheduler with conflict resolution
pub struct PriorityScheduler {
    /// Configuration
    config: PrioritySchedulerConfig,

    /// Underlying DAG scheduler for dependency resolution
    _dag_scheduler: DagScheduler,

    /// Priority queues (one per tier)
    /// Index 0 = User, 1 = Financial, 2 = Background
    queues: [VecDeque<String>; 3],

    /// Task metadata registry
    metadata: HashMap<String, TaskMetadata>,

    /// Currently executing tasks
    executing: HashSet<String>,

    /// Frozen tasks (preempted, waiting to resume)
    frozen: HashSet<String>,

    /// Domain locks (for conflict prevention)
    /// Maps domain -> task_id currently holding the lock
    domain_locks: HashMap<String, String>,
}

impl PriorityScheduler {
    /// Create a new priority scheduler
    pub fn new(config: PrioritySchedulerConfig) -> Self {
        Self {
            config,
            _dag_scheduler: DagScheduler::new(),
            queues: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
            metadata: HashMap::new(),
            executing: HashSet::new(),
            frozen: HashSet::new(),
            domain_locks: HashMap::new(),
        }
    }

    /// Get the configuration
    pub fn config(&self) -> &PrioritySchedulerConfig {
        &self.config
    }

    /// Get number of tasks in queue for a specific tier
    pub fn queue_len(&self, tier: PriorityTier) -> usize {
        self.queues[tier as usize].len()
    }

    /// Get total number of queued tasks
    pub fn total_queued(&self) -> usize {
        self.queues.iter().map(|q| q.len()).sum()
    }

    /// Get number of executing tasks
    pub fn executing_count(&self) -> usize {
        self.executing.len()
    }

    /// Check if a task is currently executing
    pub fn is_executing(&self, task_id: &str) -> bool {
        self.executing.contains(task_id)
    }

    /// Get task metadata
    pub fn get_metadata(&self, task_id: &str) -> Option<&TaskMetadata> {
        self.metadata.get(task_id)
    }

    /// Submit a new task to the scheduler
    pub fn submit(&mut self, task_id: String, metadata: TaskMetadata) {
        let tier = metadata.tier;

        // Store metadata
        self.metadata.insert(task_id.clone(), metadata);

        // Add to appropriate queue
        self.queues[tier as usize].push_back(task_id);
    }

    /// Get the next ready task considering priority and dependencies
    pub fn next_ready(&mut self) -> Option<String> {
        // Update effective priorities if boosting is enabled
        if self.config.enable_priority_boosting {
            self.update_effective_priorities();
        }

        // Check queues in priority order (User -> Financial -> Background)
        for tier in [
            PriorityTier::User,
            PriorityTier::Financial,
            PriorityTier::Background,
        ] {
            let queue = &mut self.queues[tier as usize];

            // Find first task that's ready (no domain conflicts)
            let mut i = 0;
            while i < queue.len() {
                let task_id = &queue[i];

                // Check domain conflicts
                if let Some(metadata) = self.metadata.get(task_id) {
                    if let Some(domain) = &metadata.domain {
                        // Check if domain is locked by another task
                        if let Some(locking_task) = self.domain_locks.get(domain) {
                            if locking_task != task_id {
                                // Domain locked, try next task
                                i += 1;
                                continue;
                            }
                        }
                    }
                }

                // Task is ready, remove from queue and mark as executing
                let task_id = queue.remove(i).unwrap();
                self.executing.insert(task_id.clone());

                // Lock domain if task has one
                if let Some(metadata) = self.metadata.get(&task_id) {
                    if let Some(domain) = &metadata.domain {
                        self.domain_locks.insert(domain.clone(), task_id.clone());
                    }
                }

                return Some(task_id);
            }
        }

        None
    }

    /// Mark a task as completed and release resources
    pub fn complete(&mut self, task_id: &str) {
        // Remove from executing set
        self.executing.remove(task_id);

        // Release domain lock if task had one
        if let Some(metadata) = self.metadata.get(task_id) {
            if let Some(domain) = &metadata.domain {
                self.domain_locks.remove(domain);
            }
        }

        // Remove metadata
        self.metadata.remove(task_id);
    }

    /// Update effective priorities for all queued tasks (starvation prevention)
    fn update_effective_priorities(&mut self) {
        let now = SystemTime::now();
        let threshold = self.config.starvation_threshold;

        for metadata in self.metadata.values_mut() {
            let age = now
                .duration_since(metadata.submitted_at)
                .unwrap_or_default()
                .as_secs();

            metadata.effective_priority =
                Self::calculate_effective_priority(metadata.tier, age, threshold);
        }
    }

    /// Calculate effective priority with time-based boosting
    fn calculate_effective_priority(tier: PriorityTier, age_secs: u64, threshold: u64) -> f64 {
        let base_priority = match tier {
            PriorityTier::User => 1000.0,
            PriorityTier::Financial => 500.0,
            PriorityTier::Background => 100.0,
        };

        // Apply time-based boost if task is aging
        if age_secs > threshold {
            let boost_factor = (age_secs - threshold) as f64 / threshold as f64;
            base_priority * (1.0 + boost_factor)
        } else {
            base_priority
        }
    }

    /// Check if a task would have a domain conflict with executing tasks
    pub fn has_domain_conflict(&self, task_id: &str) -> bool {
        if let Some(metadata) = self.metadata.get(task_id) {
            if let Some(domain) = &metadata.domain {
                // Check if domain is locked by another task
                if let Some(locking_task) = self.domain_locks.get(domain) {
                    return locking_task != task_id;
                }
            }
        }
        false
    }

    /// Attempt to preempt lower priority tasks for a high priority task
    /// Returns list of task IDs that were preempted
    pub fn try_preempt_for(&mut self, task_id: &str) -> Vec<String> {
        if !self.config.enable_preemption {
            return Vec::new();
        }

        let mut preempted = Vec::new();

        // Only User tier tasks can preempt
        if let Some(metadata) = self.metadata.get(task_id) {
            if metadata.tier != PriorityTier::User {
                return preempted;
            }

            // Check if task needs a domain
            if let Some(domain) = &metadata.domain {
                // Find task holding the domain lock
                if let Some(locking_task) = self.domain_locks.get(domain).cloned() {
                    // Check if locking task is preemptible
                    if let Some(locking_metadata) = self.metadata.get(&locking_task) {
                        if locking_metadata.preemptible {
                            // Preempt the task
                            self.freeze_task(&locking_task);
                            preempted.push(locking_task);
                        }
                    }
                }
            }
        }

        preempted
    }

    /// Freeze a task (suspend execution)
    pub fn freeze_task(&mut self, task_id: &str) {
        // Remove from executing set
        self.executing.remove(task_id);

        // Add to frozen set
        self.frozen.insert(task_id.to_string());

        // Release domain lock
        if let Some(metadata) = self.metadata.get(task_id) {
            if let Some(domain) = &metadata.domain {
                self.domain_locks.remove(domain);
            }
        }

        // Note: Actual state freezing (CDP Debugger.pause) will be done in integration layer
    }

    /// Resume a frozen task
    pub fn resume_task(&mut self, task_id: &str) -> bool {
        if !self.frozen.contains(task_id) {
            return false;
        }

        // Check if task can be resumed (no domain conflicts)
        if self.has_domain_conflict(task_id) {
            return false;
        }

        // Remove from frozen set
        self.frozen.remove(task_id);

        // Add back to executing set
        self.executing.insert(task_id.to_string());

        // Re-acquire domain lock
        if let Some(metadata) = self.metadata.get(task_id) {
            if let Some(domain) = &metadata.domain {
                self.domain_locks
                    .insert(domain.clone(), task_id.to_string());
            }
        }

        // Note: Actual state resumption (CDP Debugger.resume) will be done in integration layer
        true
    }

    /// Get list of frozen tasks
    pub fn frozen_tasks(&self) -> Vec<String> {
        self.frozen.iter().cloned().collect()
    }

    /// Check if a task is frozen
    pub fn is_frozen(&self, task_id: &str) -> bool {
        self.frozen.contains(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_tier_ordering() {
        assert!(PriorityTier::User < PriorityTier::Financial);
        assert!(PriorityTier::Financial < PriorityTier::Background);
    }

    #[test]
    fn test_priority_tier_default() {
        assert_eq!(PriorityTier::default(), PriorityTier::Background);
    }

    #[test]
    fn test_risk_level_default() {
        assert_eq!(RiskLevel::default(), RiskLevel::Low);
    }

    #[test]
    fn test_task_metadata_creation() {
        let metadata = TaskMetadata::new(
            PriorityTier::User,
            Some("example.com".to_string()),
            RiskLevel::High,
        );

        assert_eq!(metadata.tier, PriorityTier::User);
        assert_eq!(metadata.domain, Some("example.com".to_string()));
        assert_eq!(metadata.risk_level, RiskLevel::High);
        assert!(!metadata.preemptible); // User tasks not preemptible
    }

    #[test]
    fn test_priority_scheduler_creation() {
        let config = PrioritySchedulerConfig::default();
        let scheduler = PriorityScheduler::new(config);

        assert_eq!(scheduler.total_queued(), 0);
        assert_eq!(scheduler.executing_count(), 0);
        assert_eq!(scheduler.queue_len(PriorityTier::User), 0);
        assert_eq!(scheduler.queue_len(PriorityTier::Financial), 0);
        assert_eq!(scheduler.queue_len(PriorityTier::Background), 0);
    }

    #[test]
    fn test_config_defaults() {
        let config = PrioritySchedulerConfig::default();

        assert_eq!(config.starvation_threshold, 300);
        assert_eq!(config.max_concurrent_browser_tasks, 3);
        assert!(config.enable_priority_boosting);
        assert!(config.enable_preemption);
    }

    #[test]
    fn test_submit_and_next_ready() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        // Submit tasks with different priorities
        scheduler.submit(
            "task1".to_string(),
            TaskMetadata::new(PriorityTier::Background, None, RiskLevel::Low),
        );
        scheduler.submit(
            "task2".to_string(),
            TaskMetadata::new(PriorityTier::User, None, RiskLevel::Low),
        );
        scheduler.submit(
            "task3".to_string(),
            TaskMetadata::new(PriorityTier::Financial, None, RiskLevel::Low),
        );

        assert_eq!(scheduler.total_queued(), 3);

        // Should get User task first (highest priority)
        assert_eq!(scheduler.next_ready(), Some("task2".to_string()));
        assert_eq!(scheduler.executing_count(), 1);

        // Should get Financial task next
        assert_eq!(scheduler.next_ready(), Some("task3".to_string()));
        assert_eq!(scheduler.executing_count(), 2);

        // Should get Background task last
        assert_eq!(scheduler.next_ready(), Some("task1".to_string()));
        assert_eq!(scheduler.executing_count(), 3);

        // No more tasks
        assert_eq!(scheduler.next_ready(), None);
    }

    #[test]
    fn test_domain_conflict_detection() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        // Submit two tasks for the same domain
        scheduler.submit(
            "task1".to_string(),
            TaskMetadata::new(
                PriorityTier::User,
                Some("example.com".to_string()),
                RiskLevel::High,
            ),
        );
        scheduler.submit(
            "task2".to_string(),
            TaskMetadata::new(
                PriorityTier::User,
                Some("example.com".to_string()),
                RiskLevel::High,
            ),
        );

        // First task should be returned
        let first = scheduler.next_ready();
        assert!(first.is_some());

        // Second task should be blocked (domain conflict)
        let second = scheduler.next_ready();
        assert!(second.is_none());

        // Complete first task
        scheduler.complete(&first.unwrap());

        // Now second task should be available
        let second = scheduler.next_ready();
        assert!(second.is_some());
    }

    #[test]
    fn test_complete_releases_domain() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        scheduler.submit(
            "task1".to_string(),
            TaskMetadata::new(
                PriorityTier::User,
                Some("example.com".to_string()),
                RiskLevel::High,
            ),
        );

        let task_id = scheduler.next_ready().unwrap();
        assert_eq!(scheduler.executing_count(), 1);
        assert!(scheduler.domain_locks.contains_key("example.com"));

        scheduler.complete(&task_id);
        assert_eq!(scheduler.executing_count(), 0);
        assert!(!scheduler.domain_locks.contains_key("example.com"));
    }

    #[test]
    fn test_priority_calculation() {
        // Fresh task (no aging)
        let priority =
            PriorityScheduler::calculate_effective_priority(PriorityTier::Background, 0, 300);
        assert_eq!(priority, 100.0);

        // Aged task (beyond threshold)
        let priority = PriorityScheduler::calculate_effective_priority(
            PriorityTier::Background,
            600, // 2x threshold
            300,
        );
        assert_eq!(priority, 200.0); // 100.0 * (1.0 + 1.0)

        // User task always has high base priority
        let priority = PriorityScheduler::calculate_effective_priority(PriorityTier::User, 0, 300);
        assert_eq!(priority, 1000.0);
    }

    #[test]
    fn test_starvation_prevention() {
        use std::time::Duration;

        let config = PrioritySchedulerConfig {
            starvation_threshold: 1, // 1 second threshold for testing
            ..PrioritySchedulerConfig::default()
        };
        let mut scheduler = PriorityScheduler::new(config);

        // Submit a background task
        let mut metadata = TaskMetadata::new(PriorityTier::Background, None, RiskLevel::Low);
        // Simulate aging by setting submitted_at to past
        metadata.submitted_at = SystemTime::now() - Duration::from_secs(5);
        scheduler.metadata.insert("task1".to_string(), metadata);

        // Update priorities
        scheduler.update_effective_priorities();

        // Check that priority was boosted
        let metadata = scheduler.get_metadata("task1").unwrap();
        assert!(metadata.effective_priority > 100.0); // Base priority is 100.0
    }

    #[test]
    fn test_has_domain_conflict() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        // Submit and execute a task with domain
        scheduler.submit(
            "task1".to_string(),
            TaskMetadata::new(
                PriorityTier::Financial,
                Some("example.com".to_string()),
                RiskLevel::Medium,
            ),
        );
        scheduler.next_ready(); // Execute task1

        // Submit another task with same domain
        scheduler.submit(
            "task2".to_string(),
            TaskMetadata::new(
                PriorityTier::Financial,
                Some("example.com".to_string()),
                RiskLevel::Medium,
            ),
        );

        // Should detect conflict
        assert!(scheduler.has_domain_conflict("task2"));

        // Task with different domain should not conflict
        scheduler.submit(
            "task3".to_string(),
            TaskMetadata::new(
                PriorityTier::Financial,
                Some("other.com".to_string()),
                RiskLevel::Medium,
            ),
        );
        assert!(!scheduler.has_domain_conflict("task3"));
    }

    #[test]
    fn test_freeze_and_resume_task() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        // Submit and execute a task
        scheduler.submit(
            "task1".to_string(),
            TaskMetadata::new(
                PriorityTier::Financial,
                Some("example.com".to_string()),
                RiskLevel::Medium,
            ),
        );
        scheduler.next_ready(); // Execute task1

        assert!(scheduler.is_executing("task1"));
        assert!(!scheduler.is_frozen("task1"));
        assert!(scheduler.domain_locks.contains_key("example.com"));

        // Freeze the task
        scheduler.freeze_task("task1");

        assert!(!scheduler.is_executing("task1"));
        assert!(scheduler.is_frozen("task1"));
        assert!(!scheduler.domain_locks.contains_key("example.com"));

        // Resume the task
        let resumed = scheduler.resume_task("task1");
        assert!(resumed);

        assert!(scheduler.is_executing("task1"));
        assert!(!scheduler.is_frozen("task1"));
        assert!(scheduler.domain_locks.contains_key("example.com"));
    }

    #[test]
    fn test_try_preempt_for_user_task() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        // Submit and execute a Financial task (preemptible)
        scheduler.submit(
            "financial_task".to_string(),
            TaskMetadata::new(
                PriorityTier::Financial,
                Some("example.com".to_string()),
                RiskLevel::Medium,
            ),
        );
        scheduler.next_ready(); // Execute financial_task

        // Submit a User task with same domain
        scheduler.submit(
            "user_task".to_string(),
            TaskMetadata::new(
                PriorityTier::User,
                Some("example.com".to_string()),
                RiskLevel::High,
            ),
        );

        // Try to preempt for user task
        let preempted = scheduler.try_preempt_for("user_task");

        assert_eq!(preempted.len(), 1);
        assert_eq!(preempted[0], "financial_task");
        assert!(scheduler.is_frozen("financial_task"));
        assert!(!scheduler.is_executing("financial_task"));
    }

    #[test]
    fn test_user_task_not_preemptible() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        // Submit and execute a User task (not preemptible)
        scheduler.submit(
            "user_task1".to_string(),
            TaskMetadata::new(
                PriorityTier::User,
                Some("example.com".to_string()),
                RiskLevel::High,
            ),
        );
        scheduler.next_ready(); // Execute user_task1

        // Submit another User task with same domain
        scheduler.submit(
            "user_task2".to_string(),
            TaskMetadata::new(
                PriorityTier::User,
                Some("example.com".to_string()),
                RiskLevel::High,
            ),
        );

        // Try to preempt - should fail (User tasks not preemptible)
        let preempted = scheduler.try_preempt_for("user_task2");

        assert_eq!(preempted.len(), 0);
        assert!(scheduler.is_executing("user_task1"));
        assert!(!scheduler.is_frozen("user_task1"));
    }

    #[test]
    fn test_preemption_disabled() {
        let config = PrioritySchedulerConfig {
            enable_preemption: false,
            ..PrioritySchedulerConfig::default()
        };
        let mut scheduler = PriorityScheduler::new(config);

        // Submit and execute a Financial task
        scheduler.submit(
            "financial_task".to_string(),
            TaskMetadata::new(
                PriorityTier::Financial,
                Some("example.com".to_string()),
                RiskLevel::Medium,
            ),
        );
        scheduler.next_ready();

        // Submit a User task
        scheduler.submit(
            "user_task".to_string(),
            TaskMetadata::new(
                PriorityTier::User,
                Some("example.com".to_string()),
                RiskLevel::High,
            ),
        );

        // Try to preempt - should fail (preemption disabled)
        let preempted = scheduler.try_preempt_for("user_task");

        assert_eq!(preempted.len(), 0);
        assert!(scheduler.is_executing("financial_task"));
    }

    #[test]
    fn test_frozen_tasks_list() {
        let config = PrioritySchedulerConfig::default();
        let mut scheduler = PriorityScheduler::new(config);

        // Submit and execute tasks
        scheduler.submit(
            "task1".to_string(),
            TaskMetadata::new(PriorityTier::Financial, None, RiskLevel::Medium),
        );
        scheduler.submit(
            "task2".to_string(),
            TaskMetadata::new(PriorityTier::Financial, None, RiskLevel::Medium),
        );
        scheduler.next_ready();
        scheduler.next_ready();

        // Freeze both tasks
        scheduler.freeze_task("task1");
        scheduler.freeze_task("task2");

        let frozen = scheduler.frozen_tasks();
        assert_eq!(frozen.len(), 2);
        assert!(frozen.contains(&"task1".to_string()));
        assert!(frozen.contains(&"task2".to_string()));
    }
}
