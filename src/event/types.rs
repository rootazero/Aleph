// Aleph/core/src/event/types.rs
//! Event type definitions for the event-driven architecture.
//!
//! Scope: only event variants with a live producer and a live subscriber are
//! present here. Dead variants (InputReceived, ToolCall*, Loop*, Session*,
//! AiResponseGenerated, etc.) were removed in the 2026-08-16 severed-wire
//! audit — their structs/enums had zero production consumers and were only
//! kept alive by tests inside this module.

use serde::{Deserialize, Serialize};

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

/// Event type discriminant for subscription filtering.
///
/// Only variants with a live producer AND a live subscriber are listed — a
/// filter that mentions an EventType with no emitter is the form-1 dead
/// scaffolding that the 2026-08-16 severed-wire audit removed. Adding a new
/// variant here requires both halves; `AlephEvent::event_type` is the compile
/// gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    // Sub-agent
    SubAgentCompleted,
    /// Live sub-agent tree update (spawned / progress / settled) — fed to the
    /// panel's background sub-agent tree view via the gateway relay. Pure
    /// observability; carries no reasoning (R4/R10).
    SubAgentTreeUpdate,

    // Background `bash` jobs
    ProcessCompleted,

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

/// Unified event enum - all events with a live producer in the system.
///
/// Adding a new variant here requires: (a) an emitter that calls
/// `GlobalBus::broadcast(...)` or `EventBus::publish(...)`, and (b) a
/// subscriber registered through `EventFilter::new(vec![...])` or
/// `handler.subscriptions()`. A variant with neither is a dead variant — see
/// the 2026-08-16 audit for the 13 dead variants removed from this enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AlephEvent {
    // Sub-agent events
    SubAgentCompleted(SubAgentCompletionEvent),
    /// Live sub-agent tree update (spawned / progress / settled) — fed to the
    /// panel's background sub-agent tree view via the gateway relay. Pure
    /// observability; carries no reasoning (R4/R10).
    SubAgentTreeUpdate(aleph_protocol::subagent_tree::SubagentTreeEvent),

    // Background `bash` job events
    ProcessCompleted(ProcessCompletionEvent),

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
            Self::SubAgentCompleted(_) => EventType::SubAgentCompleted,
            Self::SubAgentTreeUpdate(_) => EventType::SubAgentTreeUpdate,
            Self::ProcessCompleted(_) => EventType::ProcessCompleted,
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
            Self::SubAgentCompleted(_) => "SubAgentCompleted",
            Self::SubAgentTreeUpdate(_) => "SubAgentTreeUpdate",
            Self::ProcessCompleted(_) => "ProcessCompleted",
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
// Background Process Event Types
// ============================================================================

/// A background `bash` job reached a natural completion. Payload of
/// [`AlephEvent::ProcessCompleted`].
///
/// Broadcast only from the detached task in `builtin_tools::bash_exec`, and only
/// when that task's `ProcessRegistry::complete` actually performed the
/// `Running → Done` transition. A killed job produces none: the owner asked for
/// the stop, so its outcome is not news — the same stance
/// `subagent_tool::spawn` takes for a cancelled child.
///
/// Every string here is already masked by the producer. The reader is a *later*
/// turn whose reply may fan out to a chat channel, so redaction cannot be left
/// to whoever renders it (§5.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCompletionEvent {
    /// Registry id — the value the model passes back as `process_id`.
    pub process_id: u64,
    /// Masked, truncated command preview (the text `list` shows).
    pub command: String,
    pub exit_code: i32,
    pub success: bool,
    /// Masked **tail** of the finished output. Bounded on purpose: the notice
    /// opens a model turn, and the full output stays one `poll` away.
    pub output_tail: String,
    /// Whether [`output_tail`](Self::output_tail) had a head cut off it. A tail
    /// presented as the whole output is how a model concludes a build printed
    /// nothing before its last line.
    #[serde(default)]
    pub output_truncated: bool,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_mapping() {
        let event = AlephEvent::SubAgentCompleted(SubAgentCompletionEvent {
            agent_id: "a".into(),
            child_session_id: "s".into(),
            summary: "done".into(),
            success: true,
            error: None,
            request_id: None,
        });

        assert_eq!(event.event_type(), EventType::SubAgentCompleted);
        assert_eq!(event.name(), "SubAgentCompleted");
    }

    #[test]
    fn test_timestamped_event_sequence() {
        let e1 = TimestampedEvent::new(
            AlephEvent::SubAgentCompleted(SubAgentCompletionEvent {
                agent_id: "a".into(),
                child_session_id: "s".into(),
                summary: "done".into(),
                success: true,
                error: None,
                request_id: None,
            }),
            1,
        );
        let e2 = TimestampedEvent::new(
            AlephEvent::SubAgentCompleted(SubAgentCompletionEvent {
                agent_id: "a".into(),
                child_session_id: "s".into(),
                summary: "done".into(),
                success: true,
                error: None,
                request_id: None,
            }),
            2,
        );

        assert!(e2.sequence > e1.sequence);
    }

    #[test]
    fn test_event_serialization() {
        let event = AlephEvent::ProcessCompleted(ProcessCompletionEvent {
            process_id: 1,
            command: "echo".into(),
            exit_code: 0,
            success: true,
            output_tail: "ok".into(),
            output_truncated: false,
        });

        let json = serde_json::to_string(&event).unwrap();
        let parsed: AlephEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.event_type(), EventType::ProcessCompleted);
    }
}
