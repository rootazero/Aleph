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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// What kind of failure a [`SessionEvent::Error`] receipt records.
///
/// Deliberately NOT open vocabulary. `Llm`, `Tool`, `Sandbox`, `Harness`,
/// `Serialization` and `Other` were removed for the same reason
/// [`ApprovalSource::Autoconfirm`] was: nothing constructed them, so no stored
/// event could carry one and nothing could ever read one back. The next kind
/// arrives in the same commit as the producer that emits it — a variant is a
/// claim about what the log can contain, and an unproduced one is false.
pub enum ErrorKind {
    /// A guardrail refused the run's input. Produced by
    /// [`crate::orchestrator::harness_bridge`] when a run finishes having
    /// screened its input and said nothing.
    Guardrail,
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

impl SessionEvent {
    /// A plain-text `UserMessage` the harness wrote itself — a grace nudge, a
    /// stop-hook halt, a verifier veto.
    ///
    /// Three call sites in `src/harness/` had built this literal by hand, which
    /// is one past the point where the duplication should collapse (P6). The
    /// reason to collapse it *here* rather than leave three copies is the last
    /// field: a harness-authored message has no human author by construction,
    /// and this is what makes that unforgettable rather than merely true today.
    /// Adding a fourth synthetic message in the loop must not require
    /// remembering spec §6.2 — and it must not spend R10 budget on remembering.
    #[must_use]
    pub fn synthetic_user(turn_id: TurnId, text: String) -> Self {
        SessionEvent::UserMessage {
            turn_id,
            content: MessageContent {
                text,
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: true,
            author_user_id: None,
        }
    }
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

/// The session-knob envelope a run started under, frozen onto its
/// `RunStarted` marker so a resume replays the crashed run's configuration
/// instead of re-deriving it from whatever the knobs say now.
///
/// Every field is a **String**, spelled with the same literal word the
/// `identity_meta.custom` bag uses, so the snapshot, the session row and the
/// client-facing `SessionSnapshot` share one vocabulary rather than three
/// enums that have to be kept convertible. The key set is pinned against
/// [`crate::gateway::session_snapshot::RUN_ENVELOPE_KNOB_KEYS`] by a census
/// test: a seventh knob has to appear in both places or that test fails.
///
/// `model` / `model_provider` are the pair the run was **actually bound to**
/// after provider validation — not the pin that was asked for. A resume that
/// replayed the unvalidated hint would re-derive a route the crashed run never
/// took.
///
/// Absent from the wire when `None` — a legacy log deserialises to `None`,
/// which `ResumeReport::unsnapshotted` counts rather than papers over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunEnvelopeSnapshot {
    /// `ExecTier::id()` — the tier the run was executing under. On resume this
    /// is a **ceiling**, never a request: recovery may only tighten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,
    /// `SessionMode::id()` — chat / work / code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_mode: Option<String>,
    /// `ThinkLevel::id()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_level: Option<String>,
    /// `MemoryMode::id()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mode: Option<String>,
    /// Model id the run was **served by** — the directive's model when it
    /// carried one, else what the provider chain said it was about to serve.
    ///
    /// `None` means the writer could not name the model AT ALL, not "the run
    /// carried no pin". The narrower reading is load-bearing: a resume that
    /// finds `None` here re-derives the model from today's session, which is a
    /// different model than the crashed run used whenever the session was
    /// re-pinned in between — so `plan_resume` treats `None` as a degrade and
    /// says so, rather than answering on a substitute in silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider the model above was pinned to, or `None` for an unqualified
    /// pin (the resolver picks the provider by model-name heuristic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
}

impl RunEnvelopeSnapshot {
    /// True when the writer resolved nothing at all.
    ///
    /// Distinct from a `None` envelope: `None` means *no writer captured one*
    /// (a legacy marker, or a producer — split / compaction / sub-agent — that
    /// has no envelope to capture), while an empty one means the capture
    /// happened and the gateway had resolved nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exec_tier.is_none()
            && self.session_mode.is_none()
            && self.think_level.is_none()
            && self.memory_mode.is_none()
            && self.model.is_none()
            && self.model_provider.is_none()
    }
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
        /// The knob envelope this run started under (see
        /// [`RunEnvelopeSnapshot`]). `None` on every marker written before
        /// the snapshot existed and on markers whose writer did not capture
        /// one; omitted from the wire when `None` so the legacy forms stay
        /// byte-identical.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        envelope: Option<RunEnvelopeSnapshot>,
    },
    /// A harness run reached a terminal state on this session.
    RunFinished {
        run_id: String,
        outcome: RunOutcome,
        at: Timestamp,
    },

    /// A turn opened. There is deliberately no closing `TurnEnded` marker: a
    /// turn ends when the next one opens or when the run does, and the crash
    /// boundary is read off the `RunStarted`/`RunFinished` pair instead
    /// ([`crate::session::reduction::reduce_disposition`]). A
    /// `TurnEnded` variant existed here for a long time with no producer, so
    /// every turn matched "crashed mid-turn" and nothing could use it.
    TurnStarted {
        turn_id: TurnId,
        trigger: TurnTrigger,
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
        /// Who typed this, in a multi-human project room (spec §6.2). `None`
        /// for every single-author session, for every harness-authored
        /// message, and for every event written before P2 — absent means "the
        /// session's own user", the same adoption-by-absence rule the rest of
        /// the multi-user arc uses.
        ///
        /// Stamped from [`crate::scope::room_author`], which is the single
        /// source for *when* a message needs an author at all. Only the id is
        /// stored: a display name is presentation, resolved fresh at render
        /// time through `scope::directory`, so a rename shows up in history
        /// instead of being frozen into it.
        ///
        /// Mirrors the [`SessionEvent::RunStarted::project_root`] precedent —
        /// an optional payload field, not a side channel — and like it,
        /// `skip_serializing_if` keeps it off the wire and out of the prompt's
        /// cached prefix for the single-author sessions that are still the
        /// overwhelming majority.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author_user_id: Option<String>,
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

    /// A durable receipt for a failure that produced no other trace.
    ///
    /// Its one producer is the input-guardrail block receipt in
    /// [`crate::orchestrator::harness_bridge`]: a screened-out input ends the
    /// run `Ok`, so without this the log reads as a clean empty run and every
    /// re-attaching client (reload, second tab, room peer) sees an unanswered
    /// user message. Projected to a `system` row by
    /// [`crate::session::projection::project_row`] so `chat.history` serves it;
    /// NOT prompt-bearing — the model must not be told its own refusal was
    /// something it said.
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
            envelope: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        assert!(json.contains("\"type\":\"run_started\""));
        // Optional fields are omitted on the wire when None so the legacy
        // 2-field form stays byte-identical for old event-log readers.
        assert!(!json.contains("project_root"));
        assert!(!json.contains("envelope"));
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
            envelope: None,
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
    /// (no `project_root` key, no `envelope` key) and the 3-field form that
    /// predates `envelope` both yield `None` for every absent optional field
    /// thanks to `#[serde(default)]`.
    #[test]
    fn run_started_legacy_log_deserialises_with_none() {
        let two_field = r#"{"type":"run_started","run_id":"old","at":1700000000000}"#;
        let three_field =
            r#"{"type":"run_started","run_id":"old","at":1700000000000,"project_root":"/p"}"#;
        for (legacy, expected_root) in [(two_field, None), (three_field, Some("/p"))] {
            let back: SessionEvent = serde_json::from_str(legacy).unwrap();
            match back {
                SessionEvent::RunStarted {
                    project_root,
                    envelope,
                    ..
                } => {
                    assert_eq!(project_root.as_deref(), expected_root);
                    assert!(
                        envelope.is_none(),
                        "legacy log {legacy} must carry no envelope"
                    );
                }
                other => panic!("expected RunStarted, got {other:?}"),
            }
        }
    }

    /// Third generation: a marker written by a build that captures the ④
    /// envelope. Round-trips, and every field survives.
    #[test]
    fn run_started_with_an_envelope_round_trips() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-env".into(),
            at: 1_700_000_000_000,
            project_root: Some("/p".into()),
            envelope: Some(RunEnvelopeSnapshot {
                exec_tier: Some("full".into()),
                session_mode: Some("code".into()),
                think_level: Some("high".into()),
                memory_mode: Some("off".into()),
                model: Some("m-old".into()),
                model_provider: Some("p-old".into()),
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        match back {
            SessionEvent::RunStarted { envelope, .. } => {
                let env = envelope.expect("envelope survives the round trip");
                assert_eq!(env.exec_tier.as_deref(), Some("full"));
                assert_eq!(env.session_mode.as_deref(), Some("code"));
                assert_eq!(env.think_level.as_deref(), Some("high"));
                assert_eq!(env.memory_mode.as_deref(), Some("off"));
                assert_eq!(env.model.as_deref(), Some("m-old"));
                assert_eq!(env.model_provider.as_deref(), Some("p-old"));
                assert!(!env.is_empty());
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    /// A captured-but-empty envelope is NOT the same answer as an absent one:
    /// it serialises as `{}` and deserialises back to `Some`, which is what
    /// lets `ResumeReport::unsnapshotted` mean "no writer captured one"
    /// instead of "the gateway had resolved nothing".
    #[test]
    fn an_empty_envelope_is_still_some() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-empty".into(),
            at: 1,
            project_root: None,
            envelope: Some(RunEnvelopeSnapshot::default()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"envelope\":{}"), "{json}");
        match serde_json::from_str::<SessionEvent>(&json).unwrap() {
            SessionEvent::RunStarted { envelope, .. } => {
                let env = envelope.expect("an empty object is Some, not None");
                assert!(env.is_empty());
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    /// Census: the envelope's key set IS the knob vocabulary
    /// `session_snapshot` publishes. A seventh knob added to one side and not
    /// the other fails here — which is the whole reason the array exists.
    #[test]
    fn the_envelope_carries_exactly_the_published_knob_keys() {
        let all = RunEnvelopeSnapshot {
            exec_tier: Some("a".into()),
            session_mode: Some("b".into()),
            think_level: Some("c".into()),
            memory_mode: Some("d".into()),
            model: Some("e".into()),
            model_provider: Some("f".into()),
        };
        let value = serde_json::to_value(&all).unwrap();
        let mut got: Vec<String> = value
            .as_object()
            .expect("the envelope serialises as an object")
            .keys()
            .cloned()
            .collect();
        got.sort();
        let mut want: Vec<String> =
            crate::gateway::session_snapshot::RUN_ENVELOPE_KNOB_KEYS
                .iter()
                .map(|k| (*k).to_string())
                .collect();
        want.sort();
        assert_eq!(got, want);
    }

    /// The four `custom`-bag names in that array are the ones the session
    /// snapshot's decoder actually reads — not just four strings that happen
    /// to match the struct. Feeds a metadata bag keyed by the array itself and
    /// asserts each value comes back out.
    #[test]
    fn the_custom_bag_names_in_the_array_are_the_ones_the_decoder_reads() {
        use crate::gateway::session_manager::SessionIdentityMeta;
        use crate::gateway::session_snapshot::{snapshot_from_metadata, RUN_ENVELOPE_KNOB_KEYS};
        use crate::gateway::session_store::types::SessionMetadata;

        let mut identity = SessionIdentityMeta::default();
        for (i, value) in ["full", "code", "high", "off"].iter().enumerate() {
            identity
                .custom
                .insert(RUN_ENVELOPE_KNOB_KEYS[i].to_string(), (*value).into());
        }
        let meta = SessionMetadata {
            identity_meta: Some(identity),
            model: Some("m".to_string()),
            model_provider: Some("p".to_string()),
            ..SessionMetadata::default()
        };
        let snap = snapshot_from_metadata(&meta);
        assert_eq!(snap.exec_tier.as_deref(), Some("full"));
        assert_eq!(snap.mode.as_deref(), Some("code"));
        assert_eq!(snap.think_level.as_deref(), Some("high"));
        assert_eq!(snap.memory_mode.as_deref(), Some("off"));
        assert_eq!(snap.model.as_deref(), Some("m"));
        assert_eq!(snap.model_provider.as_deref(), Some("p"));
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
    fn a_project_rooms_user_message_carries_its_author() {
        let ev = SessionEvent::UserMessage {
            turn_id: TurnId::new_v4(),
            content: MessageContent {
                text: "ship it".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: 1,
            synthetic: false,
            author_user_id: Some("u-alice".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"author_user_id\":\"u-alice\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn a_single_author_message_puts_no_author_on_the_wire() {
        // `skip_serializing_if` is not cosmetic here: these bytes sit in the
        // prompt's cached prefix, and a `"author_user_id":null` on every
        // message of every single-author session is a per-turn tax paid by the
        // sessions that get nothing back for it.
        let ev = SessionEvent::synthetic_user(TurnId::new_v4(), "hint".into());
        assert!(!serde_json::to_string(&ev)
            .unwrap()
            .contains("author_user_id"));
    }

    #[test]
    fn a_pre_p2_event_without_an_author_still_deserializes() {
        // On-disk session logs predate this field. `#[serde(default)]` is what
        // keeps every historical log readable, and reading it back as `None` is
        // adoption-by-absence: an unlabelled message belongs to the session's
        // own user, exactly as it always did.
        let legacy = r#"{"type":"user_message","turn_id":"11111111-1111-4111-8111-111111111111","content":{"text":"hi"},"at":7,"synthetic":false}"#;
        let ev: SessionEvent = serde_json::from_str(legacy).unwrap();
        match ev {
            SessionEvent::UserMessage {
                author_user_id,
                content,
                ..
            } => {
                assert_eq!(author_user_id, None);
                assert_eq!(content.text, "hi");
            }
            other => panic!("expected a user message, got {other:?}"),
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
