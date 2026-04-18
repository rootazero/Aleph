//! Event types for the session log.

use crate::gateway::session_manager::SessionIdentityMeta;
use serde::{Deserialize, Serialize};

pub type Timestamp = i64; // unix milliseconds
pub type EventSeq = u64;
pub type TurnId = uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnTrigger {
    UserMessage,
    SubagentRequest,
    Scheduled,
    Wake,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Errored { kind: ErrorKind },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSource {
    User,
    Trusted,
    Autoconfirm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Llm,
    Tool,
    Sandbox,
    Harness,
    Serialization,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageContent {
    /// Free-form text body (UI-displayable).
    pub text: String,
    /// Optional rich blocks (images, tool_use). Uses JSON to avoid pulling in
    /// provider-specific types at this layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolOutput {
    pub value: serde_json::Value,
    #[serde(default)]
    pub metadata: ToolOutputMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolOutputMetadata {
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub cost_cents: Option<u64>,
}

// NOTE: `PartialEq` is intentionally omitted from `SessionEvent` because
// `SessionIdentityMeta` (used by `SessionCreated`) does not implement it.
// Tests that need comparison should compare on the serialized JSON form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEvent {
    SessionCreated { identity: SessionIdentityMeta, at: Timestamp },
    SessionWoken { at: Timestamp, prior_head: EventSeq },
    SessionDetached { at: Timestamp },

    TurnStarted { turn_id: TurnId, trigger: TurnTrigger, at: Timestamp },
    TurnEnded { turn_id: TurnId, outcome: TurnOutcome, at: Timestamp },

    UserMessage { turn_id: TurnId, content: MessageContent, at: Timestamp },
    AssistantMessage { turn_id: TurnId, content: MessageContent, at: Timestamp },
    SystemMessage { turn_id: TurnId, content: String, at: Timestamp },

    LlmCallStarted { turn_id: TurnId, provider: String, model: String, at: Timestamp },
    LlmCallEnded {
        turn_id: TurnId,
        tokens_in: u32,
        tokens_out: u32,
        finish_reason: String,
        at: Timestamp,
    },

    ToolCallRequested {
        turn_id: TurnId,
        call_id: String,
        name: String,
        input: serde_json::Value,
        at: Timestamp,
    },
    ToolCallApproved { turn_id: TurnId, call_id: String, by: ApprovalSource, at: Timestamp },
    ToolCallDenied { turn_id: TurnId, call_id: String, reason: String, at: Timestamp },
    ToolResult { turn_id: TurnId, call_id: String, output: ToolOutput, at: Timestamp },
    ToolError { turn_id: TurnId, call_id: String, error: String, at: Timestamp },

    SubagentSpawned {
        turn_id: TurnId,
        child_id: crate::routing::session_key::SessionKey,
        flow: String,
        at: Timestamp,
    },
    SubagentReturned {
        turn_id: TurnId,
        child_id: crate::routing::session_key::SessionKey,
        summary: String,
        at: Timestamp,
    },

    BudgetUpdated {
        turn_id: TurnId,
        tokens_used: u32,
        tokens_budget: u32,
        at: Timestamp,
    },
    CompactionPerformed {
        from_seq: EventSeq,
        to_seq: EventSeq,
        summary_ref: String,
        at: Timestamp,
    },

    Error {
        turn_id: Option<TurnId>,
        kind: ErrorKind,
        message: String,
        recoverable: bool,
        at: Timestamp,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEventRecord {
    pub seq: EventSeq,
    pub event: SessionEvent,
    pub created_at_ms: Timestamp,
}

/// Current wall-clock in unix ms.
pub fn now_ms() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
