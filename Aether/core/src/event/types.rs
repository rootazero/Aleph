// Aether/core/src/event/types.rs
//! Event type definitions for the event-driven architecture.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

// Global sequence counter for events
static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Generate next event sequence number
fn next_sequence() -> u64 {
    EVENT_SEQUENCE.fetch_add(1, Ordering::SeqCst)
}

// ============================================================================
// Core Event Types
// ============================================================================

/// Timestamped event wrapper for history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampedEvent {
    pub event: AetherEvent,
    pub timestamp: i64,
    pub sequence: u64,
}

impl TimestampedEvent {
    pub fn new(event: AetherEvent) -> Self {
        Self {
            event,
            timestamp: chrono::Utc::now().timestamp_millis(),
            sequence: next_sequence(),
        }
    }
}

/// Event type discriminant for subscription filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    // Input
    InputReceived,

    // Planning
    PlanRequested,
    PlanCreated,

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
    SubAgentStarted,
    SubAgentCompleted,

    // User interaction
    UserQuestionAsked,
    UserResponseReceived,

    // AI response
    AiResponseGenerated,

    // Wildcard for components that want all events
    All,
}

/// Unified event enum - all events in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AetherEvent {
    // Input events
    InputReceived(InputEvent),

    // Planning events
    PlanRequested(PlanRequest),
    PlanCreated(TaskPlan),

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
    SubAgentStarted(SubAgentRequest),
    SubAgentCompleted(SubAgentResult),

    // User interaction events
    UserQuestionAsked(UserQuestion),
    UserResponseReceived(UserResponse),

    // AI response events
    AiResponseGenerated(AiResponse),
}

impl AetherEvent {
    /// Get the event type discriminant
    pub fn event_type(&self) -> EventType {
        match self {
            Self::InputReceived(_) => EventType::InputReceived,
            Self::PlanRequested(_) => EventType::PlanRequested,
            Self::PlanCreated(_) => EventType::PlanCreated,
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
            Self::SubAgentStarted(_) => EventType::SubAgentStarted,
            Self::SubAgentCompleted(_) => EventType::SubAgentCompleted,
            Self::UserQuestionAsked(_) => EventType::UserQuestionAsked,
            Self::UserResponseReceived(_) => EventType::UserResponseReceived,
            Self::AiResponseGenerated(_) => EventType::AiResponseGenerated,
        }
    }

    /// Get a human-readable name for the event
    pub fn name(&self) -> &'static str {
        match self {
            Self::InputReceived(_) => "InputReceived",
            Self::PlanRequested(_) => "PlanRequested",
            Self::PlanCreated(_) => "PlanCreated",
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
            Self::SubAgentStarted(_) => "SubAgentStarted",
            Self::SubAgentCompleted(_) => "SubAgentCompleted",
            Self::UserQuestionAsked(_) => "UserQuestionAsked",
            Self::UserResponseReceived(_) => "UserResponseReceived",
            Self::AiResponseGenerated(_) => "AiResponseGenerated",
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
    pub topic_id: Option<String>,
    pub context: Option<InputContext>,
    pub timestamp: i64,
}

/// Context captured with user input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputContext {
    pub app_name: Option<String>,
    pub app_bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub selected_text: Option<String>,
}

// ============================================================================
// Planning Event Types
// ============================================================================

/// Request to create a task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRequest {
    pub input: InputEvent,
    pub intent_type: Option<String>,
    pub detected_steps: Vec<String>,
}

/// Generated task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub id: String,
    pub steps: Vec<PlanStep>,
    pub parallel_groups: Vec<Vec<String>>,
    pub current_step_index: usize,
}

/// Single step in a task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub tool: String,
    pub parameters: Value,
    pub depends_on: Vec<String>,
    pub status: StepStatus,
}

/// Status of a plan step
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
}

// ============================================================================
// Tool Execution Event Types
// ============================================================================

/// Request to call a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub tool: String,
    pub parameters: Value,
    pub plan_step_id: Option<String>,
}

/// Tool call has started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallStarted {
    pub call_id: String,
    pub tool: String,
    pub input: Value,
    pub timestamp: i64,
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
}

/// Tool call failed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallError {
    pub call_id: String,
    pub tool: String,
    pub error: String,
    pub error_kind: ErrorKind,
    pub is_retryable: bool,
    pub attempts: u32,
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
    /// No steps to execute
    EmptyPlan,
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

/// Request to start a sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRequest {
    pub agent_id: String,
    pub prompt: String,
    pub parent_session_id: String,
    pub child_session_id: String,
}

/// Sub-agent completed its task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub agent_id: String,
    pub child_session_id: String,
    pub summary: String,
    pub success: bool,
    pub error: Option<String>,
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

/// User's response to a question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResponse {
    pub question_id: String,
    pub response: String,
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
        let event = AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        });

        assert_eq!(event.event_type(), EventType::InputReceived);
        assert_eq!(event.name(), "InputReceived");
    }

    #[test]
    fn test_timestamped_event_sequence() {
        let e1 = TimestampedEvent::new(AetherEvent::LoopStop(StopReason::Completed));
        let e2 = TimestampedEvent::new(AetherEvent::LoopStop(StopReason::Completed));

        assert!(e2.sequence > e1.sequence);
    }

    #[test]
    fn test_event_serialization() {
        let event = AetherEvent::ToolCallCompleted(ToolCallResult {
            call_id: "123".to_string(),
            tool: "search".to_string(),
            input: serde_json::json!({"query": "test"}),
            output: "results".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
        });

        let json = serde_json::to_string(&event).unwrap();
        let parsed: AetherEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.event_type(), EventType::ToolCallCompleted);
    }
}
