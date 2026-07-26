// Aleph/core/src/event/types.rs
//! Event type definitions for the event-driven architecture.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// Core Event Types
// ============================================================================

/// Timestamped event wrapper for history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampedEvent {
    pub event: AlephEvent,
    pub timestamp: i64,
    pub sequence: u64,
}

impl TimestampedEvent {
    /// Create a new timestamped event with the given sequence number.
    #[must_use]
    pub fn new(event: AlephEvent, sequence: u64) -> Self {
        Self {
            event,
            timestamp: chrono::Utc::now().timestamp_millis(),
            sequence,
        }
    }
}

/// Event type discriminant for subscription filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    // Input
    InputReceived,

    // Tool execution
    ToolCallRequested,
    ToolCallStarted,
    ToolCallCompleted,
    ToolCallFailed,
    ToolCallRetrying,

    // Loop control
    LoopContinue,
    LoopStop,

    // Session
    SessionCreated,
    SessionUpdated,
    SessionResumed,
    SessionCompacted,

    // Sub-agent
    SubAgentCompleted,
    SubAgentTreeUpdate,

    // AI response
    AiResponseGenerated,

    // Part updates (for UI message flow rendering)
    PartAdded,
    PartUpdated,
    PartRemoved,

    // Team events
    TeamCreated,
    TeamMemberAdded,
    TeamMemberRemoved,
    TeamTaskAssigned,
    TeamTaskUpdated,
    TeamTaskCompleted,
    TeamTaskFailed,
    TeamDisbanded,

    // Wildcard for components that want all events
    All,
}

/// Unified event enum - all events in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AlephEvent {
    // Input events
    InputReceived(InputEvent),

    // Tool execution events
    ToolCallRequested(ToolCallRequest),
    ToolCallStarted(ToolCallStarted),
    ToolCallCompleted(ToolCallResult),
    ToolCallFailed(ToolCallError),
    ToolCallRetrying(ToolCallRetry),

    // Loop control events
    LoopContinue(LoopState),
    LoopStop(StopReason),

    // Session events
    SessionCreated(SessionInfo),
    SessionUpdated(SessionDiff),
    SessionResumed(SessionInfo),
    SessionCompacted(CompactionInfo),

    // Sub-agent events
    SubAgentCompleted(SubAgentCompletionEvent),
    /// Live sub-agent tree update (spawned / progress / settled) — fed to the
    /// panel's background sub-agent tree view via the gateway relay. Pure
    /// observability; carries no reasoning (R4/R10).
    SubAgentTreeUpdate(aleph_protocol::subagent_tree::SubagentTreeEvent),

    // AI response events
    AiResponseGenerated(AiResponse),

    // Part update events (for UI message flow rendering)
    PartAdded(crate::components::PartUpdateData),
    PartUpdated(crate::components::PartUpdateData),
    PartRemoved(crate::components::PartUpdateData),

    // Team events
    TeamCreated {
        team_id: String,
        name: String,
        member_ids: Vec<String>,
    },
    TeamMemberAdded {
        team_id: String,
        member_id: String,
        role: String,
    },
    TeamMemberRemoved {
        team_id: String,
        member_id: String,
    },
    TeamTaskAssigned {
        team_id: String,
        task_id: String,
        assignee_id: String,
    },
    TeamTaskUpdated {
        team_id: String,
        task_id: String,
        status: String,
        progress: Option<f32>,
    },
    TeamTaskCompleted {
        team_id: String,
        task_id: String,
        result_summary: Option<String>,
    },
    TeamTaskFailed {
        team_id: String,
        task_id: String,
        error: String,
    },
    TeamDisbanded {
        team_id: String,
    },
    // NOTE: no `TeamMessageSent` — message sends are logged directly into the
    // team event log by `MessageRouter::send`; a global-bus variant existed
    // with zero publishers (its only producer sat behind a `with_bus` builder
    // nothing called) and was removed (R10 zero-consumer).
}

impl AlephEvent {
    /// Get the event type discriminant
    #[must_use]
    pub const fn event_type(&self) -> EventType {
        match self {
            Self::InputReceived(_) => EventType::InputReceived,
            Self::ToolCallRequested(_) => EventType::ToolCallRequested,
            Self::ToolCallStarted(_) => EventType::ToolCallStarted,
            Self::ToolCallCompleted(_) => EventType::ToolCallCompleted,
            Self::ToolCallFailed(_) => EventType::ToolCallFailed,
            Self::ToolCallRetrying(_) => EventType::ToolCallRetrying,
            Self::LoopContinue(_) => EventType::LoopContinue,
            Self::LoopStop(_) => EventType::LoopStop,
            Self::SessionCreated(_) => EventType::SessionCreated,
            Self::SessionUpdated(_) => EventType::SessionUpdated,
            Self::SessionResumed(_) => EventType::SessionResumed,
            Self::SessionCompacted(_) => EventType::SessionCompacted,
            Self::SubAgentCompleted(_) => EventType::SubAgentCompleted,
            Self::SubAgentTreeUpdate(_) => EventType::SubAgentTreeUpdate,
            Self::AiResponseGenerated(_) => EventType::AiResponseGenerated,
            Self::PartAdded(_) => EventType::PartAdded,
            Self::PartUpdated(_) => EventType::PartUpdated,
            Self::PartRemoved(_) => EventType::PartRemoved,
            Self::TeamCreated { .. } => EventType::TeamCreated,
            Self::TeamMemberAdded { .. } => EventType::TeamMemberAdded,
            Self::TeamMemberRemoved { .. } => EventType::TeamMemberRemoved,
            Self::TeamTaskAssigned { .. } => EventType::TeamTaskAssigned,
            Self::TeamTaskUpdated { .. } => EventType::TeamTaskUpdated,
            Self::TeamTaskCompleted { .. } => EventType::TeamTaskCompleted,
            Self::TeamTaskFailed { .. } => EventType::TeamTaskFailed,
            Self::TeamDisbanded { .. } => EventType::TeamDisbanded,
        }
    }

    /// Get a human-readable name for the event
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::InputReceived(_) => "InputReceived",
            Self::ToolCallRequested(_) => "ToolCallRequested",
            Self::ToolCallStarted(_) => "ToolCallStarted",
            Self::ToolCallCompleted(_) => "ToolCallCompleted",
            Self::ToolCallFailed(_) => "ToolCallFailed",
            Self::ToolCallRetrying(_) => "ToolCallRetrying",
            Self::LoopContinue(_) => "LoopContinue",
            Self::LoopStop(_) => "LoopStop",
            Self::SessionCreated(_) => "SessionCreated",
            Self::SessionUpdated(_) => "SessionUpdated",
            Self::SessionResumed(_) => "SessionResumed",
            Self::SessionCompacted(_) => "SessionCompacted",
            Self::SubAgentCompleted(_) => "SubAgentCompleted",
            Self::SubAgentTreeUpdate(_) => "SubAgentTreeUpdate",
            Self::AiResponseGenerated(_) => "AiResponseGenerated",
            Self::PartAdded(_) => "PartAdded",
            Self::PartUpdated(_) => "PartUpdated",
            Self::PartRemoved(_) => "PartRemoved",
            Self::TeamCreated { .. } => "TeamCreated",
            Self::TeamMemberAdded { .. } => "TeamMemberAdded",
            Self::TeamMemberRemoved { .. } => "TeamMemberRemoved",
            Self::TeamTaskAssigned { .. } => "TeamTaskAssigned",
            Self::TeamTaskUpdated { .. } => "TeamTaskUpdated",
            Self::TeamTaskCompleted { .. } => "TeamTaskCompleted",
            Self::TeamTaskFailed { .. } => "TeamTaskFailed",
            Self::TeamDisbanded { .. } => "TeamDisbanded",
        }
    }
}

// ============================================================================
// Input Event Types
// ============================================================================

/// User input event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputEvent {
    pub text: String,
    pub session_id: Option<String>,
    pub context: Option<InputContext>,
    pub timestamp: i64,
}

/// Context captured with user input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputContext {
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub selected_text: Option<String>,
}

// ============================================================================
// Tool Execution Event Types
// ============================================================================

/// Request to call a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub tool: String,
    pub parameters: Value,
}

/// Tool call has started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallStarted {
    pub call_id: String,
    pub tool: String,
    pub input: Value,
    pub timestamp: i64,
    /// Session ID for sub-agent correlation (optional for backwards compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Tool call completed successfully
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub tool: String,
    pub input: Value,
    pub output: String,
    pub started_at: i64,
    pub completed_at: i64,
    pub token_usage: TokenUsage,
    /// Session ID for sub-agent correlation (optional for backwards compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Tool call failed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallError {
    pub call_id: String,
    pub tool: String,
    pub input: Value,
    pub error: String,
    pub error_kind: ErrorKind,
    pub is_retryable: bool,
    pub attempts: u32,
    /// Session ID for sub-agent correlation (optional for backwards compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Error classification for retry logic
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    InvalidInput,
    PermissionDenied,
    Timeout,
    RateLimit,
    ServiceUnavailable,
    ExecutionFailed,
    Aborted,
}

/// Tool call is being retried
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRetry {
    pub call_id: String,
    pub attempt: u32,
    pub delay_ms: u64,
    pub reason: Option<String>,
}

/// Token usage tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// ============================================================================
// Loop Control Event Types
// ============================================================================

/// Current state of the agentic loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    pub session_id: String,
    pub iteration: u32,
    pub total_tokens: u64,
    pub last_tool: Option<String>,
    /// Model identifier for context limit lookup
    #[serde(default)]
    pub model: String,
}

/// Reason for stopping the loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopReason {
    /// Task completed normally
    Completed,
    /// Hit iteration limit
    MaxIterationsReached,
    /// Detected infinite loop
    DoomLoopDetected,
    /// Context overflow
    TokenLimitReached,
    /// User cancelled
    UserAborted,
    /// Unrecoverable error
    Error(String),
}

// ============================================================================
// Session Event Types
// ============================================================================

/// Session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub model: String,
    pub created_at: i64,
}

/// Session state diff for incremental updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDiff {
    pub session_id: String,
    pub iteration_count: Option<u32>,
    pub total_tokens: Option<u64>,
    pub status: Option<String>,
}

/// Session compaction information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionInfo {
    pub session_id: String,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub timestamp: i64,
}

// ============================================================================
// Sub-agent Event Types
// ============================================================================

/// Sub-agent completed its task. Payload of [`AlephEvent::SubAgentCompleted`].
///
/// Named distinctly from `agents::sub_agents::SubAgentResult` (the A2A
/// delegation result type) — the old shared `SubAgentResult` name was a
/// permanent grep collision between two unrelated types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentCompletionEvent {
    pub agent_id: String,
    pub child_session_id: String,
    pub summary: String,
    pub success: bool,
    pub error: Option<String>,
    /// Request ID for result correlation (optional for backwards compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// ============================================================================
// User Interaction Event Types
// ============================================================================

/// Question asked to user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserQuestion {
    pub question_id: String,
    pub question: String,
    pub options: Option<Vec<String>>,
}

// ============================================================================
// AI Response Event Types
// ============================================================================

/// AI generated response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    pub content: String,
    pub reasoning: Option<String>,
    pub is_final: bool,
    pub timestamp: i64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_mapping() {
        let event = AlephEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            session_id: None,
            context: None,
            timestamp: 0,
        });

        assert_eq!(event.event_type(), EventType::InputReceived);
        assert_eq!(event.name(), "InputReceived");
    }

    #[test]
    fn test_timestamped_event_sequence() {
        let e1 = TimestampedEvent::new(AlephEvent::LoopStop(StopReason::Completed), 1);
        let e2 = TimestampedEvent::new(AlephEvent::LoopStop(StopReason::Completed), 2);

        assert!(e2.sequence > e1.sequence);
    }

    #[test]
    fn test_event_serialization() {
        let event = AlephEvent::ToolCallCompleted(ToolCallResult {
            call_id: "123".to_string(),
            tool: "search".to_string(),
            input: serde_json::json!({"query": "test"}),
            output: "results".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
            session_id: None,
        });

        let json = serde_json::to_string(&event).unwrap();
        let parsed: AlephEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.event_type(), EventType::ToolCallCompleted);
    }

    #[test]
    fn test_part_event_types() {
        use crate::components::{
            PartEventType, PartUpdateData, SessionPart, ToolCallPart, ToolCallStatus,
        };

        // Create a tool call part
        let tool_call = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "search".to_string(),
            input: serde_json::json!({"query": "test"}),
            status: ToolCallStatus::Running,
            output: None,
            error: None,
            started_at: 1000,
            completed_at: None,
        });

        // Test PartAdded event
        let added_data = PartUpdateData::added("session-1", &tool_call).unwrap();
        let event = AlephEvent::PartAdded(added_data.clone());
        assert_eq!(event.event_type(), EventType::PartAdded);
        assert_eq!(event.name(), "PartAdded");
        assert_eq!(added_data.event_type, PartEventType::Added);
        assert_eq!(added_data.part_type, "tool_call");
        assert_eq!(added_data.session_id, "session-1");

        // Test PartUpdated event with delta
        let delta_data =
            PartUpdateData::text_delta("session-1", "response-1", "ai_response", "Hello, ");
        let event = AlephEvent::PartUpdated(delta_data.clone());
        assert_eq!(event.event_type(), EventType::PartUpdated);
        assert_eq!(event.name(), "PartUpdated");
        assert_eq!(delta_data.delta, Some("Hello, ".to_string()));

        // Test PartRemoved event
        let removed_data = PartUpdateData::removed("session-1", "call-1", "tool_call");
        let event = AlephEvent::PartRemoved(removed_data.clone());
        assert_eq!(event.event_type(), EventType::PartRemoved);
        assert_eq!(event.name(), "PartRemoved");
        assert_eq!(removed_data.event_type, PartEventType::Removed);
    }

    #[test]
    fn test_loop_state_model_field() {
        // Test with model field
        let state = LoopState {
            session_id: "test-session".to_string(),
            iteration: 5,
            total_tokens: 100_000,
            last_tool: Some("search".to_string()),
            model: "gpt-4-turbo".to_string(),
        };

        assert_eq!(state.model, "gpt-4-turbo");

        // Test serialization with model
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("gpt-4-turbo"));

        // Test deserialization with model
        let parsed: LoopState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, "gpt-4-turbo");
    }

    #[test]
    fn test_loop_state_model_default() {
        // Test deserialization without model field (backwards compatibility)
        let json = r#"{
            "session_id": "test",
            "iteration": 1,
            "total_tokens": 1000,
            "last_tool": null
        }"#;

        let parsed: LoopState = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.model, ""); // Should default to empty string
    }
}
