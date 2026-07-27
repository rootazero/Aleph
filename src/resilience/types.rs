//! Multi-Agent Resilience Types
//!
//! Core types for the Multi-Agent Resilience architecture:
//! - `AgentTask`: Task state and recovery checkpoints
//! - `TaskTrace`: Structured execution traces for Shadow Replay
//! - `AgentEvent`: Tiered event persistence (Skeleton & Pulse)

use aleph_protocol::AgentTraceEvent;
use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Enums
// =============================================================================

/// Task execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskStatus {
    /// Task is waiting to be executed
    Pending,
    /// Task is currently executing
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed with an error
    Failed,
    /// Task was interrupted (e.g., system restart)
    Interrupted,
    /// Task is paused in Idle state (Session-as-a-Service)
    Idle,
    /// Task context has been swapped out to disk
    Swapped,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Interrupted => write!(f, "interrupted"),
            Self::Idle => write!(f, "idle"),
            Self::Swapped => write!(f, "swapped"),
        }
    }
}

impl TaskStatus {
    /// Parse status from database string with fallback to Pending
    pub fn from_str_or_default(s: &str) -> Self {
        match s.parse() {
            Ok(status) => status,
            Err(e) => {
                tracing::warn!(input = %s, error = %e, "Unknown task status, defaulting to Pending");
                Self::Pending
            }
        }
    }

    /// Check if task can be auto-resumed on restart
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(self, Self::Running | Self::Interrupted)
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "idle" => Ok(Self::Idle),
            "swapped" => Ok(Self::Swapped),
            _ => Err(format!("Unknown task status: {s}")),
        }
    }
}

/// Risk level for task recovery decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RiskLevel {
    /// Low risk: read-only operations, safe to auto-resume
    Low,
    /// High risk: write operations, requires user confirmation
    High,
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::High => write!(f, "high"),
        }
    }
}

impl RiskLevel {
    /// Parse risk level from database string with fallback to Low
    pub fn from_str_or_default(s: &str) -> Self {
        match s.parse() {
            Ok(level) => level,
            Err(e) => {
                tracing::warn!(input = %s, error = %e, "Unknown risk level, defaulting to Low");
                Self::Low
            }
        }
    }
}

impl std::str::FromStr for RiskLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "high" => Ok(Self::High),
            _ => Err(format!("Unknown risk level: {s}")),
        }
    }
}

/// Priority lane for resource isolation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Lane {
    /// Main lane: user interactions, abort commands (high priority)
    Main,
    /// Subagent lane: background work (normal priority)
    Subagent,
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Main => write!(f, "main"),
            Self::Subagent => write!(f, "subagent"),
        }
    }
}

impl Lane {
    /// Parse lane from database string with fallback to Subagent
    pub fn from_str_or_default(s: &str) -> Self {
        match s.parse() {
            Ok(lane) => lane,
            Err(e) => {
                tracing::warn!(input = %s, error = %e, "Unknown lane, defaulting to Subagent");
                Self::Subagent
            }
        }
    }
}

impl std::str::FromStr for Lane {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "main" => Ok(Self::Main),
            "subagent" => Ok(Self::Subagent),
            _ => Err(format!("Unknown lane: {s}")),
        }
    }
}

// =============================================================================
// Agent Task
// =============================================================================

/// Agent task with recovery support
///
/// Represents a task dispatched to a subagent, with all necessary
/// metadata for Shadow Replay recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentTask {
    /// Unique task identifier
    pub id: String,

    /// Parent session ID (for task grouping)
    pub parent_session_id: String,

    /// Agent type handling this task (e.g., "explorer", "coder")
    pub agent_id: String,

    /// Original task prompt/instruction
    pub task_prompt: String,

    /// Current task status
    pub status: TaskStatus,

    /// Risk level for recovery decisions
    pub risk_level: RiskLevel,

    /// Priority lane
    pub lane: Lane,

    /// Path to checkpoint snapshot (for Shadow Replay)
    pub checkpoint_snapshot_path: Option<String>,

    /// ID of last executed tool call
    pub last_tool_call_id: Option<String>,

    /// Recursion depth (for Recursive Sentry)
    pub recursion_depth: u32,

    /// Parent task ID (for recursion tracking)
    pub parent_task_id: Option<String>,

    /// Creation timestamp (Unix epoch seconds)
    pub created_at: i64,

    /// Last update timestamp
    pub updated_at: i64,

    /// When task started executing
    pub started_at: Option<i64>,

    /// When task completed
    pub completed_at: Option<i64>,

    /// Extensible metadata (JSON)
    pub metadata_json: Option<String>,
}

impl AgentTask {
    /// Create a new pending task
    pub fn new(
        id: impl Into<String>,
        parent_session_id: impl Into<String>,
        agent_id: impl Into<String>,
        task_prompt: impl Into<String>,
        risk_level: RiskLevel,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: id.into(),
            parent_session_id: parent_session_id.into(),
            agent_id: agent_id.into(),
            task_prompt: task_prompt.into(),
            status: TaskStatus::Pending,
            risk_level,
            lane: Lane::Subagent,
            checkpoint_snapshot_path: None,
            last_tool_call_id: None,
            recursion_depth: 0,
            parent_task_id: None,
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
            metadata_json: None,
        }
    }

    /// Builder: set parent task for recursion tracking
    pub fn with_parent_task(mut self, parent_id: impl Into<String>, parent_depth: u32) -> Self {
        self.parent_task_id = Some(parent_id.into());
        self.recursion_depth = parent_depth.saturating_add(1);
        self
    }

    /// Builder: set lane
    #[must_use]
    pub const fn with_lane(mut self, lane: Lane) -> Self {
        self.lane = lane;
        self
    }

    /// Check if task should auto-resume on restart
    #[must_use]
    pub fn should_auto_resume(&self) -> bool {
        self.status.is_recoverable() && self.risk_level == RiskLevel::Low
    }

    /// Check if task needs user confirmation to resume
    #[must_use]
    pub fn needs_resume_confirmation(&self) -> bool {
        self.status.is_recoverable() && self.risk_level == RiskLevel::High
    }
}

// =============================================================================
// Task Trace
// =============================================================================

/// Execution trace entry for Shadow Replay
///
/// Records a single structured execution event in task execution,
/// enabling deterministic replay without rebuilding semantics from flat logs.
///
/// Deliberately NOT `#[non_exhaustive]`: `alephcore` is workspace-internal, so
/// the attribute buys no semver protection and only locks out in-repo consumers
/// that are separate crates — `tests/gateway_trace_replay_rpc.rs` builds these
/// with explicit ids/timestamps that `new()` cannot express.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTrace {
    /// Auto-incremented ID
    pub id: i64,

    /// Associated task ID
    pub task_id: String,

    /// Step index within the task (0-based)
    pub step_index: u32,

    /// Structured agent trace event
    pub event: AgentTraceEvent,

    /// Timestamp (Unix epoch seconds)
    pub timestamp: i64,
}

impl TaskTrace {
    /// Create a new trace entry
    pub fn new(
        task_id: impl Into<String>,
        step_index: u32,
        event: impl Into<AgentTraceEvent>,
    ) -> Self {
        Self {
            id: 0, // Will be set by database
            task_id: task_id.into(),
            step_index,
            event: event.into(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Replace the wall-clock stamp [`new`](Self::new) applied.
    ///
    /// `new` stamps `Utc::now()`, which is what live recording wants but leaves
    /// no way to place a trace at a chosen instant. Trace listing paginates on a
    /// `timestamp` cursor with strict less-than semantics, so seeding an ordered
    /// fixture needs to say *when* — and outside this crate `#[non_exhaustive]`
    /// rules out reaching for the struct literal.
    #[must_use]
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Stable event kind string for storage/indexing.
    #[must_use]
    pub const fn event_kind(&self) -> &'static str {
        self.event.kind()
    }
}

/// Summary info for trace listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTraceInfo {
    pub task_id: String,
    pub event_count: i64,
    pub last_timestamp: i64,
}

// =============================================================================
// Agent Event
// =============================================================================

/// Agent event for tiered persistence (Skeleton & Pulse model)
///
/// Structural events (skeleton) are persisted immediately.
/// Streaming events (pulse) are batched before persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Auto-incremented ID
    pub id: i64,

    /// Associated task ID
    pub task_id: String,

    /// Sequence number for ordering (Gap-Fill support)
    pub seq: u64,

    /// Event type identifier
    pub event_type: String,

    /// Full event payload as JSON
    pub payload_json: String,

    /// Whether this is a structural event (skeleton)
    pub is_structural: bool,

    /// Timestamp (Unix epoch seconds)
    pub timestamp: i64,
}

impl AgentEvent {
    /// Create a new event (`is_structural` determined by caller)
    pub fn new(
        task_id: impl Into<String>,
        seq: u64,
        event_type: impl Into<String>,
        payload_json: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            task_id: task_id.into(),
            seq,
            event_type: event_type.into(),
            payload_json: payload_json.into(),
            is_structural: false,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create a structural (skeleton) event
    pub fn structural(
        task_id: impl Into<String>,
        seq: u64,
        event_type: impl Into<String>,
        payload_json: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            task_id: task_id.into(),
            seq,
            event_type: event_type.into(),
            payload_json: payload_json.into(),
            is_structural: true,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create a pulse event (batched)
    pub fn pulse(
        task_id: impl Into<String>,
        seq: u64,
        event_type: impl Into<String>,
        payload_json: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            task_id: task_id.into(),
            seq,
            event_type: event_type.into(),
            payload_json: payload_json.into(),
            is_structural: false,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Common structural event types
    pub const TYPE_TASK_STARTED: &'static str = "task_started";
    pub const TYPE_TOOL_CALL_STARTED: &'static str = "tool_call_started";
    pub const TYPE_TOOL_CALL_COMPLETED: &'static str = "tool_call_completed";
    pub const TYPE_ARTIFACT_CREATED: &'static str = "artifact_created";
    pub const TYPE_TASK_COMPLETED: &'static str = "task_completed";
    pub const TYPE_TASK_FAILED: &'static str = "task_failed";

    /// Common pulse event types
    pub const TYPE_AI_STREAMING: &'static str = "ai_streaming";
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_status_display() {
        assert_eq!(TaskStatus::Pending.to_string(), "pending");
        assert_eq!(TaskStatus::Running.to_string(), "running");
        assert_eq!(TaskStatus::Completed.to_string(), "completed");
        assert_eq!(TaskStatus::Interrupted.to_string(), "interrupted");
    }

    #[test]
    fn test_task_status_from_str() {
        assert_eq!(
            TaskStatus::from_str_or_default("RUNNING"),
            TaskStatus::Running
        );
        assert_eq!(
            TaskStatus::from_str_or_default("interrupted"),
            TaskStatus::Interrupted
        );
        assert_eq!(
            TaskStatus::from_str_or_default("unknown"),
            TaskStatus::Pending
        );
    }

    #[test]
    fn test_task_status_recoverable() {
        assert!(TaskStatus::Running.is_recoverable());
        assert!(TaskStatus::Interrupted.is_recoverable());
        assert!(!TaskStatus::Completed.is_recoverable());
        assert!(!TaskStatus::Failed.is_recoverable());
    }

    #[test]
    fn test_agent_task_new() {
        let task = AgentTask::new(
            "task-1",
            "session-1",
            "explorer",
            "Search for files",
            RiskLevel::Low,
        );

        assert_eq!(task.id, "task-1");
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.risk_level, RiskLevel::Low);
        assert_eq!(task.lane, Lane::Subagent);
        assert_eq!(task.recursion_depth, 0);
    }

    #[test]
    fn test_agent_task_with_parent() {
        let task = AgentTask::new(
            "task-2",
            "session-1",
            "coder",
            "Write code",
            RiskLevel::High,
        )
        .with_parent_task("task-1", 1);

        assert_eq!(task.parent_task_id, Some("task-1".to_string()));
        assert_eq!(task.recursion_depth, 2);
    }

    #[test]
    fn test_task_recovery_decisions() {
        let low_risk = AgentTask::new("t1", "s1", "explorer", "search", RiskLevel::Low);
        let high_risk = AgentTask::new("t2", "s1", "coder", "write", RiskLevel::High);

        // Both are pending, so not recoverable
        assert!(!low_risk.should_auto_resume());
        assert!(!high_risk.needs_resume_confirmation());
    }

    #[test]
    fn test_task_trace_new() {
        let trace = TaskTrace::new(
            "task-1",
            0,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: aleph_protocol::AgentTraceTextKind::Final,
                text: "hello".to_string(),
            },
        );

        assert_eq!(trace.task_id, "task-1");
        assert_eq!(trace.step_index, 0);
        assert_eq!(trace.event.kind(), "text_emitted");
    }

    #[test]
    fn test_agent_event_structural() {
        let event = AgentEvent::structural(
            "task-1",
            1,
            AgentEvent::TYPE_TOOL_CALL_STARTED,
            r#"{"tool":"search"}"#,
        );

        assert!(event.is_structural);
        assert_eq!(event.event_type, "tool_call_started");
    }

    #[test]
    fn test_agent_event_pulse() {
        let event = AgentEvent::pulse(
            "task-1",
            2,
            AgentEvent::TYPE_AI_STREAMING,
            r#"{"delta":"..."}"#,
        );

        assert!(!event.is_structural);
    }

    #[test]
    fn with_timestamp_replaces_the_wall_clock_stamp() {
        // The trace-listing cursor paginates on `timestamp` with strict
        // less-than semantics, so an ordered fixture needs traces at chosen
        // instants — `new` alone stamps `Utc::now()` for every one of them.
        let event = aleph_protocol::AgentTraceEvent::TextEmitted {
            iteration: 0,
            stream: aleph_protocol::AgentTraceTextKind::Final,
            text: "payload".into(),
        };
        // rust-doctor-disable-next-line excessive-clone
        let now_stamped = TaskTrace::new("task-1", 0, event.clone());
        let pinned = TaskTrace::new("task-1", 0, event).with_timestamp(1_700_000_000);

        assert_eq!(pinned.timestamp, 1_700_000_000);
        assert_ne!(pinned.timestamp, now_stamped.timestamp);
        // Everything `new` established survives the override.
        assert_eq!(pinned.id, 0, "the database still assigns the id");
        assert_eq!(pinned.task_id, "task-1");
        assert_eq!(pinned.event_kind(), now_stamped.event_kind());
    }
}
