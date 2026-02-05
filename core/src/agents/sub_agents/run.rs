//! Sub-Agent Run Data Model
//!
//! This module defines the data structures for tracking sub-agent execution
//! lifecycle as part of the Multi-Agent 2.0 system.
//!
//! # Overview
//!
//! The `SubAgentRun` struct represents a single execution of a sub-agent,
//! tracking its status, timing, outcome, and configuration.
//!
//! # State Machine
//!
//! ```text
//! ┌─────────┐     ┌─────────┐     ┌───────────┐
//! │ Pending │────▶│ Running │────▶│ Completed │
//! └────┬────┘     └────┬────┘     └───────────┘
//!      │               │
//!      │               ├─────────▶ Failed
//!      │               │
//!      │               ├─────────▶ Paused ────▶ Running
//!      │               │                            │
//!      └───────────────┴─────────▶ Cancelled ◀─────┘
//! ```

use serde::{Deserialize, Serialize};

use crate::routing::SessionKey;

/// Status of a sub-agent run
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is queued but not yet started
    Pending,
    /// Run is actively executing
    Running,
    /// Run is temporarily paused
    Paused,
    /// Run completed successfully
    Completed,
    /// Run failed with an error
    Failed,
    /// Run was cancelled by user or system
    Cancelled,
}

impl RunStatus {
    /// Check if this status can transition to the target status
    ///
    /// Valid transitions:
    /// - Pending -> Running, Cancelled
    /// - Running -> Completed, Failed, Paused, Cancelled
    /// - Paused -> Running, Cancelled
    pub fn can_transition_to(&self, target: &RunStatus) -> bool {
        match (self, target) {
            // Pending can go to Running or Cancelled
            (RunStatus::Pending, RunStatus::Running) => true,
            (RunStatus::Pending, RunStatus::Cancelled) => true,

            // Running can go to Completed, Failed, Paused, or Cancelled
            (RunStatus::Running, RunStatus::Completed) => true,
            (RunStatus::Running, RunStatus::Failed) => true,
            (RunStatus::Running, RunStatus::Paused) => true,
            (RunStatus::Running, RunStatus::Cancelled) => true,

            // Paused can resume (Running) or be Cancelled
            (RunStatus::Paused, RunStatus::Running) => true,
            (RunStatus::Paused, RunStatus::Cancelled) => true,

            // Terminal states cannot transition
            (RunStatus::Completed, _) => false,
            (RunStatus::Failed, _) => false,
            (RunStatus::Cancelled, _) => false,

            // All other transitions are invalid
            _ => false,
        }
    }

    /// Check if this status is a terminal state
    ///
    /// Terminal states are: Completed, Failed, Cancelled
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
        )
    }
}

/// Execution lane for sub-agent runs
///
/// Lanes provide isolation and resource management for different types of runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// Main agent lane (user-initiated)
    Main,
    /// Sub-agent lane (delegated tasks)
    #[default]
    Subagent,
    /// Cron job lane (scheduled tasks)
    Cron,
    /// Nested sub-agent lane (sub-agents spawning sub-agents)
    Nested,
}

impl Lane {
    /// Get the default maximum concurrent runs for this lane
    pub fn default_max_concurrent(&self) -> usize {
        match self {
            Lane::Main => 2,
            Lane::Subagent => 8,
            Lane::Cron => 2,
            Lane::Nested => 4,
        }
    }

    /// Get the default priority for this lane
    ///
    /// Higher values indicate higher priority.
    pub fn default_priority(&self) -> i8 {
        match self {
            Lane::Main => 10,
            Lane::Nested => 8,
            Lane::Subagent => 5,
            Lane::Cron => 0,
        }
    }
}

/// Cleanup policy for completed runs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    /// Delete immediately after completion
    Delete,
    /// Keep for 1 hour (default)
    #[default]
    Keep,
    /// Archive for 7 days
    Archive,
}

/// Outcome of a completed sub-agent run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    /// Summary of what was accomplished
    pub summary: String,
    /// Structured output data (if any)
    pub output: Option<serde_json::Value>,
    /// Number of artifacts produced
    pub artifacts_count: usize,
    /// Number of tools called during execution
    pub tools_called: usize,
    /// Total execution duration in milliseconds
    pub duration_ms: u64,
}

impl RunOutcome {
    /// Create a new run outcome
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            output: None,
            artifacts_count: 0,
            tools_called: 0,
            duration_ms: 0,
        }
    }

    /// Set the output data
    pub fn with_output(mut self, output: serde_json::Value) -> Self {
        self.output = Some(output);
        self
    }

    /// Set the artifacts count
    pub fn with_artifacts_count(mut self, count: usize) -> Self {
        self.artifacts_count = count;
        self
    }

    /// Set the tools called count
    pub fn with_tools_called(mut self, count: usize) -> Self {
        self.tools_called = count;
        self
    }

    /// Set the duration in milliseconds
    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }
}

/// A sub-agent run represents a single execution of a sub-agent
///
/// This struct tracks the full lifecycle of a sub-agent execution,
/// from creation through completion or failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRun {
    /// Unique identifier for this run
    pub run_id: String,
    /// Session key for this sub-agent's session
    pub session_key: SessionKey,
    /// Session key of the parent agent that spawned this run
    pub parent_session_key: SessionKey,
    /// The task/prompt assigned to this sub-agent
    pub task: String,
    /// Type of agent (e.g., "mcp", "skill", "coding")
    pub agent_type: String,
    /// Optional human-readable label for this run
    pub label: Option<String>,
    /// Timestamp when the run was created (milliseconds since epoch)
    pub created_at: i64,
    /// Timestamp when the run started executing
    pub started_at: Option<i64>,
    /// Timestamp when the run ended (completed, failed, or cancelled)
    pub ended_at: Option<i64>,
    /// Timestamp when the run was archived
    pub archived_at: Option<i64>,
    /// Current status of the run
    pub status: RunStatus,
    /// Outcome of the run (populated on completion)
    pub outcome: Option<RunOutcome>,
    /// Error message if the run failed
    pub error: Option<String>,
    /// Execution lane for resource management
    pub lane: Lane,
    /// Priority within the lane (higher = more important)
    pub priority: u8,
    /// Maximum number of turns/iterations allowed
    pub max_turns: Option<u32>,
    /// Timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Checkpoint ID for resumable runs
    pub checkpoint_id: Option<String>,
    /// Number of retry attempts
    pub retry_count: u32,
    /// Cleanup policy after completion
    pub cleanup_policy: CleanupPolicy,
}

impl SubAgentRun {
    /// Create a new sub-agent run
    ///
    /// # Arguments
    ///
    /// * `session_key` - Session key for this sub-agent
    /// * `parent_session_key` - Session key of the parent agent
    /// * `task` - The task/prompt for the sub-agent
    /// * `agent_type` - Type of agent (e.g., "mcp", "skill")
    pub fn new(
        session_key: SessionKey,
        parent_session_key: SessionKey,
        task: impl Into<String>,
        agent_type: impl Into<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Self {
            run_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            parent_session_key,
            task: task.into(),
            agent_type: agent_type.into(),
            label: None,
            created_at: now,
            started_at: None,
            ended_at: None,
            archived_at: None,
            status: RunStatus::Pending,
            outcome: None,
            error: None,
            lane: Lane::default(),
            priority: Lane::default().default_priority() as u8,
            max_turns: None,
            timeout_ms: None,
            checkpoint_id: None,
            retry_count: 0,
            cleanup_policy: CleanupPolicy::default(),
        }
    }

    /// Set the execution lane
    pub fn with_lane(mut self, lane: Lane) -> Self {
        self.lane = lane;
        self.priority = lane.default_priority() as u8;
        self
    }

    /// Set a human-readable label
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the timeout in milliseconds
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Set the maximum number of turns/iterations
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    /// Set the priority
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Set the cleanup policy
    pub fn with_cleanup_policy(mut self, policy: CleanupPolicy) -> Self {
        self.cleanup_policy = policy;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_status_transitions() {
        // Valid transitions from Pending
        assert!(RunStatus::Pending.can_transition_to(&RunStatus::Running));
        assert!(RunStatus::Pending.can_transition_to(&RunStatus::Cancelled));
        assert!(!RunStatus::Pending.can_transition_to(&RunStatus::Completed));
        assert!(!RunStatus::Pending.can_transition_to(&RunStatus::Failed));
        assert!(!RunStatus::Pending.can_transition_to(&RunStatus::Paused));

        // Valid transitions from Running
        assert!(RunStatus::Running.can_transition_to(&RunStatus::Completed));
        assert!(RunStatus::Running.can_transition_to(&RunStatus::Failed));
        assert!(RunStatus::Running.can_transition_to(&RunStatus::Paused));
        assert!(RunStatus::Running.can_transition_to(&RunStatus::Cancelled));
        assert!(!RunStatus::Running.can_transition_to(&RunStatus::Pending));

        // Valid transitions from Paused
        assert!(RunStatus::Paused.can_transition_to(&RunStatus::Running));
        assert!(RunStatus::Paused.can_transition_to(&RunStatus::Cancelled));
        assert!(!RunStatus::Paused.can_transition_to(&RunStatus::Completed));
        assert!(!RunStatus::Paused.can_transition_to(&RunStatus::Failed));
        assert!(!RunStatus::Paused.can_transition_to(&RunStatus::Pending));

        // Terminal states cannot transition
        assert!(!RunStatus::Completed.can_transition_to(&RunStatus::Running));
        assert!(!RunStatus::Completed.can_transition_to(&RunStatus::Pending));
        assert!(!RunStatus::Failed.can_transition_to(&RunStatus::Running));
        assert!(!RunStatus::Cancelled.can_transition_to(&RunStatus::Running));
    }

    #[test]
    fn test_run_status_is_terminal() {
        assert!(!RunStatus::Pending.is_terminal());
        assert!(!RunStatus::Running.is_terminal());
        assert!(!RunStatus::Paused.is_terminal());
        assert!(RunStatus::Completed.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
        assert!(RunStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_subagent_run_creation() {
        let parent_key = SessionKey::main("main");
        let session_key = SessionKey::Subagent {
            parent_key: Box::new(parent_key.clone()),
            subagent_id: "test-agent".to_string(),
        };

        let run = SubAgentRun::new(
            session_key.clone(),
            parent_key.clone(),
            "Test task",
            "mcp",
        );

        // Verify default values
        assert!(!run.run_id.is_empty());
        assert_eq!(run.task, "Test task");
        assert_eq!(run.agent_type, "mcp");
        assert_eq!(run.status, RunStatus::Pending);
        assert_eq!(run.lane, Lane::Subagent);
        assert_eq!(run.priority, Lane::Subagent.default_priority() as u8);
        assert_eq!(run.cleanup_policy, CleanupPolicy::Keep);
        assert!(run.created_at > 0);
        assert!(run.started_at.is_none());
        assert!(run.ended_at.is_none());
        assert!(run.outcome.is_none());
        assert!(run.error.is_none());
        assert_eq!(run.retry_count, 0);
    }

    #[test]
    fn test_subagent_run_builder() {
        let parent_key = SessionKey::main("main");
        let session_key = SessionKey::Subagent {
            parent_key: Box::new(parent_key.clone()),
            subagent_id: "builder-test".to_string(),
        };

        let run = SubAgentRun::new(session_key, parent_key, "Builder test", "skill")
            .with_lane(Lane::Main)
            .with_label("Test Label")
            .with_timeout(30000)
            .with_max_turns(10)
            .with_cleanup_policy(CleanupPolicy::Archive);

        assert_eq!(run.lane, Lane::Main);
        assert_eq!(run.priority, Lane::Main.default_priority() as u8);
        assert_eq!(run.label, Some("Test Label".to_string()));
        assert_eq!(run.timeout_ms, Some(30000));
        assert_eq!(run.max_turns, Some(10));
        assert_eq!(run.cleanup_policy, CleanupPolicy::Archive);
    }

    #[test]
    fn test_lane_default_quotas() {
        assert_eq!(Lane::Main.default_max_concurrent(), 2);
        assert_eq!(Lane::Subagent.default_max_concurrent(), 8);
        assert_eq!(Lane::Cron.default_max_concurrent(), 2);
        assert_eq!(Lane::Nested.default_max_concurrent(), 4);
    }

    #[test]
    fn test_lane_default_priorities() {
        assert_eq!(Lane::Main.default_priority(), 10);
        assert_eq!(Lane::Nested.default_priority(), 8);
        assert_eq!(Lane::Subagent.default_priority(), 5);
        assert_eq!(Lane::Cron.default_priority(), 0);
    }

    #[test]
    fn test_lane_default() {
        assert_eq!(Lane::default(), Lane::Subagent);
    }

    #[test]
    fn test_cleanup_policy_default() {
        assert_eq!(CleanupPolicy::default(), CleanupPolicy::Keep);
    }

    #[test]
    fn test_run_outcome_builder() {
        let outcome = RunOutcome::new("Task completed successfully")
            .with_output(serde_json::json!({"result": "success"}))
            .with_artifacts_count(3)
            .with_tools_called(5)
            .with_duration_ms(1500);

        assert_eq!(outcome.summary, "Task completed successfully");
        assert!(outcome.output.is_some());
        assert_eq!(outcome.artifacts_count, 3);
        assert_eq!(outcome.tools_called, 5);
        assert_eq!(outcome.duration_ms, 1500);
    }

    #[test]
    fn test_run_status_serialization() {
        let status = RunStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let deserialized: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, RunStatus::Running);
    }

    #[test]
    fn test_lane_serialization() {
        let lane = Lane::Subagent;
        let json = serde_json::to_string(&lane).unwrap();
        assert_eq!(json, "\"subagent\"");

        let deserialized: Lane = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, Lane::Subagent);
    }
}
