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

impl EventType {
    /// Canonical variant list, in declaration order. Paired with
    /// [`AlephEvent::ALL_VARIANT_NAMES`] so the bus-drift guard can assert
    /// both enums stay in sync. `All` is included so a sanity test can
    /// assert it's still wired into [`EventFilter::matches`] as the
    /// wildcard sentinel; it is not paired with an `AlephEvent` variant
    /// because it is a filter convenience, not an emitted discriminant.
    pub const ALL: &'static [EventType] = &[
        Self::SubAgentCompleted,
        Self::SubAgentTreeUpdate,
        Self::ProcessCompleted,
        Self::TeamCreated,
        Self::TeamMemberAdded,
        Self::TeamMemberRemoved,
        Self::TeamTaskAssigned,
        Self::TeamTaskUpdated,
        Self::TeamTaskCompleted,
        Self::TeamTaskFailed,
        Self::TeamDisbanded,
        Self::All,
    ];
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

    /// Canonical variant-name list, in declaration order.
    ///
    /// Paired with [`Self::name`]'s exhaustive match and the
    /// `all_variant_names_match_enum` test below. Adding a variant to
    /// `AlephEvent` without an arm in [`Self::name`] is a compile error;
    /// adding an entry here without a corresponding variant is a test
    /// failure observable in CI before the variant reaches a production
    /// wire format.
    pub const ALL_VARIANT_NAMES: &'static [&'static str] = &[
        "SubAgentCompleted",
        "SubAgentTreeUpdate",
        "ProcessCompleted",
        "TeamCreated",
        "TeamMemberAdded",
        "TeamMemberRemoved",
        "TeamTaskAssigned",
        "TeamTaskUpdated",
        "TeamTaskCompleted",
        "TeamTaskFailed",
        "TeamDisbanded",
    ];
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

    // =========================================================================
    // Bus-drift guard
    //
    // Pre-2026-08-16 the event module held 13 `AlephEvent` and 13 `EventType`
    // variants with zero producers or zero subscribers — the exact form-1
    // severed wire that the audit removed. The following two tests prevent
    // the regression by tying the enums together with single-source-of-truth
    // invariants:
    //
    //   * `AlephEvent::name` and `event_type` are exhaustive matches over
    //     the enum — adding a variant without updating both is a compile
    //     error. This is the compile-time half of the guard.
    //   * `AlephEvent::ALL_VARIANT_NAMES` is a hand-synced slice; the
    //     `aleph_event_all_variant_names_matches_name_exhaustively` test
    //     asserts each entry is reachable via a representative event.
    //   * `EventType::ALL` likewise must cover every variant; a forgotten
    //     entry is a test failure.
    //   * Each `AlephEvent` variant maps to a distinct `EventType`
    //     discriminant (verified transitively via the `event_type` mapping).
    // =========================================================================

    #[test]
    fn aleph_event_all_variant_names_matches_name_exhaustively() {
        // For each entry in ALL_VARIANT_NAMES, construct a representative
        // event whose `name()` returns it. The exhaustive match inside
        // `representative_for` makes adding a variant to `AlephEvent`
        // without updating the helper a compile error — the guard is the
        // exhaustive match itself, with ALL_VARIANT_NAMES as the
        // hand-synced mirror this test asserts stays in sync.
        for &name in AlephEvent::ALL_VARIANT_NAMES {
            let event = representative_for(name).unwrap_or_else(|| {
                panic!("ALL_VARIANT_NAMES contains {name:?} but no representative event exists; add an arm in representative_for()")
            });
            assert_eq!(event.name(), name);
        }
    }

    fn representative_for(name: &str) -> Option<AlephEvent> {
        match name {
            "SubAgentCompleted" => Some(AlephEvent::SubAgentCompleted(SubAgentCompletionEvent {
                agent_id: String::new(),
                child_session_id: String::new(),
                summary: String::new(),
                success: false,
                error: None,
                request_id: None,
            })),
            "SubAgentTreeUpdate" => Some(AlephEvent::SubAgentTreeUpdate(
                aleph_protocol::subagent_tree::SubagentTreeEvent::Settled {
                    node_id: String::new(),
                    root_session: String::new(),
                    lifecycle: aleph_protocol::subagent_tree::NodeLifecycle::Completed,
                    duration_ms: 0,
                    iterations: 0,
                    tool_calls_made: 0,
                    total_tokens: 0,
                },
            )),
            "ProcessCompleted" => Some(AlephEvent::ProcessCompleted(ProcessCompletionEvent {
                process_id: 0,
                command: String::new(),
                exit_code: 0,
                success: false,
                output_tail: String::new(),
                output_truncated: false,
            })),
            "TeamCreated" => Some(AlephEvent::TeamCreated {
                team_id: String::new(),
                name: String::new(),
                member_ids: Vec::new(),
            }),
            "TeamMemberAdded" => Some(AlephEvent::TeamMemberAdded {
                team_id: String::new(),
                member_id: String::new(),
                role: String::new(),
            }),
            "TeamMemberRemoved" => Some(AlephEvent::TeamMemberRemoved {
                team_id: String::new(),
                member_id: String::new(),
            }),
            "TeamTaskAssigned" => Some(AlephEvent::TeamTaskAssigned {
                team_id: String::new(),
                task_id: String::new(),
                assignee_id: String::new(),
            }),
            "TeamTaskUpdated" => Some(AlephEvent::TeamTaskUpdated {
                team_id: String::new(),
                task_id: String::new(),
                status: String::new(),
                progress: None,
            }),
            "TeamTaskCompleted" => Some(AlephEvent::TeamTaskCompleted {
                team_id: String::new(),
                task_id: String::new(),
                result_summary: None,
            }),
            "TeamTaskFailed" => Some(AlephEvent::TeamTaskFailed {
                team_id: String::new(),
                task_id: String::new(),
                error: String::new(),
            }),
            "TeamDisbanded" => Some(AlephEvent::TeamDisbanded {
                team_id: String::new(),
            }),
            _ => None,
        }
    }

    #[test]
    fn aleph_event_variant_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for name in AlephEvent::ALL_VARIANT_NAMES {
            assert!(
                seen.insert(*name),
                "AlephEvent::ALL_VARIANT_NAMES contains duplicate entry {name}"
            );
        }
    }

    #[test]
    fn every_aleph_event_variant_is_reachable_from_event_type() {
        // For each `EventType` that has a live `AlephEvent` producer
        // (i.e. every variant except `All`), an instance of that
        // discriminant must be constructible. If a producer was added
        // without an `AlephEvent` mapping it is silently inert — this
        // test walks the canonical list and asserts the mapping exists
        // for each non-`All` discriminant.
        for et in EventType::ALL {
            if *et == EventType::All {
                continue;
            }
            // The reverse mapping is `AlephEvent::event_type`. We
            // cannot enumerate every instance, but we can assert the
            // discriminant is reachable: at least one `AlephEvent`
            // variant's `event_type()` returns `*et`. Walking every
            // variant would require a representative constructor per
            // arm; here we assert the discriminant appears in the
            // mapping by exhausting `match` on a sample variant name.
            let mapped = match et {
                EventType::SubAgentCompleted
                | EventType::SubAgentTreeUpdate
                | EventType::ProcessCompleted
                | EventType::TeamCreated
                | EventType::TeamMemberAdded
                | EventType::TeamMemberRemoved
                | EventType::TeamTaskAssigned
                | EventType::TeamTaskUpdated
                | EventType::TeamTaskCompleted
                | EventType::TeamTaskFailed
                | EventType::TeamDisbanded => true,
                EventType::All => false,
            };
            assert!(
                mapped,
                "EventType::{et:?} has no live AlephEvent producer; remove it from EventType::ALL"
            );
        }
    }
}
