//! Memory Event Sourcing
//!
//! Event-sourced memory lifecycle management. Every mutation to a `MemoryFact`
//! is captured as an immutable `MemoryEvent` wrapped in a `MemoryEventEnvelope`.
//!
//! ## Skeleton vs Pulse
//!
//! Events follow the Skeleton/Pulse classification from the resilience layer:
//! - **Skeleton** — structural mutations that must be persisted immediately
//!   (FactCreated, FactContentUpdated, FactMetadataUpdated, TierTransitioned,
//!    FactInvalidated, FactRestored, FactDeleted, FactConsolidated, FactMigrated)
//! - **Pulse** — high-frequency observations that may be buffered before persist
//!   (FactAccessed, StrengthDecayed)
//!
//! ## Submodules
//!
//! - `commands`   — command structs dispatched to the handler
//! - `handler`    — `MemoryCommandHandler` processes commands into events
//! - `projector`  — `EventProjector` folds events into current-state projections
//! - `traveler`   — `MemoryTimeTraveler` replays events to reconstruct past state
//! - `migration`  — one-shot migration from legacy CRUD to event-sourced facts

pub mod commands;
pub mod handler;
pub mod migration;
pub mod projector;
pub mod traveler;

use serde::{Deserialize, Serialize};

use crate::memory::context::{FactSource, FactType, MemoryScope, MemoryTier};

// ============================================================================
// EventActor — who caused the event
// ============================================================================

/// The actor that caused a memory event.
///
/// Modeled after [`crate::memory::audit::AuditActor`] but extended with
/// `Migration` for the one-shot CRUD-to-ES migration.
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
            EventActor::Agent => write!(f, "agent"),
            EventActor::User => write!(f, "user"),
            EventActor::System => write!(f, "system"),
            EventActor::Decay => write!(f, "decay"),
            EventActor::Migration => write!(f, "migration"),
        }
    }
}

impl std::str::FromStr for EventActor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "agent" => Ok(EventActor::Agent),
            "user" => Ok(EventActor::User),
            "system" => Ok(EventActor::System),
            "decay" => Ok(EventActor::Decay),
            "migration" => Ok(EventActor::Migration),
            _ => Err(format!("Unknown event actor: {}", s)),
        }
    }
}

// ============================================================================
// TierTransitionTrigger — why a tier transition happened
// ============================================================================

/// The trigger that caused a memory tier transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierTransitionTrigger {
    /// Consolidation promoted the fact (e.g. ShortTerm -> LongTerm)
    Consolidation,
    /// Repeated access reinforced the fact (e.g. ShortTerm -> LongTerm)
    Reinforcement,
    /// Decay demoted the fact (e.g. LongTerm -> ShortTerm)
    Decay,
}

impl std::fmt::Display for TierTransitionTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TierTransitionTrigger::Consolidation => write!(f, "consolidation"),
            TierTransitionTrigger::Reinforcement => write!(f, "reinforcement"),
            TierTransitionTrigger::Decay => write!(f, "decay"),
        }
    }
}

impl std::str::FromStr for TierTransitionTrigger {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "consolidation" => Ok(TierTransitionTrigger::Consolidation),
            "reinforcement" => Ok(TierTransitionTrigger::Reinforcement),
            "decay" => Ok(TierTransitionTrigger::Decay),
            _ => Err(format!("Unknown tier transition trigger: {}", s)),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MemoryEvent {
    // ------------------------------------------------------------------
    // Skeleton events (immediate persist)
    // ------------------------------------------------------------------
    /// A new fact was created
    FactCreated {
        fact_id: String,
        content: String,
        fact_type: FactType,
        tier: MemoryTier,
        scope: MemoryScope,
        path: String,
        namespace: String,
        workspace: String,
        confidence: f32,
        source: FactSource,
        source_memory_ids: Vec<String>,
    },

    /// The textual content of a fact was updated
    FactContentUpdated {
        fact_id: String,
        old_content: String,
        new_content: String,
        reason: String,
    },

    /// A single metadata field was updated
    FactMetadataUpdated {
        fact_id: String,
        field: String,
        old_value: String,
        new_value: String,
    },

    /// The fact moved between memory tiers
    TierTransitioned {
        fact_id: String,
        from_tier: MemoryTier,
        to_tier: MemoryTier,
        trigger: TierTransitionTrigger,
    },

    // ------------------------------------------------------------------
    // Pulse events (buffered persist)
    // ------------------------------------------------------------------
    /// The fact was accessed / retrieved
    FactAccessed {
        fact_id: String,
        query: Option<String>,
        relevance_score: Option<f32>,
        used_in_response: bool,
        new_access_count: u32,
    },

    /// The fact's strength decayed
    StrengthDecayed {
        fact_id: String,
        old_strength: f32,
        new_strength: f32,
        decay_factor: f32,
    },

    // ------------------------------------------------------------------
    // Skeleton events (continued)
    // ------------------------------------------------------------------
    /// The fact was soft-deleted (invalidated)
    FactInvalidated {
        fact_id: String,
        reason: String,
        actor: EventActor,
        strength_at_invalidation: Option<f32>,
    },

    /// The fact was restored from the recycle bin
    FactRestored {
        fact_id: String,
        new_strength: f32,
    },

    /// The fact was permanently deleted
    FactDeleted {
        fact_id: String,
        reason: String,
    },

    /// Multiple facts were consolidated into this one
    FactConsolidated {
        fact_id: String,
        source_fact_ids: Vec<String>,
        consolidated_content: String,
    },

    /// The fact was migrated from the legacy CRUD store
    FactMigrated {
        fact_id: String,
        snapshot: serde_json::Value,
    },
}

impl MemoryEvent {
    /// Extract the fact_id from any event variant.
    pub fn fact_id(&self) -> &str {
        match self {
            MemoryEvent::FactCreated { fact_id, .. }
            | MemoryEvent::FactContentUpdated { fact_id, .. }
            | MemoryEvent::FactMetadataUpdated { fact_id, .. }
            | MemoryEvent::TierTransitioned { fact_id, .. }
            | MemoryEvent::FactAccessed { fact_id, .. }
            | MemoryEvent::StrengthDecayed { fact_id, .. }
            | MemoryEvent::FactInvalidated { fact_id, .. }
            | MemoryEvent::FactRestored { fact_id, .. }
            | MemoryEvent::FactDeleted { fact_id, .. }
            | MemoryEvent::FactConsolidated { fact_id, .. }
            | MemoryEvent::FactMigrated { fact_id, .. } => fact_id,
        }
    }

    /// Return the serde tag string for this event variant.
    ///
    /// Matches the `#[serde(tag = "type")]` discriminant so callers can
    /// filter events by type without deserializing the full payload.
    pub fn event_type_tag(&self) -> &'static str {
        match self {
            MemoryEvent::FactCreated { .. } => "FactCreated",
            MemoryEvent::FactContentUpdated { .. } => "FactContentUpdated",
            MemoryEvent::FactMetadataUpdated { .. } => "FactMetadataUpdated",
            MemoryEvent::TierTransitioned { .. } => "TierTransitioned",
            MemoryEvent::FactAccessed { .. } => "FactAccessed",
            MemoryEvent::StrengthDecayed { .. } => "StrengthDecayed",
            MemoryEvent::FactInvalidated { .. } => "FactInvalidated",
            MemoryEvent::FactRestored { .. } => "FactRestored",
            MemoryEvent::FactDeleted { .. } => "FactDeleted",
            MemoryEvent::FactConsolidated { .. } => "FactConsolidated",
            MemoryEvent::FactMigrated { .. } => "FactMigrated",
        }
    }

    /// Whether this event is a Skeleton event (must be persisted immediately).
    ///
    /// Only `FactAccessed` and `StrengthDecayed` are Pulse (buffered).
    /// All other variants are Skeleton.
    pub fn is_skeleton(&self) -> bool {
        !matches!(
            self,
            MemoryEvent::FactAccessed { .. } | MemoryEvent::StrengthDecayed { .. }
        )
    }
}

// ============================================================================
// MemoryEventEnvelope — metadata wrapper
// ============================================================================

/// Immutable envelope wrapping a `MemoryEvent` with metadata.
///
/// Stored as a single row in the event store (SQLite). The `id` field
/// is the SQLite auto-increment primary key (0 before insert, assigned
/// on write). The `seq` field provides per-fact monotonic ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEventEnvelope {
    /// Auto-increment global ID (assigned by SQLite on insert; 0 before insert).
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
    pub fn new(
        fact_id: String,
        seq: u64,
        event: MemoryEvent,
        actor: EventActor,
        correlation_id: Option<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
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
    pub fn event_type_tag(&self) -> &'static str {
        self.event.event_type_tag()
    }

    /// Convenience: whether the inner event is Skeleton.
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

    // --- TierTransitionTrigger ----------------------------------------------

    #[test]
    fn test_tier_transition_trigger_display() {
        assert_eq!(TierTransitionTrigger::Consolidation.to_string(), "consolidation");
        assert_eq!(TierTransitionTrigger::Reinforcement.to_string(), "reinforcement");
        assert_eq!(TierTransitionTrigger::Decay.to_string(), "decay");
    }

    #[test]
    fn test_tier_transition_trigger_roundtrip() {
        for trigger in &[
            TierTransitionTrigger::Consolidation,
            TierTransitionTrigger::Reinforcement,
            TierTransitionTrigger::Decay,
        ] {
            let s = trigger.to_string();
            let parsed: TierTransitionTrigger = s.parse().unwrap();
            assert_eq!(&parsed, trigger);
        }
    }

    // --- MemoryEvent: fact_id -----------------------------------------------

    #[test]
    fn test_fact_id_all_variants() {
        let events: Vec<MemoryEvent> = vec![
            MemoryEvent::FactCreated {
                fact_id: "a".into(), content: "c".into(), fact_type: FactType::Other,
                tier: MemoryTier::ShortTerm, scope: MemoryScope::Global,
                path: "p".into(), namespace: "n".into(), workspace: "w".into(),
                confidence: 1.0, source: FactSource::Manual, source_memory_ids: vec![],
            },
            MemoryEvent::FactContentUpdated {
                fact_id: "b".into(), old_content: "o".into(),
                new_content: "n".into(), reason: "r".into(),
            },
            MemoryEvent::FactMetadataUpdated {
                fact_id: "c".into(), field: "tier".into(),
                old_value: "ShortTerm".into(), new_value: "LongTerm".into(),
            },
            MemoryEvent::TierTransitioned {
                fact_id: "d".into(), from_tier: MemoryTier::ShortTerm,
                to_tier: MemoryTier::LongTerm, trigger: TierTransitionTrigger::Consolidation,
            },
            MemoryEvent::FactAccessed {
                fact_id: "e".into(), query: None, relevance_score: None,
                used_in_response: false, new_access_count: 0,
            },
            MemoryEvent::StrengthDecayed {
                fact_id: "f".into(), old_strength: 1.0, new_strength: 0.5, decay_factor: 0.5,
            },
            MemoryEvent::FactInvalidated {
                fact_id: "g".into(), reason: "r".into(),
                actor: EventActor::Decay, strength_at_invalidation: Some(0.05),
            },
            MemoryEvent::FactRestored { fact_id: "h".into(), new_strength: 0.8 },
            MemoryEvent::FactDeleted { fact_id: "i".into(), reason: "r".into() },
            MemoryEvent::FactConsolidated {
                fact_id: "j".into(), source_fact_ids: vec![], consolidated_content: "c".into(),
            },
            MemoryEvent::FactMigrated {
                fact_id: "k".into(), snapshot: serde_json::json!({}),
            },
        ];
        let expected = ["a","b","c","d","e","f","g","h","i","j","k"];
        for (evt, exp) in events.iter().zip(expected.iter()) {
            assert_eq!(evt.fact_id(), *exp);
        }
    }

    // --- MemoryEvent: event_type_tag ----------------------------------------

    #[test]
    fn test_event_type_tag_all_variants() {
        let cases: Vec<(MemoryEvent, &str)> = vec![
            (
                MemoryEvent::FactCreated {
                    fact_id: "f".into(), content: String::new(), fact_type: FactType::Other,
                    source: FactSource::Extracted, tier: MemoryTier::ShortTerm,
                    scope: MemoryScope::Global, path: String::new(), namespace: "owner".into(),
                    workspace: "default".into(), confidence: 1.0,
                    source_memory_ids: vec![],
                },
                "FactCreated",
            ),
            (
                MemoryEvent::FactContentUpdated {
                    fact_id: "f".into(), old_content: String::new(),
                    new_content: String::new(), reason: String::new(),
                },
                "FactContentUpdated",
            ),
            (
                MemoryEvent::FactMetadataUpdated {
                    fact_id: "f".into(), field: "tier".into(),
                    old_value: "a".into(), new_value: "b".into(),
                },
                "FactMetadataUpdated",
            ),
            (
                MemoryEvent::TierTransitioned {
                    fact_id: "f".into(), from_tier: MemoryTier::ShortTerm,
                    to_tier: MemoryTier::LongTerm, trigger: TierTransitionTrigger::Consolidation,
                },
                "TierTransitioned",
            ),
            (
                MemoryEvent::FactAccessed {
                    fact_id: "f".into(), query: None, relevance_score: None,
                    used_in_response: false, new_access_count: 0,
                },
                "FactAccessed",
            ),
            (
                MemoryEvent::StrengthDecayed {
                    fact_id: "f".into(), old_strength: 1.0, new_strength: 0.9, decay_factor: 0.95,
                },
                "StrengthDecayed",
            ),
            (
                MemoryEvent::FactInvalidated {
                    fact_id: "f".into(), reason: String::new(),
                    actor: EventActor::System, strength_at_invalidation: None,
                },
                "FactInvalidated",
            ),
            (
                MemoryEvent::FactRestored { fact_id: "f".into(), new_strength: 0.5 },
                "FactRestored",
            ),
            (
                MemoryEvent::FactDeleted { fact_id: "f".into(), reason: String::new() },
                "FactDeleted",
            ),
            (
                MemoryEvent::FactConsolidated {
                    fact_id: "f".into(), source_fact_ids: vec![],
                    consolidated_content: String::new(),
                },
                "FactConsolidated",
            ),
            (
                MemoryEvent::FactMigrated {
                    fact_id: "f".into(), snapshot: serde_json::json!({}),
                },
                "FactMigrated",
            ),
        ];

        for (event, expected_tag) in &cases {
            assert_eq!(event.event_type_tag(), *expected_tag);
        }
        assert_eq!(cases.len(), 11);
    }

    // --- MemoryEvent: is_skeleton -------------------------------------------

    #[test]
    fn test_is_skeleton_classification() {
        // Pulse events
        assert!(!MemoryEvent::FactAccessed {
            fact_id: "f".into(), query: None, relevance_score: None,
            used_in_response: false, new_access_count: 0,
        }.is_skeleton());
        assert!(!MemoryEvent::StrengthDecayed {
            fact_id: "f".into(), old_strength: 1.0, new_strength: 0.9, decay_factor: 0.95,
        }.is_skeleton());

        // Skeleton events
        assert!(MemoryEvent::FactCreated {
            fact_id: "f".into(), content: "c".into(), fact_type: FactType::Other,
            tier: MemoryTier::ShortTerm, scope: MemoryScope::Global,
            path: "p".into(), namespace: "n".into(), workspace: "w".into(),
            confidence: 1.0, source: FactSource::Extracted, source_memory_ids: vec![],
        }.is_skeleton());
        assert!(MemoryEvent::FactDeleted { fact_id: "f".into(), reason: "r".into() }.is_skeleton());
        assert!(MemoryEvent::FactMigrated { fact_id: "f".into(), snapshot: serde_json::json!({}) }.is_skeleton());
    }

    // --- MemoryEvent: serde -------------------------------------------------

    #[test]
    fn test_event_serde_roundtrip_fact_created() {
        let event = MemoryEvent::FactCreated {
            fact_id: "fact-001".into(),
            content: "User prefers Rust".into(),
            fact_type: FactType::Preference,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            path: "aleph://user/preferences/language".into(),
            namespace: "owner".into(),
            workspace: "default".into(),
            confidence: 0.85,
            source: FactSource::Extracted,
            source_memory_ids: vec!["mem-001".into()],
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"FactCreated\""));
        assert!(json.contains("User prefers Rust"));
        assert!(json.contains("aleph://user/preferences/language"));

        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fact_id(), "fact-001");
    }

    #[test]
    fn test_event_serde_roundtrip_fact_migrated() {
        let snapshot = serde_json::json!({
            "id": "old-fact",
            "content": "test",
            "is_valid": true
        });
        let event = MemoryEvent::FactMigrated {
            fact_id: "old-fact".into(),
            snapshot: snapshot.clone(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        if let MemoryEvent::FactMigrated { snapshot: s, .. } = parsed {
            assert_eq!(s, snapshot);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn test_event_serde_roundtrip_all_variants() {
        let events = vec![
            MemoryEvent::FactCreated {
                fact_id: "f".into(), content: "c".into(), fact_type: FactType::Learning,
                source: FactSource::Manual, tier: MemoryTier::Core, scope: MemoryScope::Workspace,
                path: "p".into(), namespace: "n".into(), workspace: "w".into(),
                confidence: 1.0, source_memory_ids: vec![],
            },
            MemoryEvent::FactContentUpdated {
                fact_id: "f".into(), old_content: "a".into(),
                new_content: "b".into(), reason: "correction".into(),
            },
            MemoryEvent::FactMetadataUpdated {
                fact_id: "f".into(), field: "scope".into(),
                old_value: "global".into(), new_value: "persona".into(),
            },
            MemoryEvent::TierTransitioned {
                fact_id: "f".into(), from_tier: MemoryTier::LongTerm,
                to_tier: MemoryTier::ShortTerm, trigger: TierTransitionTrigger::Decay,
            },
            MemoryEvent::FactAccessed {
                fact_id: "f".into(), query: Some("q".into()),
                relevance_score: Some(0.5), used_in_response: true, new_access_count: 3,
            },
            MemoryEvent::StrengthDecayed {
                fact_id: "f".into(), old_strength: 0.9, new_strength: 0.8, decay_factor: 0.95,
            },
            MemoryEvent::FactInvalidated {
                fact_id: "f".into(), reason: "outdated".into(),
                actor: EventActor::Decay, strength_at_invalidation: Some(0.05),
            },
            MemoryEvent::FactRestored { fact_id: "f".into(), new_strength: 0.6 },
            MemoryEvent::FactDeleted { fact_id: "f".into(), reason: "user request".into() },
            MemoryEvent::FactConsolidated {
                fact_id: "f".into(), source_fact_ids: vec!["x".into()],
                consolidated_content: "merged".into(),
            },
            MemoryEvent::FactMigrated {
                fact_id: "f".into(), snapshot: serde_json::json!({"id": "old"}),
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
        let event = MemoryEvent::FactCreated {
            fact_id: "fact-abc".into(),
            content: "Test fact".into(),
            fact_type: FactType::Other,
            source: FactSource::Extracted,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            path: "p".into(),
            namespace: "owner".into(),
            workspace: "default".into(),
            confidence: 1.0,
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
        assert_eq!(envelope.event_type_tag(), "FactCreated");
        assert!(envelope.is_skeleton());
        assert!(envelope.timestamp > 0);
        assert_eq!(envelope.id, 0); // Not yet assigned by DB
    }

    #[test]
    fn test_envelope_pulse() {
        let envelope = MemoryEventEnvelope::new(
            "fact-xyz".into(),
            5,
            MemoryEvent::StrengthDecayed {
                fact_id: "fact-xyz".into(),
                old_strength: 0.5,
                new_strength: 0.4,
                decay_factor: 0.95,
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
            MemoryEvent::FactContentUpdated {
                fact_id: "fact-001".into(),
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
        assert_eq!(parsed.event.event_type_tag(), "FactContentUpdated");
    }
}
