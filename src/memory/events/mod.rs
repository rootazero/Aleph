//! Memory Event Sourcing
//!
//! Event-sourced memory lifecycle management. Every mutation to a `MemoryFact`
//! is captured as an immutable `MemoryEvent` wrapped in a `MemoryEventEnvelope`.
//!
//! ## Skeleton vs Pulse
//!
//! Events follow the Skeleton/Pulse classification from the resilience layer:
//! - **Skeleton** — structural mutations that must be persisted immediately
//!   (`NoteCreated`, `NoteContentUpdated`, `NoteMetadataUpdated`,
//!   `NoteInvalidated`, `NoteRestored`, `NoteDeleted`, `NoteConsolidated`, `NoteMigrated`)
//! - **Pulse** — high-frequency observations that may be buffered before persist
//!   (`NoteAccessed`)
//!
//! ## Submodules
//!
//! - `commands`   — command structs dispatched to the handler
//! - `handler`    — `MemoryCommandHandler` processes commands into events
//! - `projector`  — `fold_events_to_note` folds events into current-state projections
//! - `traveler`   — `MemoryTimeTraveler` replays events to reconstruct past state

pub mod commands;
pub mod handler;
pub mod projector;
pub mod testing;
pub mod traveler;

use serde::{Deserialize, Serialize};

use crate::memory::context::{FactSource, NoteType};

// ============================================================================
// EventActor — who caused the event
// ============================================================================

/// The actor that caused a memory event.
///
/// Was modeled after `memory::audit::AuditActor`, which has since been deleted
/// as an unwritten audit vocabulary (see [`crate::memory::explain`]). This enum
/// is the surviving one — it has real producers — and adds `Migration` for the
/// one-shot CRUD-to-ES migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventActor {
    /// AI agent performing automatic operations
    Agent,
    /// User performing manual operations
    User,
    /// System processes (compression, decay, consolidation, etc.)
    System,
    /// Decay mechanism (distinct from System for audit clarity)
    Decay,
    /// One-shot migration from legacy CRUD store
    Migration,
}

impl std::fmt::Display for EventActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Agent => write!(f, "agent"),
            Self::User => write!(f, "user"),
            Self::System => write!(f, "system"),
            Self::Decay => write!(f, "decay"),
            Self::Migration => write!(f, "migration"),
        }
    }
}

impl std::str::FromStr for EventActor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "agent" => Ok(Self::Agent),
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            "decay" => Ok(Self::Decay),
            "migration" => Ok(Self::Migration),
            _ => Err(format!("Unknown event actor: {s}")),
        }
    }
}

// ============================================================================
// MemoryEvent — the domain event enum
// ============================================================================

/// Domain events for the Memory bounded context.
///
/// Every mutation to a `MemoryFact` is captured as one of these variants.
/// The enum is internally tagged with `"type"` for deterministic serialization.
///
/// Field definitions match the design doc at
/// `docs/plans/2026-02-26-memory-event-sourcing-design.md`.
///
/// ## R2.2 rename — Note* variants with Fact* aliases
///
/// As of phase R2.2, variants are named `Note*` (matching the note-layer
/// terminology). Each variant carries `#[serde(alias = "Fact...")]` so legacy
/// on-disk events still deserialize. Likewise, the payload field
/// `note_path` carries `#[serde(alias = "fact_id")]` and `source_note_paths`
/// carries `#[serde(alias = "source_fact_ids")]` for backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
// rust-doctor-disable-next-line large-enum-variant
pub enum MemoryEvent {
    // ------------------------------------------------------------------
    // Skeleton events (immediate persist)
    // ------------------------------------------------------------------
    /// A new note was created
    #[serde(alias = "FactCreated")]
    NoteCreated {
        #[serde(alias = "fact_id")]
        note_path: String,
        content: String,
        note_type: NoteType,
        path: String,
        namespace: String,
        agent: String,
        source: FactSource,
        source_memory_ids: Vec<String>,
    },

    /// The textual content of a note was updated
    #[serde(alias = "FactContentUpdated")]
    NoteContentUpdated {
        #[serde(alias = "fact_id")]
        note_path: String,
        old_content: String,
        new_content: String,
        reason: String,
    },

    /// A single metadata field was updated
    #[serde(alias = "FactMetadataUpdated")]
    NoteMetadataUpdated {
        #[serde(alias = "fact_id")]
        note_path: String,
        field: String,
        old_value: String,
        new_value: String,
    },

    // ------------------------------------------------------------------
    // Pulse events (buffered persist)
    // ------------------------------------------------------------------
    /// The note was accessed / retrieved
    #[serde(alias = "FactAccessed")]
    NoteAccessed {
        #[serde(alias = "fact_id")]
        note_path: String,
        query: Option<String>,
        relevance_score: Option<f32>,
        used_in_response: bool,
        new_access_count: u32,
    },

    // ------------------------------------------------------------------
    // Skeleton events (continued)
    // ------------------------------------------------------------------
    /// The note was soft-deleted (invalidated)
    #[serde(alias = "FactInvalidated")]
    NoteInvalidated {
        #[serde(alias = "fact_id")]
        note_path: String,
        reason: String,
        actor: EventActor,
    },

    /// The note was restored from the recycle bin
    #[serde(alias = "FactRestored")]
    NoteRestored {
        #[serde(alias = "fact_id")]
        note_path: String,
    },

    /// The note was permanently deleted
    #[serde(alias = "FactDeleted")]
    NoteDeleted {
        #[serde(alias = "fact_id")]
        note_path: String,
        reason: String,
    },

    /// Multiple notes were consolidated into this one
    #[serde(alias = "FactConsolidated")]
    NoteConsolidated {
        #[serde(alias = "fact_id")]
        note_path: String,
        #[serde(alias = "source_fact_ids")]
        source_note_paths: Vec<String>,
        consolidated_content: String,
    },

    /// The note was migrated from the legacy CRUD store
    #[serde(alias = "FactMigrated")]
    NoteMigrated {
        #[serde(alias = "fact_id")]
        note_path: String,
        snapshot: serde_json::Value,
    },
}

impl MemoryEvent {
    /// Extract the `note_path` (legacy: `fact_id`) from any event variant.
    #[must_use]
    pub fn fact_id(&self) -> &str {
        match self {
            Self::NoteCreated { note_path, .. }
            | Self::NoteContentUpdated { note_path, .. }
            | Self::NoteMetadataUpdated { note_path, .. }
            | Self::NoteAccessed { note_path, .. }
            | Self::NoteInvalidated { note_path, .. }
            | Self::NoteRestored { note_path, .. }
            | Self::NoteDeleted { note_path, .. }
            | Self::NoteConsolidated { note_path, .. }
            | Self::NoteMigrated { note_path, .. } => note_path,
        }
    }

    /// Return the serde tag string for this event variant.
    ///
    /// Matches the `#[serde(tag = "type")]` discriminant so callers can
    /// filter events by type without deserializing the full payload.
    #[must_use]
    pub const fn event_type_tag(&self) -> &'static str {
        match self {
            Self::NoteCreated { .. } => "NoteCreated",
            Self::NoteContentUpdated { .. } => "NoteContentUpdated",
            Self::NoteMetadataUpdated { .. } => "NoteMetadataUpdated",
            Self::NoteAccessed { .. } => "NoteAccessed",
            Self::NoteInvalidated { .. } => "NoteInvalidated",
            Self::NoteRestored { .. } => "NoteRestored",
            Self::NoteDeleted { .. } => "NoteDeleted",
            Self::NoteConsolidated { .. } => "NoteConsolidated",
            Self::NoteMigrated { .. } => "NoteMigrated",
        }
    }

    /// Whether this event is a Skeleton event (must be persisted immediately).
    ///
    /// Only `NoteAccessed` is Pulse (buffered).
    /// All other variants are Skeleton.
    #[must_use]
    pub const fn is_skeleton(&self) -> bool {
        !matches!(self, Self::NoteAccessed { .. })
    }
}

// ============================================================================
// MemoryEventEnvelope — metadata wrapper
// ============================================================================

/// Immutable envelope wrapping a `MemoryEvent` with metadata.
///
/// Stored as a single row in the event store (`SQLite`). The `id` field
/// is the `SQLite` auto-increment primary key (0 before insert, assigned
/// on write). The `seq` field provides per-fact monotonic ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEventEnvelope {
    /// Auto-increment global ID (assigned by `SQLite` on insert; 0 before insert).
    pub id: i64,
    /// The fact this event belongs to.
    pub fact_id: String,
    /// Per-fact monotonic sequence number (1-based).
    pub seq: u64,
    /// The domain event payload.
    pub event: MemoryEvent,
    /// Who caused this event.
    pub actor: EventActor,
    /// When the event occurred (Unix timestamp, seconds).
    pub timestamp: i64,
    /// Optional correlation to a task or session.
    pub correlation_id: Option<String>,
}

impl MemoryEventEnvelope {
    /// Build a new envelope. `id` is set to 0 (assigned by DB on insert).
    #[must_use]
    pub fn new(
        fact_id: String,
        seq: u64,
        event: MemoryEvent,
        actor: EventActor,
        correlation_id: Option<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs() as i64;
        Self {
            id: 0,
            fact_id,
            seq,
            event,
            actor,
            timestamp: now,
            correlation_id,
        }
    }

    /// Convenience: return the event type tag.
    #[must_use]
    pub fn event_type_tag(&self) -> &'static str {
        self.event.event_type_tag()
    }

    /// Convenience: whether the inner event is Skeleton.
    #[must_use]
    pub fn is_skeleton(&self) -> bool {
        self.event.is_skeleton()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- EventActor ---------------------------------------------------------

    #[test]
    fn test_event_actor_display() {
        assert_eq!(EventActor::Agent.to_string(), "agent");
        assert_eq!(EventActor::User.to_string(), "user");
        assert_eq!(EventActor::System.to_string(), "system");
        assert_eq!(EventActor::Decay.to_string(), "decay");
        assert_eq!(EventActor::Migration.to_string(), "migration");
    }

    #[test]
    fn test_event_actor_from_str() {
        assert_eq!("agent".parse::<EventActor>().unwrap(), EventActor::Agent);
        assert_eq!("USER".parse::<EventActor>().unwrap(), EventActor::User);
        assert_eq!("System".parse::<EventActor>().unwrap(), EventActor::System);
        assert_eq!("decay".parse::<EventActor>().unwrap(), EventActor::Decay);
        assert_eq!(
            "migration".parse::<EventActor>().unwrap(),
            EventActor::Migration
        );
    }

    #[test]
    fn test_event_actor_from_str_unknown() {
        assert!("unknown".parse::<EventActor>().is_err());
    }

    #[test]
    fn test_event_actor_roundtrip() {
        for actor in &[
            EventActor::Agent,
            EventActor::User,
            EventActor::System,
            EventActor::Decay,
            EventActor::Migration,
        ] {
            let s = actor.to_string();
            let parsed: EventActor = s.parse().unwrap();
            assert_eq!(&parsed, actor);
        }
    }

    #[test]
    fn test_event_actor_serde() {
        let actor = EventActor::Migration;
        let json = serde_json::to_string(&actor).unwrap();
        assert_eq!(json, "\"migration\"");
        let parsed: EventActor = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, EventActor::Migration);
    }

    // --- MemoryEvent: fact_id -----------------------------------------------

    #[test]
    fn test_fact_id_all_variants() {
        let events: Vec<MemoryEvent> = vec![
            MemoryEvent::NoteCreated {
                note_path: "a".into(),
                content: "c".into(),
                note_type: NoteType::Other,
                path: "p".into(),
                namespace: "n".into(),
                agent: "w".into(),
                source: FactSource::Manual,
                source_memory_ids: vec![],
            },
            MemoryEvent::NoteContentUpdated {
                note_path: "b".into(),
                old_content: "o".into(),
                new_content: "n".into(),
                reason: "r".into(),
            },
            MemoryEvent::NoteMetadataUpdated {
                note_path: "c".into(),
                field: "tier".into(),
                old_value: "ShortTerm".into(),
                new_value: "LongTerm".into(),
            },
            MemoryEvent::NoteAccessed {
                note_path: "e".into(),
                query: None,
                relevance_score: None,
                used_in_response: false,
                new_access_count: 0,
            },
            MemoryEvent::NoteInvalidated {
                note_path: "g".into(),
                reason: "r".into(),
                actor: EventActor::Decay,
            },
            MemoryEvent::NoteRestored {
                note_path: "h".into(),
            },
            MemoryEvent::NoteDeleted {
                note_path: "i".into(),
                reason: "r".into(),
            },
            MemoryEvent::NoteConsolidated {
                note_path: "j".into(),
                source_note_paths: vec![],
                consolidated_content: "c".into(),
            },
            MemoryEvent::NoteMigrated {
                note_path: "k".into(),
                snapshot: serde_json::json!({}),
            },
        ];
        let expected = ["a", "b", "c", "e", "g", "h", "i", "j", "k"];
        for (evt, exp) in events.iter().zip(expected.iter()) {
            assert_eq!(evt.fact_id(), *exp);
        }
    }

    // --- MemoryEvent: event_type_tag ----------------------------------------

    #[test]
    fn test_event_type_tag_all_variants() {
        let cases: Vec<(MemoryEvent, &str)> = vec![
            (
                MemoryEvent::NoteCreated {
                    note_path: "f".into(),
                    content: String::new(),
                    note_type: NoteType::Other,
                    source: FactSource::Extracted,
                    path: String::new(),
                    namespace: "owner".into(),
                    agent: "default".into(),
                    source_memory_ids: vec![],
                },
                "NoteCreated",
            ),
            (
                MemoryEvent::NoteContentUpdated {
                    note_path: "f".into(),
                    old_content: String::new(),
                    new_content: String::new(),
                    reason: String::new(),
                },
                "NoteContentUpdated",
            ),
            (
                MemoryEvent::NoteMetadataUpdated {
                    note_path: "f".into(),
                    field: "tier".into(),
                    old_value: "a".into(),
                    new_value: "b".into(),
                },
                "NoteMetadataUpdated",
            ),
            (
                MemoryEvent::NoteAccessed {
                    note_path: "f".into(),
                    query: None,
                    relevance_score: None,
                    used_in_response: false,
                    new_access_count: 0,
                },
                "NoteAccessed",
            ),
            (
                MemoryEvent::NoteInvalidated {
                    note_path: "f".into(),
                    reason: String::new(),
                    actor: EventActor::System,
                },
                "NoteInvalidated",
            ),
            (
                MemoryEvent::NoteRestored {
                    note_path: "f".into(),
                },
                "NoteRestored",
            ),
            (
                MemoryEvent::NoteDeleted {
                    note_path: "f".into(),
                    reason: String::new(),
                },
                "NoteDeleted",
            ),
            (
                MemoryEvent::NoteConsolidated {
                    note_path: "f".into(),
                    source_note_paths: vec![],
                    consolidated_content: String::new(),
                },
                "NoteConsolidated",
            ),
            (
                MemoryEvent::NoteMigrated {
                    note_path: "f".into(),
                    snapshot: serde_json::json!({}),
                },
                "NoteMigrated",
            ),
        ];

        for (event, expected_tag) in &cases {
            assert_eq!(event.event_type_tag(), *expected_tag);
        }
        assert_eq!(cases.len(), 9);
    }

    // --- MemoryEvent: is_skeleton -------------------------------------------

    #[test]
    fn test_is_skeleton_classification() {
        // Pulse events
        assert!(!MemoryEvent::NoteAccessed {
            note_path: "f".into(),
            query: None,
            relevance_score: None,
            used_in_response: false,
            new_access_count: 0,
        }
        .is_skeleton());
        // Skeleton events
        assert!(MemoryEvent::NoteCreated {
            note_path: "f".into(),
            content: "c".into(),
            note_type: NoteType::Other,
            path: "p".into(),
            namespace: "n".into(),
            agent: "w".into(),
            source: FactSource::Extracted,
            source_memory_ids: vec![],
        }
        .is_skeleton());
        assert!(MemoryEvent::NoteDeleted {
            note_path: "f".into(),
            reason: "r".into()
        }
        .is_skeleton());
        assert!(MemoryEvent::NoteMigrated {
            note_path: "f".into(),
            snapshot: serde_json::json!({})
        }
        .is_skeleton());
    }

    // --- MemoryEvent: serde -------------------------------------------------

    #[test]
    fn test_event_serde_roundtrip_fact_created() {
        let event = MemoryEvent::NoteCreated {
            note_path: "fact-001".into(),
            content: "User prefers Rust".into(),
            note_type: NoteType::Preference,
            path: "aleph://user/preferences/language".into(),
            namespace: "owner".into(),
            agent: "default".into(),
            source: FactSource::Extracted,
            source_memory_ids: vec!["mem-001".into()],
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"NoteCreated\""));
        assert!(json.contains("User prefers Rust"));
        assert!(json.contains("aleph://user/preferences/language"));

        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fact_id(), "fact-001");
    }

    #[test]
    fn test_event_serde_roundtrip_fact_migrated_with_type_field() {
        // Edge case: snapshot contains a "type" field that could conflict with serde tag
        let snapshot = serde_json::json!({
            "type": "old_event_type",
            "id": "old-fact",
            "content": "test"
        });
        let event = MemoryEvent::NoteMigrated {
            note_path: "old-fact".into(),
            snapshot: snapshot.clone(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        if let MemoryEvent::NoteMigrated { snapshot: s, .. } = parsed {
            assert_eq!(s["type"].as_str(), Some("old_event_type"));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn test_event_serde_roundtrip_fact_migrated() {
        let snapshot = serde_json::json!({
            "id": "old-fact",
            "content": "test",
            "is_valid": true
        });
        let event = MemoryEvent::NoteMigrated {
            note_path: "old-fact".into(),
            snapshot: snapshot.clone(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        if let MemoryEvent::NoteMigrated { snapshot: s, .. } = parsed {
            assert_eq!(s, snapshot);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn test_event_serde_roundtrip_all_variants() {
        let events = vec![
            MemoryEvent::NoteCreated {
                note_path: "f".into(),
                content: "c".into(),
                note_type: NoteType::Learning,
                source: FactSource::Manual,
                path: "p".into(),
                namespace: "n".into(),
                agent: "w".into(),
                source_memory_ids: vec![],
            },
            MemoryEvent::NoteContentUpdated {
                note_path: "f".into(),
                old_content: "a".into(),
                new_content: "b".into(),
                reason: "correction".into(),
            },
            MemoryEvent::NoteMetadataUpdated {
                note_path: "f".into(),
                field: "scope".into(),
                old_value: "global".into(),
                new_value: "persona".into(),
            },
            MemoryEvent::NoteAccessed {
                note_path: "f".into(),
                query: Some("q".into()),
                relevance_score: Some(0.5),
                used_in_response: true,
                new_access_count: 3,
            },
            MemoryEvent::NoteInvalidated {
                note_path: "f".into(),
                reason: "outdated".into(),
                actor: EventActor::Decay,
            },
            MemoryEvent::NoteRestored {
                note_path: "f".into(),
            },
            MemoryEvent::NoteDeleted {
                note_path: "f".into(),
                reason: "user request".into(),
            },
            MemoryEvent::NoteConsolidated {
                note_path: "f".into(),
                source_note_paths: vec!["x".into()],
                consolidated_content: "merged".into(),
            },
            MemoryEvent::NoteMigrated {
                note_path: "f".into(),
                snapshot: serde_json::json!({"id": "old"}),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.event_type_tag(), event.event_type_tag());
            assert_eq!(parsed.fact_id(), event.fact_id());
        }
    }

    // --- MemoryEventEnvelope ------------------------------------------------

    #[test]
    fn test_envelope_new() {
        let event = MemoryEvent::NoteCreated {
            note_path: "fact-abc".into(),
            content: "Test fact".into(),
            note_type: NoteType::Other,
            source: FactSource::Extracted,
            path: "p".into(),
            namespace: "owner".into(),
            agent: "default".into(),
            source_memory_ids: vec![],
        };

        let envelope = MemoryEventEnvelope::new(
            "fact-abc".into(),
            1,
            event,
            EventActor::Agent,
            Some("corr-123".into()),
        );

        assert_eq!(envelope.fact_id, "fact-abc");
        assert_eq!(envelope.seq, 1);
        assert_eq!(envelope.actor, EventActor::Agent);
        assert_eq!(envelope.correlation_id.as_deref(), Some("corr-123"));
        assert_eq!(envelope.event_type_tag(), "NoteCreated");
        assert!(envelope.is_skeleton());
        assert!(envelope.timestamp > 0);
        assert_eq!(envelope.id, 0); // Not yet assigned by DB
    }

    #[test]
    fn test_envelope_pulse() {
        let envelope = MemoryEventEnvelope::new(
            "fact-xyz".into(),
            5,
            MemoryEvent::NoteAccessed {
                note_path: "fact-xyz".into(),
                query: None,
                relevance_score: None,
                used_in_response: false,
                new_access_count: 1,
            },
            EventActor::System,
            None,
        );

        assert_eq!(envelope.fact_id, "fact-xyz");
        assert!(!envelope.is_skeleton()); // Pulse event
    }

    #[test]
    fn test_envelope_serde_roundtrip() {
        let envelope = MemoryEventEnvelope::new(
            "fact-001".into(),
            3,
            MemoryEvent::NoteContentUpdated {
                note_path: "fact-001".into(),
                old_content: "old".into(),
                new_content: "new".into(),
                reason: "user correction".into(),
            },
            EventActor::User,
            Some("session-42".into()),
        );

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: MemoryEventEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.fact_id, envelope.fact_id);
        assert_eq!(parsed.seq, envelope.seq);
        assert_eq!(parsed.actor, envelope.actor);
        assert_eq!(parsed.timestamp, envelope.timestamp);
        assert_eq!(parsed.correlation_id, envelope.correlation_id);
        assert_eq!(parsed.event.event_type_tag(), "NoteContentUpdated");
    }

    // --- R2.2: legacy event aliases -----------------------------------------

    #[test]
    fn legacy_envelope_with_fact_created_deserializes_via_alias() {
        // Legacy on-disk events used the "Fact*" variant tag. After the
        // R2.2 rename to "Note*", the alias must still let old payloads through.
        // Note: enum is internally tagged via #[serde(tag = "type")].
        let json = r#"{
            "type": "FactCreated",
            "fact_id": "reference/rust",
            "content": "hello",
            "note_type": "other",
            "path": "p",
            "namespace": "owner",
            "agent": "default",
            "source": "manual",
            "source_memory_ids": []
        }"#;
        let parsed: MemoryEvent =
            serde_json::from_str(json).expect("alias must let old name through");
        match parsed {
            MemoryEvent::NoteCreated {
                note_path, content, ..
            } => {
                assert_eq!(note_path, "reference/rust");
                assert_eq!(content, "hello");
            }
            _ => panic!("expected NoteCreated via alias"),
        }
    }

    #[test]
    fn writes_only_note_created_name() {
        let ev = MemoryEvent::NoteCreated {
            note_path: "x".into(),
            content: "y".into(),
            note_type: NoteType::Other,
            path: "p".into(),
            namespace: "owner".into(),
            agent: "default".into(),
            source: FactSource::Manual,
            source_memory_ids: vec![],
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("NoteCreated"));
        assert!(!json.contains("FactCreated"));
        assert!(json.contains("note_path"));
        assert!(!json.contains("fact_id"));
    }

    #[test]
    fn legacy_field_alias_source_fact_ids_works() {
        // Legacy payloads used "source_fact_ids"; new format uses "source_note_paths".
        let json = r#"{
            "type": "NoteConsolidated",
            "note_path": "j",
            "source_fact_ids": ["a", "b"],
            "consolidated_content": "merged"
        }"#;
        let parsed: MemoryEvent =
            serde_json::from_str(json).expect("source_fact_ids alias must work");
        match parsed {
            MemoryEvent::NoteConsolidated {
                source_note_paths,
                consolidated_content,
                ..
            } => {
                assert_eq!(source_note_paths, vec!["a", "b"]);
                assert_eq!(consolidated_content, "merged");
            }
            _ => panic!("expected NoteConsolidated"),
        }
    }
}
