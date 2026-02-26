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
use std::collections::HashMap;

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
    /// One-shot migration from legacy CRUD store
    Migration,
}

impl std::fmt::Display for EventActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventActor::Agent => write!(f, "agent"),
            EventActor::User => write!(f, "user"),
            EventActor::System => write!(f, "system"),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MemoryEvent {
    // ------------------------------------------------------------------
    // Skeleton events (immediate persist)
    // ------------------------------------------------------------------
    /// A new fact was created
    FactCreated {
        content: String,
        fact_type: FactType,
        source: FactSource,
        tier: MemoryTier,
        scope: MemoryScope,
        /// Initial strength (0.0 ..= 1.0)
        strength: f32,
        /// Optional metadata key-value pairs
        metadata: HashMap<String, String>,
    },

    /// The textual content of a fact was updated
    FactContentUpdated {
        old_content: String,
        new_content: String,
        reason: String,
    },

    /// Non-content metadata was updated (tags, scope, etc.)
    FactMetadataUpdated {
        /// Key-value pairs that were changed (key -> new value)
        changed_fields: HashMap<String, String>,
    },

    /// The fact moved between memory tiers
    TierTransitioned {
        from: MemoryTier,
        to: MemoryTier,
        trigger: TierTransitionTrigger,
    },

    /// The fact was accessed / retrieved (Pulse)
    FactAccessed {
        /// The query that triggered retrieval, if any
        query: Option<String>,
        /// Relevance score at time of access
        relevance_score: Option<f32>,
        /// Whether the fact was actually used in the response
        used_in_response: bool,
    },

    /// The fact's strength decayed (Pulse)
    StrengthDecayed {
        old_strength: f32,
        new_strength: f32,
    },

    /// The fact was soft-deleted (invalidated)
    FactInvalidated {
        reason: String,
        /// Strength at time of invalidation
        strength_at_invalidation: Option<f32>,
    },

    /// The fact was restored from the recycle bin
    FactRestored {
        /// Strength assigned after restoration
        new_strength: f32,
    },

    /// The fact was permanently deleted
    FactDeleted {
        reason: String,
        /// Days spent in recycle bin before deletion
        days_in_recycle_bin: Option<u32>,
    },

    /// Multiple facts were consolidated into this one
    FactConsolidated {
        /// IDs of the source facts that were merged
        source_fact_ids: Vec<String>,
        /// Human-readable summary of what was consolidated
        summary: String,
    },

    /// The fact was migrated from the legacy CRUD store
    FactMigrated {
        /// Snapshot of the original CRUD record (JSON)
        legacy_snapshot: String,
    },
}

impl MemoryEvent {
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
/// Stored as a single row in the event store (SQLite). The `seq` field
/// provides a per-fact monotonic sequence for deterministic replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEventEnvelope {
    /// Globally unique event ID (UUID v4)
    pub id: String,
    /// The fact this event belongs to
    pub fact_id: String,
    /// Per-fact monotonic sequence number (1-based)
    pub seq: u64,
    /// Who caused this event
    pub actor: EventActor,
    /// When the event occurred (Unix timestamp, seconds)
    pub timestamp: i64,
    /// Optional correlation ID for tracing across subsystems
    pub correlation_id: Option<String>,
    /// The domain event payload
    pub event: MemoryEvent,
}

impl MemoryEventEnvelope {
    /// Create a new envelope with a fresh UUID and current timestamp.
    pub fn new(
        fact_id: String,
        seq: u64,
        actor: EventActor,
        event: MemoryEvent,
        correlation_id: Option<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            fact_id,
            seq,
            actor,
            timestamp: now,
            correlation_id,
            event,
        }
    }

    /// Return the fact_id this event belongs to.
    pub fn fact_id(&self) -> &str {
        &self.fact_id
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
        assert_eq!(EventActor::Migration.to_string(), "migration");
    }

    #[test]
    fn test_event_actor_from_str() {
        assert_eq!("agent".parse::<EventActor>().unwrap(), EventActor::Agent);
        assert_eq!("USER".parse::<EventActor>().unwrap(), EventActor::User);
        assert_eq!("System".parse::<EventActor>().unwrap(), EventActor::System);
        assert_eq!(
            "migration".parse::<EventActor>().unwrap(),
            EventActor::Migration
        );
    }

    #[test]
    fn test_event_actor_from_str_unknown() {
        let result = "unknown".parse::<EventActor>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown event actor"));
    }

    #[test]
    fn test_event_actor_roundtrip() {
        for actor in &[
            EventActor::Agent,
            EventActor::User,
            EventActor::System,
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
        assert_eq!(
            TierTransitionTrigger::Consolidation.to_string(),
            "consolidation"
        );
        assert_eq!(
            TierTransitionTrigger::Reinforcement.to_string(),
            "reinforcement"
        );
        assert_eq!(TierTransitionTrigger::Decay.to_string(), "decay");
    }

    #[test]
    fn test_tier_transition_trigger_from_str() {
        assert_eq!(
            "consolidation".parse::<TierTransitionTrigger>().unwrap(),
            TierTransitionTrigger::Consolidation
        );
        assert_eq!(
            "REINFORCEMENT".parse::<TierTransitionTrigger>().unwrap(),
            TierTransitionTrigger::Reinforcement
        );
        assert_eq!(
            "Decay".parse::<TierTransitionTrigger>().unwrap(),
            TierTransitionTrigger::Decay
        );
    }

    #[test]
    fn test_tier_transition_trigger_from_str_unknown() {
        let result = "promotion".parse::<TierTransitionTrigger>();
        assert!(result.is_err());
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

    #[test]
    fn test_tier_transition_trigger_serde() {
        let trigger = TierTransitionTrigger::Reinforcement;
        let json = serde_json::to_string(&trigger).unwrap();
        assert_eq!(json, "\"reinforcement\"");
        let parsed: TierTransitionTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, TierTransitionTrigger::Reinforcement);
    }

    // --- MemoryEvent: event_type_tag ----------------------------------------

    #[test]
    fn test_event_type_tag_all_variants() {
        let cases: Vec<(MemoryEvent, &str)> = vec![
            (
                MemoryEvent::FactCreated {
                    content: String::new(),
                    fact_type: FactType::Other,
                    source: FactSource::Extracted,
                    tier: MemoryTier::ShortTerm,
                    scope: MemoryScope::Global,
                    strength: 1.0,
                    metadata: HashMap::new(),
                },
                "FactCreated",
            ),
            (
                MemoryEvent::FactContentUpdated {
                    old_content: String::new(),
                    new_content: String::new(),
                    reason: String::new(),
                },
                "FactContentUpdated",
            ),
            (
                MemoryEvent::FactMetadataUpdated {
                    changed_fields: HashMap::new(),
                },
                "FactMetadataUpdated",
            ),
            (
                MemoryEvent::TierTransitioned {
                    from: MemoryTier::ShortTerm,
                    to: MemoryTier::LongTerm,
                    trigger: TierTransitionTrigger::Consolidation,
                },
                "TierTransitioned",
            ),
            (
                MemoryEvent::FactAccessed {
                    query: None,
                    relevance_score: None,
                    used_in_response: false,
                },
                "FactAccessed",
            ),
            (
                MemoryEvent::StrengthDecayed {
                    old_strength: 1.0,
                    new_strength: 0.9,
                },
                "StrengthDecayed",
            ),
            (
                MemoryEvent::FactInvalidated {
                    reason: String::new(),
                    strength_at_invalidation: None,
                },
                "FactInvalidated",
            ),
            (
                MemoryEvent::FactRestored {
                    new_strength: 0.5,
                },
                "FactRestored",
            ),
            (
                MemoryEvent::FactDeleted {
                    reason: String::new(),
                    days_in_recycle_bin: None,
                },
                "FactDeleted",
            ),
            (
                MemoryEvent::FactConsolidated {
                    source_fact_ids: vec![],
                    summary: String::new(),
                },
                "FactConsolidated",
            ),
            (
                MemoryEvent::FactMigrated {
                    legacy_snapshot: String::new(),
                },
                "FactMigrated",
            ),
        ];

        for (event, expected_tag) in &cases {
            assert_eq!(
                event.event_type_tag(),
                *expected_tag,
                "Wrong tag for {:?}",
                expected_tag
            );
        }

        // Verify we tested all 11 variants
        assert_eq!(cases.len(), 11);
    }

    // --- MemoryEvent: is_skeleton -------------------------------------------

    #[test]
    fn test_is_skeleton_skeleton_events() {
        let skeleton_events = vec![
            MemoryEvent::FactCreated {
                content: "test".into(),
                fact_type: FactType::Other,
                source: FactSource::Extracted,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                strength: 1.0,
                metadata: HashMap::new(),
            },
            MemoryEvent::FactContentUpdated {
                old_content: "a".into(),
                new_content: "b".into(),
                reason: "update".into(),
            },
            MemoryEvent::FactMetadataUpdated {
                changed_fields: HashMap::new(),
            },
            MemoryEvent::TierTransitioned {
                from: MemoryTier::ShortTerm,
                to: MemoryTier::LongTerm,
                trigger: TierTransitionTrigger::Consolidation,
            },
            MemoryEvent::FactInvalidated {
                reason: "stale".into(),
                strength_at_invalidation: Some(0.1),
            },
            MemoryEvent::FactRestored {
                new_strength: 0.5,
            },
            MemoryEvent::FactDeleted {
                reason: "purge".into(),
                days_in_recycle_bin: Some(30),
            },
            MemoryEvent::FactConsolidated {
                source_fact_ids: vec!["a".into()],
                summary: "merged".into(),
            },
            MemoryEvent::FactMigrated {
                legacy_snapshot: "{}".into(),
            },
        ];

        for event in &skeleton_events {
            assert!(
                event.is_skeleton(),
                "{} should be Skeleton",
                event.event_type_tag()
            );
        }
    }

    #[test]
    fn test_is_skeleton_pulse_events() {
        let pulse_events = vec![
            MemoryEvent::FactAccessed {
                query: Some("rust".into()),
                relevance_score: Some(0.95),
                used_in_response: true,
            },
            MemoryEvent::StrengthDecayed {
                old_strength: 0.8,
                new_strength: 0.7,
            },
        ];

        for event in &pulse_events {
            assert!(
                !event.is_skeleton(),
                "{} should be Pulse (not Skeleton)",
                event.event_type_tag()
            );
        }
    }

    // --- MemoryEvent: serde -------------------------------------------------

    #[test]
    fn test_event_serde_roundtrip_fact_created() {
        let event = MemoryEvent::FactCreated {
            content: "User prefers Rust".into(),
            fact_type: FactType::Preference,
            source: FactSource::Extracted,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            strength: 0.85,
            metadata: {
                let mut m = HashMap::new();
                m.insert("topic".into(), "programming".into());
                m
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        // Verify the tag is present
        assert!(json.contains("\"type\":\"FactCreated\""));
        assert!(json.contains("User prefers Rust"));

        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type_tag(), "FactCreated");
        if let MemoryEvent::FactCreated {
            content, strength, ..
        } = &parsed
        {
            assert_eq!(content, "User prefers Rust");
            assert!((strength - 0.85).abs() < f32::EPSILON);
        } else {
            panic!("Wrong variant after deserialization");
        }
    }

    #[test]
    fn test_event_serde_roundtrip_tier_transitioned() {
        let event = MemoryEvent::TierTransitioned {
            from: MemoryTier::ShortTerm,
            to: MemoryTier::LongTerm,
            trigger: TierTransitionTrigger::Reinforcement,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"TierTransitioned\""));

        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        if let MemoryEvent::TierTransitioned {
            from, to, trigger, ..
        } = &parsed
        {
            assert_eq!(*from, MemoryTier::ShortTerm);
            assert_eq!(*to, MemoryTier::LongTerm);
            assert_eq!(*trigger, TierTransitionTrigger::Reinforcement);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn test_event_serde_roundtrip_fact_consolidated() {
        let event = MemoryEvent::FactConsolidated {
            source_fact_ids: vec!["fact-1".into(), "fact-2".into(), "fact-3".into()],
            summary: "Merged 3 related programming preferences".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();

        if let MemoryEvent::FactConsolidated {
            source_fact_ids,
            summary,
        } = &parsed
        {
            assert_eq!(source_fact_ids.len(), 3);
            assert!(summary.contains("Merged"));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn test_event_serde_roundtrip_all_variants() {
        // Verify every variant survives a JSON roundtrip
        let events = vec![
            MemoryEvent::FactCreated {
                content: "c".into(),
                fact_type: FactType::Learning,
                source: FactSource::Manual,
                tier: MemoryTier::Core,
                scope: MemoryScope::Workspace,
                strength: 1.0,
                metadata: HashMap::new(),
            },
            MemoryEvent::FactContentUpdated {
                old_content: "a".into(),
                new_content: "b".into(),
                reason: "correction".into(),
            },
            MemoryEvent::FactMetadataUpdated {
                changed_fields: {
                    let mut m = HashMap::new();
                    m.insert("scope".into(), "persona".into());
                    m
                },
            },
            MemoryEvent::TierTransitioned {
                from: MemoryTier::LongTerm,
                to: MemoryTier::ShortTerm,
                trigger: TierTransitionTrigger::Decay,
            },
            MemoryEvent::FactAccessed {
                query: Some("q".into()),
                relevance_score: Some(0.5),
                used_in_response: true,
            },
            MemoryEvent::StrengthDecayed {
                old_strength: 0.9,
                new_strength: 0.8,
            },
            MemoryEvent::FactInvalidated {
                reason: "outdated".into(),
                strength_at_invalidation: Some(0.05),
            },
            MemoryEvent::FactRestored {
                new_strength: 0.6,
            },
            MemoryEvent::FactDeleted {
                reason: "user request".into(),
                days_in_recycle_bin: Some(14),
            },
            MemoryEvent::FactConsolidated {
                source_fact_ids: vec!["x".into()],
                summary: "s".into(),
            },
            MemoryEvent::FactMigrated {
                legacy_snapshot: "{\"id\":\"old\"}".into(),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(
                parsed.event_type_tag(),
                event.event_type_tag(),
                "Roundtrip failed for {}",
                event.event_type_tag()
            );
        }
    }

    // --- MemoryEventEnvelope ------------------------------------------------

    #[test]
    fn test_envelope_new() {
        let event = MemoryEvent::FactCreated {
            content: "Test fact".into(),
            fact_type: FactType::Other,
            source: FactSource::Extracted,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            strength: 1.0,
            metadata: HashMap::new(),
        };

        let envelope = MemoryEventEnvelope::new(
            "fact-abc".into(),
            1,
            EventActor::Agent,
            event,
            Some("corr-123".into()),
        );

        assert_eq!(envelope.fact_id(), "fact-abc");
        assert_eq!(envelope.seq, 1);
        assert_eq!(envelope.actor, EventActor::Agent);
        assert_eq!(envelope.correlation_id.as_deref(), Some("corr-123"));
        assert_eq!(envelope.event_type_tag(), "FactCreated");
        assert!(envelope.is_skeleton());
        assert!(envelope.timestamp > 0);
        // UUID format check
        assert_eq!(envelope.id.len(), 36);
        assert!(envelope.id.contains('-'));
    }

    #[test]
    fn test_envelope_fact_id() {
        let envelope = MemoryEventEnvelope::new(
            "fact-xyz".into(),
            5,
            EventActor::System,
            MemoryEvent::StrengthDecayed {
                old_strength: 0.5,
                new_strength: 0.4,
            },
            None,
        );

        assert_eq!(envelope.fact_id(), "fact-xyz");
        assert!(!envelope.is_skeleton()); // Pulse event
    }

    #[test]
    fn test_envelope_serde_roundtrip() {
        let envelope = MemoryEventEnvelope::new(
            "fact-001".into(),
            3,
            EventActor::User,
            MemoryEvent::FactContentUpdated {
                old_content: "old".into(),
                new_content: "new".into(),
                reason: "user correction".into(),
            },
            Some("session-42".into()),
        );

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: MemoryEventEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, envelope.id);
        assert_eq!(parsed.fact_id, envelope.fact_id);
        assert_eq!(parsed.seq, envelope.seq);
        assert_eq!(parsed.actor, envelope.actor);
        assert_eq!(parsed.timestamp, envelope.timestamp);
        assert_eq!(parsed.correlation_id, envelope.correlation_id);
        assert_eq!(parsed.event.event_type_tag(), "FactContentUpdated");
    }

    #[test]
    fn test_envelope_is_skeleton_delegates() {
        let skeleton = MemoryEventEnvelope::new(
            "f1".into(),
            1,
            EventActor::Agent,
            MemoryEvent::FactDeleted {
                reason: "purge".into(),
                days_in_recycle_bin: None,
            },
            None,
        );
        assert!(skeleton.is_skeleton());

        let pulse = MemoryEventEnvelope::new(
            "f2".into(),
            1,
            EventActor::System,
            MemoryEvent::FactAccessed {
                query: None,
                relevance_score: None,
                used_in_response: false,
            },
            None,
        );
        assert!(!pulse.is_skeleton());
    }

    #[test]
    fn test_envelope_unique_ids() {
        let e1 = MemoryEventEnvelope::new(
            "f".into(),
            1,
            EventActor::Agent,
            MemoryEvent::FactRestored {
                new_strength: 0.5,
            },
            None,
        );
        let e2 = MemoryEventEnvelope::new(
            "f".into(),
            2,
            EventActor::Agent,
            MemoryEvent::FactRestored {
                new_strength: 0.5,
            },
            None,
        );
        assert_ne!(e1.id, e2.id, "Each envelope should get a unique UUID");
    }
}
