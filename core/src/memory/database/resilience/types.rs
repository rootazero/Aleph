//! Multi-Agent Resilience Types
//!
//! Core types for the Multi-Agent Resilience architecture:
//! - AgentTask: Task state and recovery checkpoints
//! - TaskTrace: Execution traces for Shadow Replay
//! - AgentEvent: Tiered event persistence (Skeleton & Pulse)
//! - SubagentSession: Long-lived subagent session management

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Enums
// =============================================================================

/// Task execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::Running => write!(f, "running"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
            TaskStatus::Interrupted => write!(f, "interrupted"),
            TaskStatus::Idle => write!(f, "idle"),
            TaskStatus::Swapped => write!(f, "swapped"),
        }
    }
}

impl TaskStatus {
    /// Parse status from database string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pending" => TaskStatus::Pending,
            "running" => TaskStatus::Running,
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            "interrupted" => TaskStatus::Interrupted,
            "idle" => TaskStatus::Idle,
            "swapped" => TaskStatus::Swapped,
            _ => TaskStatus::Pending,
        }
    }

    /// Check if task can be auto-resumed on restart
    pub fn is_recoverable(&self) -> bool {
        matches!(self, TaskStatus::Running | TaskStatus::Interrupted)
    }
}

/// Risk level for task recovery decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Low risk: read-only operations, safe to auto-resume
    Low,
    /// High risk: write operations, requires user confirmation
    High,
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "low"),
            RiskLevel::High => write!(f, "high"),
        }
    }
}

impl RiskLevel {
    /// Parse risk level from database string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "high" => RiskLevel::High,
            _ => RiskLevel::Low,
        }
    }
}

/// Priority lane for resource isolation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// Main lane: user interactions, abort commands (high priority)
    Main,
    /// Subagent lane: background work (normal priority)
    Subagent,
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lane::Main => write!(f, "main"),
            Lane::Subagent => write!(f, "subagent"),
        }
    }
}

impl Lane {
    /// Parse lane from database string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "main" => Lane::Main,
            _ => Lane::Subagent,
        }
    }
}

/// Session status for subagent lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session is actively executing
    Active,
    /// Session is idle, waiting for reuse (in memory)
    Idle,
    /// Session context has been swapped out to disk
    Swapped,
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionStatus::Active => write!(f, "active"),
            SessionStatus::Idle => write!(f, "idle"),
            SessionStatus::Swapped => write!(f, "swapped"),
        }
    }
}

impl SessionStatus {
    /// Parse session status from database string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "active" => SessionStatus::Active,
            "idle" => SessionStatus::Idle,
            "swapped" => SessionStatus::Swapped,
            _ => SessionStatus::Active,
        }
    }
}

/// Role in task trace (message sender)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceRole {
    /// Assistant message (including tool calls)
    Assistant,
    /// Tool result message
    Tool,
}

impl fmt::Display for TraceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceRole::Assistant => write!(f, "assistant"),
            TraceRole::Tool => write!(f, "tool"),
        }
    }
}

impl TraceRole {
    /// Parse role from database string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "tool" => TraceRole::Tool,
            _ => TraceRole::Assistant,
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
        self.recursion_depth = parent_depth + 1;
        self
    }

    /// Builder: set lane
    pub fn with_lane(mut self, lane: Lane) -> Self {
        self.lane = lane;
        self
    }

    /// Check if task should auto-resume on restart
    pub fn should_auto_resume(&self) -> bool {
        self.status.is_recoverable() && self.risk_level == RiskLevel::Low
    }

    /// Check if task needs user confirmation to resume
    pub fn needs_resume_confirmation(&self) -> bool {
        self.status.is_recoverable() && self.risk_level == RiskLevel::High
    }
}

// =============================================================================
// Task Trace
// =============================================================================

/// Execution trace entry for Shadow Replay
///
/// Records a single step in task execution, enabling deterministic
/// replay without LLM re-inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTrace {
    /// Auto-incremented ID
    pub id: i64,

    /// Associated task ID
    pub task_id: String,

    /// Step index within the task (0-based)
    pub step_index: u32,

    /// Message role
    pub role: TraceRole,

    /// Full message content as JSON
    pub content_json: String,

    /// Timestamp (Unix epoch seconds)
    pub timestamp: i64,
}

impl TaskTrace {
    /// Create a new trace entry
    pub fn new(
        task_id: impl Into<String>,
        step_index: u32,
        role: TraceRole,
        content_json: impl Into<String>,
    ) -> Self {
        Self {
            id: 0, // Will be set by database
            task_id: task_id.into(),
            step_index,
            role,
            content_json: content_json.into(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }
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
// Subagent Session
// =============================================================================

/// Long-lived subagent session (Session-as-a-Service)
///
/// Allows subagents to persist across multiple task invocations,
/// supporting handle reuse and context inheritance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSession {
    /// Unique session identifier (also the handle)
    pub id: String,

    /// Agent type (e.g., "explorer", "coder", "researcher")
    pub agent_type: String,

    /// Current session status
    pub status: SessionStatus,

    /// Path to serialized context (for swapped sessions)
    pub context_path: Option<String>,

    /// Parent session ID
    pub parent_session_id: String,

    /// Creation timestamp
    pub created_at: i64,

    /// Last activity timestamp
    pub last_active_at: i64,

    /// Total tokens consumed by this session
    pub total_tokens_used: u64,

    /// Total tool calls made by this session
    pub total_tool_calls: u64,
}

impl SubagentSession {
    /// Create a new active session
    pub fn new(
        id: impl Into<String>,
        agent_type: impl Into<String>,
        parent_session_id: impl Into<String>,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: id.into(),
            agent_type: agent_type.into(),
            status: SessionStatus::Active,
            context_path: None,
            parent_session_id: parent_session_id.into(),
            created_at: now,
            last_active_at: now,
            total_tokens_used: 0,
            total_tool_calls: 0,
        }
    }

    /// Check if session is in memory (Active or Idle)
    pub fn is_in_memory(&self) -> bool {
        matches!(self.status, SessionStatus::Active | SessionStatus::Idle)
    }

    /// Check if session can be swapped out
    pub fn can_swap_out(&self) -> bool {
        self.status == SessionStatus::Idle
    }
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
        assert_eq!(TaskStatus::from_str("RUNNING"), TaskStatus::Running);
        assert_eq!(TaskStatus::from_str("interrupted"), TaskStatus::Interrupted);
        assert_eq!(TaskStatus::from_str("unknown"), TaskStatus::Pending);
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
        let trace = TaskTrace::new("task-1", 0, TraceRole::Assistant, r#"{"content":"hello"}"#);

        assert_eq!(trace.task_id, "task-1");
        assert_eq!(trace.step_index, 0);
        assert_eq!(trace.role, TraceRole::Assistant);
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
    fn test_subagent_session_new() {
        let session = SubagentSession::new("sess-1", "explorer", "parent-1");

        assert_eq!(session.id, "sess-1");
        assert_eq!(session.status, SessionStatus::Active);
        assert!(session.is_in_memory());
        assert!(!session.can_swap_out());
    }

    #[test]
    fn test_session_status_swap_out() {
        let mut session = SubagentSession::new("sess-1", "explorer", "parent-1");
        session.status = SessionStatus::Idle;

        assert!(session.is_in_memory());
        assert!(session.can_swap_out());

        session.status = SessionStatus::Swapped;
        assert!(!session.is_in_memory());
        assert!(!session.can_swap_out());
    }
}
