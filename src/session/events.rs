//! Event types for the session log.

use serde::{Deserialize, Serialize};

pub type Timestamp = i64; // unix milliseconds
pub type EventSeq = u64;
pub type TurnId = uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnTrigger {
    UserMessage,
    SubagentRequest,
    Scheduled,
    Wake,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Errored { kind: ErrorKind },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Who authorized a gated tool call.
///
/// `Autoconfirm` was removed: it had no constructor anywhere in the tree, so no
/// stored event can carry it and nothing could ever read it back. A variant
/// with no producer is a claim the enum cannot honour.
pub enum ApprovalSource {
    /// A human answered the prompt for this call.
    User,
    /// A grant taken earlier in the session satisfied the gate — nobody was
    /// asked this time. Produced by the session-approval-memory short circuit
    /// in `tools::scoped::dispatch::confirm_with_memory`.
    Trusted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Llm,
    Tool,
    Sandbox,
    Harness,
    Serialization,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageContent {
    /// Free-form text body (UI-displayable).
    pub text: String,
    /// Optional rich blocks (images, `tool_use`). Uses JSON to avoid pulling in
    /// provider-specific types at this layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<serde_json::Value>,
    /// Thinking/reasoning trace from extended-thinking models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Opaque signature accompanying the thinking content. Anthropic requires
    /// a signed thinking block to be replayed verbatim on subsequent turns
    /// whenever the same assistant message also contains `tool_use` blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutput {
    pub value: serde_json::Value,
    #[serde(default)]
    pub metadata: ToolOutputMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutputMetadata {
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub cost_cents: Option<u64>,
    /// Out-of-band image payloads carried alongside the (text) `value`.
    ///
    /// Some tools — desktop screenshots above all — produce an image the
    /// vision-capable model must actually *see*. The text result budget
    /// (`apply_layer_two`) would otherwise flatten and truncate the base64
    /// into oblivion, so the image is hoisted here BEFORE truncation and
    /// re-emitted as a `ContentBlock::Image` when the tool result is rendered
    /// into the prompt. Empty for the overwhelming majority of tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolImage>,
}

/// A single out-of-band image attached to a tool result (base64 + MIME).
///
/// Mirrors UI-TARS-desktop's "screenshot re-injection as a post-tool side
/// effect": the screen the model acted on is fed back as a viewable image on
/// the next turn, closing the perceive→act loop.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolImage {
    /// Base64-encoded image bytes (no `data:` URL prefix).
    pub data: String,
    /// MIME type, e.g. `image/png` or `image/jpeg`.
    pub mime_type: String,
}

/// Terminal disposition of a harness run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Run reached its natural end (model stop / final reply).
    Completed,
    /// Run was deliberately cancelled (user `/stop`). NOT resumed.
    Cancelled,
    /// Run ended with an error. NOT resumed (the error is in the log;
    /// re-running would likely hit the same error).
    Errored,
    /// Resume gave up on this run — cap reached or too old. Terminal.
    Abandoned,
}

// NOTE: `PartialEq` is intentionally omitted from `SessionEvent` because
// some variants carry types that do not implement it.
// Tests that need comparison should compare on the serialized JSON form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
// rust-doctor-disable-next-line large-enum-variant
pub enum SessionEvent {
    SessionWoken {
        at: Timestamp,
        prior_head: EventSeq,
    },

    /// A harness run began on this session.
    RunStarted {
        run_id: String,
        at: Timestamp,
        /// Project workspace this run was scoped to, when project-mode is
        /// active. Persisted so [`crate::gateway::resume_coordinator`] can
        /// re-trigger an interrupted run in the same project folder
        /// instead of falling back to `~/.aleph/workspaces/{agent_id}/`.
        /// Stored as a string (rather than `PathBuf`) so the JSON form
        /// stays platform-portable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_root: Option<String>,
    },
    /// A harness run reached a terminal state on this session.
    RunFinished {
        run_id: String,
        outcome: RunOutcome,
        at: Timestamp,
    },

    TurnStarted {
        turn_id: TurnId,
        trigger: TurnTrigger,
        at: Timestamp,
    },
    TurnEnded {
        turn_id: TurnId,
        outcome: TurnOutcome,
        at: Timestamp,
    },

    UserMessage {
        turn_id: TurnId,
        content: MessageContent,
        at: Timestamp,
        /// `true` when this entry was injected by the harness itself rather
        /// than coming from the real end-user (e.g. verifier-veto nudge,
        /// grace-turn `MAX_STEPS` hint). Defaults to `false` for backward
        /// compatibility with on-disk session logs that pre-date this field.
        ///
        /// The prompt builder (G2) wraps every *real* mid-loop user message
        /// in `<system-reminder>` so the model treats it as an interjection;
        /// synthetic messages are passed through unchanged.
        #[serde(default)]
        synthetic: bool,
    },
    AssistantMessage {
        turn_id: TurnId,
        content: MessageContent,
        /// What the provider billed for the ONE LLM call that produced this
        /// message. The harness emits an `AssistantMessage` per Think step, so
        /// calls and assistant rows are 1:1 and the attribution is exact — there
        /// is no "which of the run's N calls does this row own" to guess at.
        ///
        /// This is what `messages.input_tokens` / `output_tokens` are projected
        /// from. They had a column, a `MessageRecord` field, and were handed to
        /// the model (the `sessions` tool) and the Panel — as zeros, forever,
        /// because their only feeder was a `SessionEvent::LlmCallEnded` that no
        /// production code has ever emitted. A fabricated 0 reads as a
        /// measurement; this is the measurement.
        ///
        /// `None` on replayed pre-existing logs (hence `serde(default)`) and on
        /// a provider that reported no usage — absent, not zero.
        #[serde(default)]
        usage: Option<crate::orchestrator::dispatch::TokenBreakdown>,
        at: Timestamp,
    },
    /// Stamped after the assistant message row is written; carries the
    /// `run_id` and context-window occupancy so the projector can persist
    /// them onto the message metadata without coupling the hot path to storage.
    AssistantRunMeta {
        turn_id: TurnId,
        run_id: String,
        context_tokens: u32,
        context_window: u32,
        total_tokens: u64,
        /// Prompt tokens this run spent — the whole run, including the calls a
        /// retry discarded before they ever became a message. Accumulated onto
        /// the session row, which is why the session total is a superset of the
        /// sum of its message rows rather than equal to it.
        ///
        /// This event is the run's one authoritative billing report, so the
        /// session-level counters ride here and NOWHERE else: `add_message_full`
        /// used to also add each row's tokens onto the same three session
        /// columns, which was harmless only because those tokens were always 0.
        #[serde(default)]
        input_tokens: u32,
        /// Completion tokens this run spent. Same story as `input_tokens`.
        #[serde(default)]
        output_tokens: u32,
        /// This run's cost in USD, or `None` when it could not be priced.
        /// `None` ≠ 0.0 — an unpriced run must not silently understate the
        /// session total.
        #[serde(default)]
        cost_usd: Option<f64>,
        /// Model that served this run, and its provider — recorded onto
        /// `sessions.model` / `sessions.model_provider`.
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        model_provider: Option<String>,
        at: Timestamp,
    },
    SystemMessage {
        turn_id: TurnId,
        content: String,
        at: Timestamp,
    },

    ToolCallRequested {
        turn_id: TurnId,
        call_id: String,
        name: String,
        input: serde_json::Value,
        at: Timestamp,
    },
    ToolCallApproved {
        turn_id: TurnId,
        call_id: String,
        by: ApprovalSource,
        at: Timestamp,
    },
    ToolCallDenied {
        turn_id: TurnId,
        call_id: String,
        reason: String,
        at: Timestamp,
    },
    ToolResult {
        turn_id: TurnId,
        call_id: String,
        output: ToolOutput,
        at: Timestamp,
    },
    ToolError {
        turn_id: TurnId,
        call_id: String,
        error: String,
        at: Timestamp,
    },

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

    CompactionPerformed {
        from_seq: EventSeq,
        to_seq: EventSeq,
        summary_ref: String,
        at: Timestamp,
    },

    /// Recorded as the first event of a child session created by
    /// compaction-driven session-split. `parent_session_id` is the parent
    /// session key string (`SessionKey::to_key_string()`).
    SessionForked {
        parent_session_id: String,
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
#[must_use]
pub fn now_ms() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |e| {
            tracing::warn!(error = %e, "System clock went backwards — returning 0");
            0
        },
        |d| d.as_millis() as i64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_forked_event_round_trips_through_json() {
        let event = SessionEvent::SessionForked {
            parent_session_id: "agent:a/main:k:s2".to_string(),
            at: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            SessionEvent::SessionForked {
                parent_session_id, ..
            } => {
                assert_eq!(parent_session_id, "agent:a/main:k:s2");
            }
            other => panic!("expected SessionForked, got {other:?}"),
        }
    }

    #[test]
    fn run_started_serde_round_trips() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-abc".into(),
            at: 1_700_000_000_000,
            project_root: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        assert!(json.contains("\"type\":\"run_started\""));
        // Optional field is omitted on the wire when None so the legacy
        // 2-field form stays byte-identical for old event-log readers.
        assert!(!json.contains("project_root"));
    }

    /// New optional `project_root` field round-trips and survives the
    /// `#[serde(default)]` re-read path used by old logs (where the field
    /// simply doesn't exist).
    #[test]
    fn run_started_with_project_root_round_trips() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-pr".into(),
            at: 1_700_000_000_000,
            project_root: Some("/Users/alice/proj".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"project_root\":\"/Users/alice/proj\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        match back {
            SessionEvent::RunStarted { project_root, .. } => {
                assert_eq!(project_root.as_deref(), Some("/Users/alice/proj"));
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    /// Backward compatibility: deserialising a legacy 2-field RunStarted
    /// (no `project_root` key) yields `None` thanks to `#[serde(default)]`.
    #[test]
    fn run_started_legacy_log_deserialises_with_none() {
        let legacy = r#"{"type":"run_started","run_id":"old","at":1700000000000}"#;
        let back: SessionEvent = serde_json::from_str(legacy).unwrap();
        match back {
            SessionEvent::RunStarted { project_root, .. } => {
                assert!(project_root.is_none());
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[test]
    fn run_finished_serde_round_trips_each_outcome() {
        for outcome in [
            RunOutcome::Completed,
            RunOutcome::Cancelled,
            RunOutcome::Errored,
            RunOutcome::Abandoned,
        ] {
            let ev = SessionEvent::RunFinished {
                run_id: "run-xyz".into(),
                outcome,
                at: 1_700_000_000_000,
            };
            let json = serde_json::to_string(&ev).unwrap();
            let back: SessionEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
            assert!(json.contains("\"type\":\"run_finished\""));
        }
    }

    #[test]
    fn run_outcome_renames_snake_case() {
        assert_eq!(
            serde_json::to_string(&RunOutcome::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&RunOutcome::Abandoned).unwrap(),
            "\"abandoned\""
        );
    }
}
